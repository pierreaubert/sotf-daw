#!/usr/bin/env python3
"""Fit an offline routing suggestion and export only after native qualification."""

import argparse
import hashlib
import json
import subprocess
from pathlib import Path

from matrix_routing import fit, validate
from matrix_routing_native import qualify


def read_examples(path):
    with Path(path).open("rb") as source:
        raw = source.read(8 * 1024 * 1024 + 1)
    if len(raw) > 8 * 1024 * 1024:
        raise ValueError("Routing examples exceed8 MiB")
    data = json.loads(raw)
    validate(data)
    return data, hashlib.sha256(raw).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--examples", type=Path, required=True)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists() or args.work.exists():
        raise ValueError("Use new output/work directories")
    data, digest = read_examples(args.examples)
    renderer_digest = hashlib.sha256(args.renderer.read_bytes()).hexdigest()
    result = fit(data)
    result.update(
        examples_sha256=digest,
        renderer_sha256=renderer_digest,
        data_origin=data["data_origin"],
    )
    if result["accepted"]:
        try:
            result["native"] = qualify(data, result, args.renderer, args.work)
        except (ValueError, subprocess.TimeoutExpired) as error:
            result["reasons"].append("Native qualification failed: " + str(error))
    if hashlib.sha256(args.renderer.read_bytes()).hexdigest() != renderer_digest:
        result["reasons"].append("Renderer changed during qualification")
    if result["reasons"]:
        result["accepted"], result["parameters"] = False, None
        result.pop("routing_table", None)
    args.output.mkdir(parents=True, exist_ok=False)
    (args.output / "report.json").write_text(
        json.dumps(result, allow_nan=False, indent=2) + "\n"
    )
    if result["accepted"]:
        (args.output / "preset.json").write_text(
            json.dumps(result["parameters"], allow_nan=False, indent=2) + "\n"
        )
    print(
        json.dumps({key: result[key] for key in ["accepted", "parameters", "reasons"]})
    )
    raise SystemExit(0 if result["accepted"] else 2)


if __name__ == "__main__":
    main()
