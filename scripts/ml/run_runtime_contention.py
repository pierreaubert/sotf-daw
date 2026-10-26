"""Run paced actual-model SOTF track/bus cases in isolated child processes."""

import argparse
import hashlib
import json
import os
import platform
import random
import shutil
import subprocess
import time
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--models-dir", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    frozen = (args.output / "runtime.frozen").resolve()
    shutil.copy2(args.binary, frozen)
    cases = [
        (tracks, cpu, df, pitch)
        for tracks in [1, 4]
        for cpu in [0, 2]
        for df in [False, True]
        for pitch in [False, True]
    ]
    random.Random(20261026).shuffle(cases)
    reports = []
    for i, (tracks, cpu, df, pitch) in enumerate(cases):
        root = args.output / str(i)
        root.mkdir()
        request = {
            "tracks": tracks,
            "block_frames": 128,
            "warmup_ms": 1000,
            "measure_ms": 2000,
            "cpu_workers": cpu,
            "deep_filter": df,
            "neural_pitch": pitch,
            "denoise_first": False,
            "playout_packets": 4,
            "models_dir": str(args.models_dir.resolve()),
            "output": str((root / "report.json").resolve()),
        }
        path = root / "request.json"
        path.write_text(json.dumps(request, indent=2) + "\n")
        start = time.monotonic()
        try:
            p = subprocess.run(
                [str(frozen), str(path.resolve())],
                capture_output=True,
                text=True,
                timeout=60,
                check=False,
            )
            code, stdout, stderr = p.returncode, p.stdout, p.stderr
        except subprocess.TimeoutExpired as error:
            code, stdout, stderr = (
                -1,
                str(error.stdout or ""),
                str(error.stderr or "") + "\ncase timed out after60s",
            )
        (root / "stdout.txt").write_text(stdout)
        (root / "stderr.txt").write_text(stderr)
        row = {
            "case": i,
            "request": request,
            "exit_code": code,
            "wall_seconds": time.monotonic() - start,
        }
        if code == 0:
            report = json.loads(Path(request["output"]).read_text())
            row["report"] = report
        reports.append(row)
        print(
            json.dumps(
                {
                    "case": i,
                    "tracks": tracks,
                    "cpu_workers": cpu,
                    "deep_filter": df,
                    "neural_pitch": pitch,
                    "exit_code": code,
                }
            ),
            flush=True,
        )
    report = {
        "binary_sha256": hashlib.sha256(frozen.read_bytes()).hexdigest(),
        "environment": {
            "system": platform.system(),
            "release": platform.release(),
            "architecture": platform.machine(),
            "logical_cpus": os.cpu_count(),
        },
        "case_order_seed": 20261026,
        "cases": reports,
        "runtime_choice_admitted": False,
        "scope": "one local paced native-graph workload matrix; models differ; no same-network runtime winner, GPU or device-callback claim",
    }
    (args.output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    raise SystemExit(any(r["exit_code"] != 0 for r in reports))


if __name__ == "__main__":
    main()
