"""Fresh-process matched CREPE runtime measurements, then separate placement probes."""

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

import numpy as np
from crepe_runtime_corpus import corpus

PROFILES = ("tract_native", "ort_cpu", "coreml_cpu", "coreml_gpu_allowed")


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def run_case(root, argv, timeout_s=180):
    """Darwin whole-process peak RSS includes startup, import, model and scratch."""
    command = ["/usr/bin/time", "-l", *argv] if platform.system() == "Darwin" else argv
    start = time.perf_counter_ns()
    # Own the wrapper and benchmark as one process group. Killing only `time`
    # would leave a timed-out runtime running into subsequent measurements.
    with subprocess.Popen(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,
    ) as child:
        try:
            stdout, stderr = child.communicate(timeout=timeout_s)
            code = child.returncode
        except subprocess.TimeoutExpired:
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass  # The owned group exited between timeout and cleanup.
            stdout, stderr = child.communicate()
            code = 124
            stderr += (
                f"\nbenchmark process group exceeded{timeout_s}s; no runtime score\n"
            )
    result = subprocess.CompletedProcess(command, code, stdout, stderr)
    wall_ns = time.perf_counter_ns() - start
    (root / "stderr.txt").write_text(result.stderr)
    (root / "stdout.txt").write_text(result.stdout)
    match = re.search(
        r"^\s*(\d+)\s+maximum resident set size\s*$", result.stderr, re.MULTILINE
    )
    return {
        "command": command,
        "exit_code": result.returncode,
        "process_total_wall_ns": wall_ns,
        "whole_process_peak_rss_bytes": int(match[1]) if match else None,
        "memory_scope": "Darwin time -l high-water RSS for complete child process, including language/framework startup; not model heap alone",
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--model-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--smoke", action="store_true")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    renderer = args.output / "tract.frozen"
    shutil.copy2(args.renderer, renderer)
    model = args.output / "crepe-tiny.frozen.onnx"
    shutil.copy2(args.model_dir / "crepe-tiny.onnx", model)
    manifest = json.loads((args.model_dir / "manifest.json").read_text())
    if digest(model) != manifest["sha256"]:
        raise ValueError("Frozen model/manifest mismatch")
    worker = args.output / "ort_worker.frozen.py"
    shutil.copy2(Path(__file__).parent / "ort_crepe_runtime.py", worker)
    sources = {}
    for name in [
        Path(__file__).name,
        "crepe_runtime_corpus.py",
        "ort_crepe_runtime.py",
    ]:
        target = args.output / (name + ".txt")
        shutil.copy2(Path(__file__).parent / name, target)
        sources[name] = digest(target)
    frames, labels = corpus()
    input_path = args.output / "corpus.f32le"
    input_path.write_bytes(frames.astype("<f4").tobytes())
    renderer_sha, worker_sha = digest(renderer), digest(worker)
    rows = []
    rng = np.random.default_rng(20309121)
    plan = [
        (trial, profile)
        for trial in range(1 if args.smoke else 3)
        for profile in rng.permutation(PROFILES)
    ]
    # All timed sessions finish before placement profiling starts.
    plan += [(None, profile) for profile in PROFILES if profile != "tract_native"]
    for index, (trial, profile) in enumerate(plan):
        root = args.output / str(index)
        root.mkdir()
        placement = trial is None
        request = {
            "input": str(input_path.resolve()),
            "output": str((root / "activations.f32le").resolve()),
            "report": str((root / "native.json").resolve()),
            "model": str(model.resolve()),
            "sha256": manifest["sha256"],
            "frames": len(frames),
            "warmup_cycles": 0 if args.smoke else 5,
            "measured_cycles": 1 if args.smoke else 20,
        }
        path = root / "request.json"
        path.write_text(json.dumps(request, indent=2) + "\n")
        argv = (
            [str(renderer.resolve()), str(path.resolve())]
            if profile == "tract_native"
            else [
                sys.executable,
                str(worker.resolve()),
                "--request",
                str(path.resolve()),
                "--profile",
                profile,
                *(["--placement"] if placement else []),
            ]
        )
        run = run_case(root, argv)
        run["files"] = {
            name: {"path": str((root / name).resolve()), "sha256": digest(root / name)}
            for name in [
                "request.json",
                "stderr.txt",
                "stdout.txt",
                "native.json",
                "activations.f32le",
            ]
            if (root / name).exists()
        }
        rows.append(
            {"trial": trial, "profile": profile, "placement_only": placement, **run}
        )
        # Preserve partial progress if a provider is unsupported or crashes.
        (args.output / "progress.json").write_text(json.dumps(rows, indent=2) + "\n")
    if (
        digest(renderer) != renderer_sha
        or digest(worker) != worker_sha
        or digest(model) != manifest["sha256"]
    ):
        raise ValueError("Frozen executable/script/model changed during matrix")
    report = {
        "smoke": args.smoke,
        "scope": "same-model matched activation wrappers; CPU and CoreML eligibility; no worker/audio integration, device guarantee or runtime winner",
        "renderer_sha256": renderer_sha,
        "ort_worker_sha256": worker_sha,
        "model_sha256": manifest["sha256"],
        "input_sha256": digest(input_path),
        "source_sha256": sources,
        "labels": labels,
        "runs": rows,
        "platform": platform.platform(),
        "machine": platform.machine(),
        "logical_cpus": os.cpu_count(),
        "environment": {
            k: os.environ.get(k)
            for k in [
                "OPENBLAS_NUM_THREADS",
                "OMP_NUM_THREADS",
                "VECLIB_MAXIMUM_THREADS",
            ]
        },
        "ordering": "three predeclared randomized rounds, then separate profiling processes; no cache flush or thermal/affinity control",
        "deployment_admitted": False,
    }
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(
        json.dumps(
            {
                "runs": len(rows),
                "failed_runs": sum(r["exit_code"] != 0 for r in rows),
                "renderer_sha256": renderer_sha,
            }
        )
    )


if __name__ == "__main__":
    main()
