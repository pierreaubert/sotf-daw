#!/usr/bin/env python3
"""Verify manual routing exports, native failure, rejection and overwrite protection."""

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sessions", type=Path, required=True)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.work.exists() or args.output.exists():
        raise ValueError("Use new verification paths")
    args.work.mkdir(parents=True)
    bad = json.loads((args.sessions / "linear-2x2-examples.json").read_text())
    bad["input_channel_map"] = [0, 0]
    invalid = args.work / "invalid.json"
    invalid.write_text(json.dumps(bad) + "\n")
    rows = []
    for name, examples, renderer, expected_code, export in [
        ("linear", args.sessions / "linear-2x2-examples.json", args.renderer, 0, True),
        (
            "ambiguous",
            args.sessions / "collinear-examples.json",
            args.renderer,
            2,
            False,
        ),
        (
            "overload",
            args.sessions / "coherent-overload-examples.json",
            args.renderer,
            2,
            False,
        ),
        ("invalid", invalid, args.renderer, 1, False),
        (
            "native-failure",
            args.sessions / "linear-2x2-examples.json",
            Path("/usr/bin/false"),
            2,
            False,
        ),
    ]:
        output = args.work / (name + "-output")
        command = [
            sys.executable,
            str(Path(__file__).with_name("fit_matrix_routing.py")),
            "--examples",
            str(examples),
            "--renderer",
            str(renderer),
            "--work",
            str(args.work / (name + "-native")),
            "--output",
            str(output),
        ]
        process = subprocess.run(
            command, check=False, capture_output=True, text=True, timeout=120
        )
        if (
            process.returncode != expected_code
            or (output / "preset.json").exists() != export
        ):
            raise ValueError("Unexpected routing CLI result: " + name + process.stderr)
        report = (
            json.loads((output / "report.json").read_text())
            if output.exists()
            else None
        )
        if export and (
            not report["accepted"]
            or json.loads((output / "preset.json").read_text()) != report["parameters"]
            or not report["routing_table"]
        ):
            raise ValueError("Export differs from native-qualified reviewable routing")
        if (
            not export
            and report is not None
            and (report["parameters"] is not None or report["accepted"])
        ):
            raise ValueError("Unqualified routing was exposed as an export")
        rows.append(
            {
                "name": name,
                "exit_code": process.returncode,
                "exported": export,
                "stdout": process.stdout,
                "stderr": process.stderr,
                "report": report,
            }
        )
        if name == "linear":
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
                raise ValueError("Existing routing outputs changed")
            rows.append(
                {
                    "name": "overwrite-rejected",
                    "exit_code": repeated.returncode,
                    "output_hashes_unchanged": True,
                }
            )
    args.output.write_text(
        json.dumps({"all_expected_results": True, "rows": rows}, indent=2) + "\n"
    )
    print(json.dumps({"checks": len(rows), "all_expected_results": True}))


if __name__ == "__main__":
    main()
