"""Frozen native PND backend/load comparison using the original source fixtures."""

import argparse
import json
import os
import platform
import shutil
import subprocess
import time
from pathlib import Path

import numpy as np
from pnd_compare_cases import KINDS, fixture
from run_crepe_runtime_matrix import digest, run_case

BACKENDS = ("tract", "ort_cpu", "ort_coreml_cpu_only")
LOADS = (0, 2)
ORDER_SEED = 20309226


def build_processes():
    """Record build/test executables without collecting unrelated process arguments."""
    listing = subprocess.check_output(["ps", "-axo", "pid,comm"], text=True)
    names = {"cargo", "cargo-nextest", "rustc", "clang", "clang++", "ld", "ld-prime"}
    return [
        line.strip()
        for line in listing.splitlines()
        if Path(line.split(maxsplit=1)[-1]).name in names
    ]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--model-dir", type=Path, required=True)
    parser.add_argument("--ort-library", type=Path, required=True)
    parser.add_argument("--ort-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--smoke", action="store_true")
    parser.add_argument("--wait-for-quiet-seconds", type=int, default=0)
    args = parser.parse_args()
    deadline = time.monotonic() + args.wait_for_quiet_seconds
    while not args.smoke and build_processes():
        if time.monotonic() >= deadline:
            raise RuntimeError("Build/test processes active; timing matrix not started")
        print("Waiting for unrelated build/test processes before timing", flush=True)
        time.sleep(min(30, max(0, deadline - time.monotonic())))
    if digest(args.ort_library) != args.ort_sha256:
        raise ValueError("ORT library identity mismatch")
    args.output.mkdir(parents=True, exist_ok=False)
    frozen = args.output / "renderer.frozen"
    shutil.copy2(args.renderer, frozen)
    model = args.output / "crepe-tiny.frozen.onnx"
    shutil.copy2(args.model_dir / "crepe-tiny.onnx", model)
    model_sha = json.loads((args.model_dir / "manifest.json").read_text())["sha256"]
    if digest(model) != model_sha:
        raise ValueError("Model manifest identity mismatch")
    renderer_sha = digest(frozen)
    sources = {}
    for name in [
        Path(__file__).name,
        "pnd_compare_cases.py",
        "run_crepe_runtime_matrix.py",
        "analyze_pnd_comparison.py",
        "analyze_pnd_backend_matrix.py",
    ]:
        target = args.output / (name + ".txt")
        shutil.copy2(Path(__file__).parent / name, target)
        sources[name] = digest(target)
    plan = [
        (seed, kind, backend, load)
        for seed in ([20308120] if args.smoke else [20308120, 20308121])
        for kind in (["offset"] if args.smoke else KINDS)
        for backend in BACKENDS
        for load in LOADS
    ]
    order = np.random.default_rng(ORDER_SEED).permutation(len(plan))
    plan = [plan[int(i)] for i in order]
    protocol = {
        "smoke": args.smoke,
        "ordering_seed": ORDER_SEED,
        "plan": plan,
        "serving_probe": "offset; raw acceptance >=0.9; selected settled decisions >=0.8",
        "confidence_parity_tolerance": 2e-5,
        "accepted_pitch_parity_cents": 0.1,
        "deployment_admitted": False,
    }
    (args.output / "protocol.json").write_text(json.dumps(protocol, indent=2) + "\n")
    cases = []
    for index, (seed, kind, backend, load) in enumerate(plan):
        before = build_processes()
        if before and not args.smoke:
            raise RuntimeError(
                "Build/test activity resumed; partial results are unqualified"
            )
        x, _, _, identity = fixture(seed, kind, seconds=0.25 if args.smoke else 3)
        root = args.output / str(index)
        root.mkdir()
        (root / "input.f32le").write_bytes(x.tobytes())
        runtime = (
            None
            if backend == "tract"
            else {
                "library_path": str(args.ort_library.resolve()),
                "library_sha256": args.ort_sha256,
                "provider": "cpu" if backend == "ort_cpu" else "core_ml_cpu_only",
            }
        )
        request = {
            "input": str((root / "input.f32le").resolve()),
            "classical_output": str((root / "classical.f32le").resolve()),
            "neural_output": str((root / "neural.f32le").resolve()),
            "report": str((root / "native.json").resolve()),
            "frames": len(x),
            "sample_rate": 48000,
            "reference_hz": identity["reference_hz"],
            "model": {
                "model_path": str(model.resolve()),
                "sha256": model_sha,
                "min_confidence": 0.8,
                "max_age_ms": 150.0,
                "max_deviation_cents": 100.0,
                "source_channel": 0,
            },
            "neural_first": bool(seed % 2),
            "cpu_workers": load,
            "ort_runtime": runtime,
        }
        path = root / "request.json"
        path.write_text(json.dumps(request, indent=2) + "\n")
        run = run_case(
            root, [str(frozen.resolve()), str(path.resolve())], timeout_s=120
        )
        after = build_processes()
        run["build_processes_before"] = before
        run["build_processes_after"] = after
        cases.append(
            {
                "identity": identity,
                "backend": backend,
                "cpu_workers": load,
                "neural_first": request["neural_first"],
                "process": run,
                "files": {
                    name: {
                        "path": str((root / name).resolve()),
                        "sha256": digest(root / name),
                    }
                    for name in [
                        "input.f32le",
                        "classical.f32le",
                        "neural.f32le",
                        "native.json",
                        "request.json",
                        "stderr.txt",
                        "stdout.txt",
                    ]
                    if (root / name).exists()
                },
            }
        )
        (args.output / "progress.json").write_text(json.dumps(cases, indent=2) + "\n")
        print(
            f"{index + 1}/{len(plan)} {backend} load={load} {seed} {kind} exit={run['exit_code']}",
            flush=True,
        )
        if after and not args.smoke:
            raise RuntimeError(
                "Build/test activity resumed; partial results are unqualified"
            )
    if (
        digest(frozen) != renderer_sha
        or digest(model) != model_sha
        or digest(args.ort_library) != args.ort_sha256
    ):
        raise ValueError("Executable/model/library changed during matrix")
    report = {
        "smoke": args.smoke,
        "cases": cases,
        "renderer_sha256": renderer_sha,
        "model_sha256": model_sha,
        "ort_library": str(args.ort_library.resolve()),
        "ort_sha256": args.ort_sha256,
        "source_sha256": sources,
        "protocol_sha256": digest(args.output / "protocol.json"),
        "platform": platform.platform(),
        "logical_cpus": os.cpu_count(),
        "environment": {
            k: os.environ.get(k)
            for k in [
                "OPENBLAS_NUM_THREADS",
                "OMP_NUM_THREADS",
                "VECLIB_MAXIMUM_THREADS",
            ]
        },
        "scope": "one randomized local round; paired callbacks; CPU load after initialization/raw estimates; no affinity/thermal control, hardware audio or runtime admission",
        "deployment_admitted": False,
    }
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
