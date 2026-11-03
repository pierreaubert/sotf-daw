"""Capture remaining DFN2 integration checks with their actual exit status."""

import json
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
ARTIFACTS = Path(__file__).resolve().parent
COMMON = [
    "--release",
    "--jobs",
    "2",
    "--target-dir",
    "/Volumes/home_tmp/tmp/sotf-daw-audit-20261018-target",
]
CASES = [
    (
        "engine-check-with-qa",
        ["check", "-p", "sotf-engine", "--tests", "--features", "deep-filter,sotf-plugins/qa"],
    ),
    (
        "legacy-speech-tests",
        [
            "test",
            "-p",
            "sotf-plugin-speech-denoiser",
            "--no-default-features",
            "--features",
            "qa",
            "--lib",
            "--test",
            "integration",
        ],
    ),
    (
        "actual-model-tests",
        [
            "test",
            "-p",
            "plugins-denoiser",
            "--features",
            "deep-filter",
            "--test",
            "deep_filter",
        ],
    ),
    (
        "engine-policy-roundtrip",
        [
            "test",
            "-p",
            "sotf-engine",
            "--features",
            "deep-filter,sotf-plugins/qa",
            "--lib",
            "speech_backend_policy_survives_serialization_and_engine_conversion",
        ],
    ),
]
results = []
for name, arguments in CASES:
    command = ["rtk", "proxy", "cargo", *arguments, *COMMON]
    row = {
        "name": name,
        "command": command,
        "status": "running",
        "started_unix": time.time(),
    }
    results.append(row)
    (ARTIFACTS / "remaining-validation.json").write_text(
        json.dumps(results, indent=2) + "\n"
    )
    print(f"Starting {name}", flush=True)
    with (ARTIFACTS / f"{name}.stdout.txt").open("w") as output:
        result = subprocess.run(
            command, check=False, cwd=ROOT, stdout=output, stderr=subprocess.STDOUT
        )
    row.update(
        status="passed" if result.returncode == 0 else "failed",
        exit_code=result.returncode,
        finished_unix=time.time(),
    )
    (ARTIFACTS / "remaining-validation.json").write_text(
        json.dumps(results, indent=2) + "\n"
    )
    print(f"{name}: exit {result.returncode}", flush=True)
    if result.returncode:
        raise SystemExit(result.returncode)
