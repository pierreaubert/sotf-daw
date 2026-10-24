#!/usr/bin/env python3
"""Specialize upstream ONNX shape annotations for the fixed native VAD contract.

Batch is one and the 16 kHz input has 64 context + 512 new samples. No weights,
operators, edges, or non-graph attributes are changed. Fixing nested batch
annotations avoids an unresolved subgraph symbol in the native runtime.
"""

import argparse
import hashlib
import json
from pathlib import Path

import onnx

SOURCE_SHA = "7671cd04b004e9076da0d4a7b1a5aec36adf161c39230c1cb94a4fd5db6bbd28"


def digest(data):
    return hashlib.sha256(data).hexdigest()


def graphs(graph):
    yield graph
    for node in graph.node:
        for attr in node.attribute:
            if attr.type == onnx.AttributeProto.GRAPH:
                yield from graphs(attr.g)


def contents(model):
    """Fingerprint model math separately from shape annotations."""
    return [
        {
            "initializers": [
                (v.name, digest(v.SerializeToString())) for v in g.initializer
            ],
            "nodes": [
                [
                    n.op_type,
                    n.domain,
                    list(n.input),
                    list(n.output),
                    [
                        digest(a.SerializeToString())
                        for a in n.attribute
                        if a.type != onnx.AttributeProto.GRAPH
                    ],
                ]
                for n in g.node
            ],
        }
        for g in graphs(model.graph)
    ]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    source = args.source.read_bytes()
    if digest(source) != SOURCE_SHA:
        raise ValueError("Upstream model identity mismatch")
    if args.output.exists():
        raise ValueError("Output exists; preserve prior specialization")
    model = onnx.load_from_string(source)
    before = contents(model)
    changed = 0
    for graph in graphs(model.graph):
        for value in [*graph.input, *graph.output, *graph.value_info]:
            for dim in value.type.tensor_type.shape.dim:
                if dim.dim_param == "batch":
                    dim.ClearField("dim_param")
                    dim.dim_value = 1
                    changed += 1
    # Only the outer source input is constrained to the wrapper's 16 kHz frame.
    audio = next(v for v in model.graph.input if v.name == "input")
    audio.type.tensor_type.shape.dim[1].ClearField("dim_param")
    audio.type.tensor_type.shape.dim[1].dim_value = 576
    if contents(model) != before:
        raise ValueError("Model math changed during shape specialization")
    onnx.checker.check_model(model)
    output = model.SerializeToString()
    args.output.write_bytes(output)
    report = {
        "source_sha256": SOURCE_SHA,
        "output_sha256": digest(output),
        "batch_annotations_fixed": changed,
        "source_samples": 576,
        "operators_weights_edges_unchanged": True,
        "onnx_version": onnx.__version__,
    }
    args.output.with_suffix(".recipe.json").write_text(
        json.dumps(report, indent=2) + "\n"
    )
    print(json.dumps(report))


if __name__ == "__main__":
    main()
