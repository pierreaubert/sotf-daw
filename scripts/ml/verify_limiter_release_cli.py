#!/usr/bin/env python3
"""Exercise release preset export, abstention, classical fallback and invalid inputs."""

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evaluation-work", type=Path, required=True)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.work.exists() or args.output.exists():
        raise ValueError("Use new verification paths")
    args.work.mkdir(parents=True)
    digest = hashlib.sha256(args.model.read_bytes()).hexdigest()
    executable = Path(__file__).with_name("fit_limiter_release.py")
    positive = args.evaluation_work / "fresh-00/target.json"
    negative = args.evaluation_work / "no-limiting/target.json"
    invalid_data = json.loads(positive.read_text())
    for key in ["dry", "reference"]:
        invalid_data[key] = str((positive.parent / invalid_data[key]).resolve())
    invalid_data["fixed_params"]["oversampling"] = 1
    bad_profile = args.work / "invalid-profile.json"
    bad_profile.write_text(json.dumps(invalid_data) + "\n")
    rows = []
    for name, target, use_model, expected_code, expected_export in [
        ("learned-positive", positive, True, 0, True),
        ("classical-positive", positive, False, 0, True),
        ("no-limiting", negative, True, 2, False),
        ("bad-checksum", positive, True, 1, False),
        ("unsupported-profile", bad_profile, True, 1, False),
    ]:
        output = args.work / (name + "-output")
        work = args.work / (name + "-native")
        command = [
            sys.executable,
            str(executable),
            "--target",
            str(target),
            "--renderer",
            str(args.renderer),
            "--output",
            str(output),
            "--work",
            str(work),
        ]
        if use_model:
            command += [
                "--model",
                str(args.model),
                "--model-sha256",
                "0" * 64 if name == "bad-checksum" else digest,
            ]
        result = subprocess.run(
            command, check=False, capture_output=True, text=True, timeout=600
        )
        preset = output / "preset.json"
        if result.returncode != expected_code or preset.exists() != expected_export:
            raise ValueError("Unexpected CLI result for " + name + ": " + result.stderr)
        report = (
            json.loads((output / "report.json").read_text())
            if output.exists()
            else None
        )
        if expected_export:
            if (
                not report["accepted"]
                or json.loads(preset.read_text()) != report["parameters"]
            ):
                raise ValueError("Preset differs from qualified native parameters")
        elif report is not None and (
            report["accepted"] or report["parameters"] is not None
        ):
            raise ValueError("Abstention exposed export parameters")
        row = {
            "name": name,
            "command": command,
            "exit_code": result.returncode,
            "preset_exported": preset.exists(),
            "stdout": result.stdout,
            "stderr": result.stderr,
            "report": report,
        }
        rows.append(row)
        if name == "learned-positive":
            before = {
                p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                for p in output.iterdir()
            }
            repeated = subprocess.run(
                command, check=False, capture_output=True, text=True, timeout=30
            )
            after = {
                p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                for p in output.iterdir()
            }
            if repeated.returncode == 0 or before != after:
                raise ValueError("Existing output was overwritten")
            rows.append(
                {
                    "name": "overwrite-rejected",
                    "exit_code": repeated.returncode,
                    "output_hashes_unchanged": True,
                }
            )
    args.output.write_text(
        json.dumps({"rows": rows, "all_expected_results": True}, indent=2) + "\n"
    )
    print(json.dumps({"checks": len(rows), "all_expected_results": True}))


if __name__ == "__main__":
    main()
