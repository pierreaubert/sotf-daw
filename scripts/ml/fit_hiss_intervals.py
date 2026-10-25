#!/usr/bin/env python3
"""Fit an optional source-labelled Hiss interval review model; no capture."""

import argparse
import json
from pathlib import Path

from hiss_intervals import digest, fit


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--session", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True)
    args = p.parse_args()
    if args.output.exists():
        raise ValueError("Use a new output directory")
    with args.session.open("rb") as f:
        raw = f.read(1048577)
    if len(raw) > 1048576:
        raise ValueError("Session exceeds1 MiB")
    result = fit(json.loads(raw))
    model = result.pop("model")
    result["session_sha256"] = digest(raw)
    args.output.mkdir(parents=True, exist_ok=False)
    if model is not None:
        payload = (json.dumps(model, indent=2, allow_nan=False) + "\n").encode()
        (args.output / "model.json").write_bytes(payload)
        result["model_sha256"] = digest(payload)
    (args.output / "report.json").write_text(
        json.dumps(result, indent=2, allow_nan=False) + "\n"
    )
    print(
        json.dumps(
            {
                "accepted": result["accepted"],
                "reasons": result["reasons"],
                "automatic_capture": False,
            }
        )
    )
    if not result["accepted"]:
        raise SystemExit(2)


if __name__ == "__main__":
    main()
