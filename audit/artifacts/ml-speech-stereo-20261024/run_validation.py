"""Capture linked-stereo DFN2 gates; preserve actual command status and stdout."""

import json
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
OUTPUT = Path(__file__).resolve().parent
COMMON = [
    "--release",
    "--jobs",
    "2",
    "--target-dir",
    "/Volumes/home_tmp/tmp/sotf-daw-audit-20261018-target",
]
CASES = [
    (
        "strict-clippy",
        [
            "clippy",
            "-p",
            "plugins-denoiser",
            "-p",
            "sotf-plugin-speech-denoiser",
            "--all-targets",
            "--features",
            "sotf-plugin-speech-denoiser/deep-filter,sotf-plugin-speech-denoiser/qa",
        ],
        ["--", "-D", "warnings"],
    ),
    (
        "adapter-model-tests",
        [
            "test",
            "-p",
            "plugins-denoiser",
            "--features",
            "deep-filter",
            "--test",
            "deep_filter",
        ],
        ["--", "--nocapture"],
    ),
    (
        "host-model-tests",
        [
            "test",
            "-p",
            "sotf-plugin-speech-denoiser",
            "--features",
            "deep-filter,qa",
            "--lib",
            "--test",
            "integration",
            "--test",
            "deep_filter",
        ],
        ["--", "--nocapture"],
    ),
    (
        "feature-disabled-tests",
        [
            "test",
            "-p",
            "sotf-plugin-speech-denoiser",
            "--features",
            "qa",
            "--no-default-features",
            "--lib",
            "--test",
            "integration",
        ],
        [],
    ),
    (
        "combined-engine-check",
        [
            "check",
            "-p",
            "sotf-engine",
            "--tests",
            "--features",
            "deep-filter,neural-pitch,sotf-plugins/qa",
        ],
        [],
    ),
]
results = []
for name, arguments, extra in CASES:
    cmd = ["rtk", "proxy", "cargo", *arguments, *COMMON, *extra]
    row = {
        "name": name,
        "command": cmd,
        "status": "running",
        "started_unix": time.time(),
    }
    results.append(row)
    (OUTPUT / "validation.json").write_text(json.dumps(results, indent=2) + "\n")
    print(f"Starting {name}", flush=True)
    with (OUTPUT / f"{name}.stdout.txt").open("w") as output:
        result = subprocess.run(
            cmd, cwd=ROOT, stdout=output, stderr=subprocess.STDOUT, check=False
        )
    row.update(
        status="passed" if result.returncode == 0 else "failed",
        exit_code=result.returncode,
        finished_unix=time.time(),
    )
    (OUTPUT / "validation.json").write_text(json.dumps(results, indent=2) + "\n")
    print(f"{name}: exit {result.returncode}", flush=True)
    if result.returncode:
        raise SystemExit(result.returncode)
