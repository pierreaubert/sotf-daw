"""Fresh-process ORT activation measurement or separate provider-placement pass."""

import time

ENTRY_NS = time.perf_counter_ns()

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
import onnxruntime as ort

PROFILES = {
    "ort_cpu": ["CPUExecutionProvider"],
    "coreml_cpu": [
        (
            "CoreMLExecutionProvider",
            {
                "ModelFormat": "NeuralNetwork",
                "MLComputeUnits": "CPUOnly",
                "RequireStaticInputShapes": "1",
            },
        ),
        "CPUExecutionProvider",
    ],
    "coreml_gpu_allowed": [
        (
            "CoreMLExecutionProvider",
            {
                "ModelFormat": "NeuralNetwork",
                "MLComputeUnits": "CPUAndGPU",
                "RequireStaticInputShapes": "1",
            },
        ),
        "CPUExecutionProvider",
    ],
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--request", type=Path, required=True)
    parser.add_argument("--profile", choices=PROFILES, required=True)
    parser.add_argument("--placement", action="store_true")
    args = parser.parse_args()
    with args.request.open("rb") as stream:
        raw = stream.read(65537)
    if len(raw) > 65536:
        raise ValueError("Request exceeds64KiB")
    request = json.loads(raw)
    count = request["frames"]
    if (
        type(count) is not int
        or not 1 <= count <= 128
        or not 0 <= request["warmup_cycles"] <= 100
        or not 1 <= request["measured_cycles"] <= 100
    ):
        raise ValueError("Invalid bounded activation corpus/cycles")
    with Path(request["input"]).open("rb") as stream:
        raw = stream.read(count * 4096 + 1)
    if len(raw) != count * 4096:
        raise ValueError("Activation corpus size mismatch")
    frames = np.frombuffer(raw, dtype="<f4").reshape(count, 1, 1024)
    if not np.isfinite(frames).all() or np.max(abs(frames)) > 64:
        raise ValueError("Invalid normalized corpus")
    options = ort.SessionOptions()
    options.intra_op_num_threads = 1
    options.inter_op_num_threads = 1
    options.execution_mode = ort.ExecutionMode.ORT_SEQUENTIAL
    options.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
    options.enable_profiling = args.placement
    if args.placement:
        options.profile_file_prefix = str(args.request.parent / "placement")
    load_start = time.perf_counter_ns()
    with Path(request["model"]).open("rb") as stream:
        model = stream.read(64 * 1024 * 1024 + 1)
    if (
        len(model) > 64 * 1024 * 1024
        or hashlib.sha256(model).hexdigest() != request["sha256"]
    ):
        raise ValueError("CREPE model size/hash mismatch")
    session = ort.InferenceSession(
        model, sess_options=options, providers=PROFILES[args.profile]
    )
    session.disable_fallback()
    if (
        args.profile != "ort_cpu"
        and "CoreMLExecutionProvider" not in session.get_providers()
    ):
        raise ValueError(
            "Requested CoreML provider is not registered; refusing a mislabeled CPU fallback"
        )
    if session.get_inputs()[0].shape != [1, 1024] or session.get_outputs()[0].shape != [
        1,
        360,
    ]:
        raise ValueError("CREPE activation shape mismatch")
    metadata = session.get_modelmeta().custom_metadata_map
    for name, expected in {
        "sotf_contract": "crepe-tiny-v1",
        "sample_rate": "16000",
        "frame_samples": "1024",
        "normalization": "zero-mean-sample-std-ddof1-min1e-10",
    }.items():
        if metadata.get(name) != expected:
            raise ValueError("CREPE metadata contract mismatch")
    load_ns = time.perf_counter_ns() - load_start
    ready_ns = time.perf_counter_ns() - ENTRY_NS
    input_name = session.get_inputs()[0].name

    def infer(frame):
        if frame.shape != (1, 1024) or not np.isfinite(frame).all():
            raise ValueError("Invalid finite frame")
        result = session.run(None, {input_name: frame})[0]
        if (
            result.shape != (1, 360)
            or not np.isfinite(result).all()
            or np.any((result < 0) | (result > 1))
        ):
            raise ValueError("Invalid CREPE activations")
        return result[0].copy()

    first = time.perf_counter_ns()
    infer(frames[0])
    first_ns = time.perf_counter_ns() - first
    outputs = None
    if args.placement:
        outputs = np.stack([infer(frame) for frame in frames])
    timings = []
    measurement_wall_ns = None
    if not args.placement:
        for _ in range(request["warmup_cycles"]):
            for frame in frames:
                infer(frame)
        # Capture parity output after warming, as the native wrapper does.
        outputs = np.stack([infer(frame) for frame in frames])
        begin = time.perf_counter_ns()
        for _ in range(request["measured_cycles"]):
            for frame in frames:
                start = time.perf_counter_ns()
                infer(frame)
                timings.append(time.perf_counter_ns() - start)
        measurement_wall_ns = time.perf_counter_ns() - begin
    report = {
        "backend": args.profile,
        "model_sha256": request["sha256"],
        "frames": count,
        "warmup_cycles": request["warmup_cycles"],
        "measured_cycles": request["measured_cycles"],
        "model_load_ns": load_ns,
        "ready_from_entry_ns": ready_ns,
        "first_inference_ns": first_ns,
        "timings_ns": timings,
        "measurement_wall_ns": measurement_wall_ns,
        "onnxruntime_version": ort.__version__,
        "numpy_version": np.__version__,
        "requested_providers": PROFILES[args.profile],
        "registered_providers": session.get_providers(),
        "provider_options": session.get_provider_options(),
        "placement_only": args.placement,
        "thread_policy": "ORT sequential, intra_op=1 inter_op=1; CoreML device scheduling is outside this control",
        "scope": "Python public activation wrapper includes input finite check, feed view/dict, output domain check/copy; no normalization/resampling/worker/audio",
    }
    if args.placement:
        profile_path = Path(session.end_profiling())
        events = json.loads(profile_path.read_text())
        counts = {}
        kernels = {}
        for event in events:
            provider = event.get("args", {}).get("provider")
            if event.get("cat") == "Node" and provider:
                counts[provider] = counts.get(provider, 0) + 1
                kernels.setdefault(provider, set()).add(event["name"])
        report["node_kernel_events_by_provider"] = counts
        report["kernel_names_by_provider"] = {k: sorted(v) for k, v in kernels.items()}
        report["profile_file"] = str(profile_path)
        report["profile_sha256"] = hashlib.sha256(profile_path.read_bytes()).hexdigest()
        report["actual_gpu_execution_proven"] = False
    with Path(request["output"]).open("xb") as stream:
        stream.write(outputs.astype("<f4").tobytes())
    with Path(request["report"]).open("x") as stream:
        stream.write(json.dumps(report, indent=2, allow_nan=False) + "\n")


if __name__ == "__main__":
    main()
