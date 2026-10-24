#!/usr/bin/env python3
"""Export the pinned pretrained CREPE tiny model, without training on QA inputs."""

import argparse
import hashlib
import importlib.metadata
import json
from pathlib import Path

import onnx
import torch
import torchcrepe
from onnx import helper, numpy_helper


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    if importlib.metadata.version("torchcrepe") != "0.0.24":
        raise RuntimeError("This export requires torchcrepe==0.0.24")
    torch.set_num_threads(1)
    torchcrepe.load.model("cpu", "tiny")
    model = torchcrepe.infer.model.eval()
    args.output.mkdir(parents=True, exist_ok=True)
    target = args.output / "crepe-tiny.onnx"
    # The legacy exporter represents constant F.pad with a dynamic shape graph
    # unsupported by tract. Express the exact padding in Conv attributes instead;
    # retain Conv -> ReLU -> BatchNorm -> MaxPool order and the source weights.
    initializers = []
    nodes = []

    def constant(name, value):
        initializers.append(numpy_helper.from_array(value, name))
        return name

    import numpy as np

    constant("input_shape", np.array([1, 1, 1024, 1], dtype=np.int64))
    nodes.append(helper.make_node("Reshape", ["audio", "input_shape"], ["x0"]))
    for layer in range(1, 7):
        conv = getattr(model, f"conv{layer}")
        bn = getattr(model, f"conv{layer}_BN")
        for name, value in [
            ("weight", conv.weight),
            ("bias", conv.bias),
            ("scale", bn.weight),
            ("offset", bn.bias),
            ("mean", bn.running_mean),
            ("variance", bn.running_var),
        ]:
            constant(f"l{layer}_{name}", value.detach().cpu().numpy())
        pads = [254, 0, 254, 0] if layer == 1 else [31, 0, 32, 0]
        strides = [4, 1] if layer == 1 else [1, 1]
        nodes += [
            helper.make_node(
                "Conv",
                [f"x{layer - 1}", f"l{layer}_weight", f"l{layer}_bias"],
                [f"conv{layer}"],
                pads=pads,
                strides=strides,
            ),
            helper.make_node("Relu", [f"conv{layer}"], [f"relu{layer}"]),
            helper.make_node(
                "BatchNormalization",
                [
                    f"relu{layer}",
                    f"l{layer}_scale",
                    f"l{layer}_offset",
                    f"l{layer}_mean",
                    f"l{layer}_variance",
                ],
                [f"bn{layer}"],
                epsilon=bn.eps,
            ),
            helper.make_node(
                "MaxPool",
                [f"bn{layer}"],
                [f"x{layer}"],
                kernel_shape=[2, 1],
                strides=[2, 1],
            ),
        ]
    constant("flat_shape", np.array([1, 256], dtype=np.int64))
    constant("classifier_weight", model.classifier.weight.detach().cpu().numpy())
    constant("classifier_bias", model.classifier.bias.detach().cpu().numpy())
    constant("probability_min", np.array(0.0, dtype=np.float32))
    constant("probability_max", np.array(1.0, dtype=np.float32))
    nodes += [
        helper.make_node("Transpose", ["x6"], ["permuted"], perm=[0, 2, 1, 3]),
        helper.make_node("Reshape", ["permuted", "flat_shape"], ["flat"]),
        helper.make_node(
            "Gemm",
            ["flat", "classifier_weight", "classifier_bias"],
            ["logits"],
            transB=1,
        ),
        helper.make_node("Sigmoid", ["logits"], ["sigmoid"]),
        # Some runtime vectorized sigmoid approximations slightly overshoot
        # [0,1]. Clip restores the exact mathematical sigmoid range, without
        # relaxing the consumer's rejection of invalid/non-finite activations.
        helper.make_node(
            "Clip", ["sigmoid", "probability_min", "probability_max"], ["activation"]
        ),
    ]
    graph = helper.make_graph(
        nodes,
        "CREPE-tiny",
        [helper.make_tensor_value_info("audio", onnx.TensorProto.FLOAT, [1, 1024])],
        [helper.make_tensor_value_info("activation", onnx.TensorProto.FLOAT, [1, 360])],
        initializers,
    )
    proto = helper.make_model(
        graph,
        opset_imports=[helper.make_opsetid("", 17)],
        ir_version=8,
        producer_name="sotf-crepe-export",
    )
    onnx.helper.set_model_props(
        proto,
        {
            "sotf_contract": "crepe-tiny-v1",
            "sample_rate": "16000",
            "frame_samples": "1024",
            "normalization": "zero-mean-sample-std-ddof1-min1e-10",
            "source": "torchcrepe==0.0.24/tiny.pth",
        },
    )
    onnx.checker.check_model(proto)
    onnx.save(proto, target)
    package = Path(torchcrepe.__file__).parent
    license_path = Path(
        importlib.metadata.distribution("torchcrepe").locate_file(
            "torchcrepe-0.0.24.dist-info/licenses/LICENSE"
        )
    )
    license_text = license_path.read_text()
    (args.output / "LICENSE-torchcrepe").write_text(license_text)
    (args.output / "LICENSE-crepe").write_text(
        license_text.replace(
            "Copyright (c) 2020 Max Morrison", "Copyright (c) 2018 Jong Wook Kim"
        )
    )
    # Deterministic parity inputs only: these are not training or quality evidence.
    generator = torch.Generator().manual_seed(20261024)
    frames = torch.randn(4, 1024, generator=generator)
    frames -= frames.mean(dim=1, keepdim=True)
    frames /= frames.std(dim=1, keepdim=True).clamp(min=1e-10)
    with torch.no_grad():
        outputs = model(frames).tolist()
    parity = {"input": frames.tolist(), "activation": outputs}
    (args.output / "parity.json").write_text(json.dumps(parity) + "\n")
    manifest = {
        "model": target.name,
        "sha256": digest(target),
        "source_weights_sha256": digest(package / "assets/tiny.pth"),
        "source": "https://github.com/maxrmorrison/torchcrepe",
        "original": "https://github.com/marl/crepe",
        "license": "MIT",
        "capacity": "tiny",
        "training": "upstream pretrained",
        "training_overlap": "unknown; parity fixtures do not establish held-out quality",
        "packages": {
            p: importlib.metadata.version(p)
            for p in ["torch", "torchcrepe", "onnx", "numpy"]
        },
        "parity_sha256": digest(args.output / "parity.json"),
        "export_script_sha256": digest(Path(__file__)),
    }
    (args.output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps(manifest, indent=2))


if __name__ == "__main__":
    main()
