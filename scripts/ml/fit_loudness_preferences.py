#!/usr/bin/env python3
"""Fit an offline manual preference-shape proposal; abstain without a preset."""

import argparse
import hashlib
import json
import subprocess
from pathlib import Path

from loudness_preference_native import qualify
from loudness_preferences import fit


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--session", type=Path, required=True)
    parser.add_argument("--level-db-spl", type=float, required=True)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists() or args.work.exists():
        raise ValueError("Use new output and work directories")
    with args.session.open("rb") as source:
        raw = source.read(524289)
    if len(raw) > 524288:
        raise ValueError("Session exceeds512KiB")
    data = json.loads(raw)
    report = fit(data, args.level_db_spl)
    report.update(
        context=data["context"],
        calibration=data["calibration"],
        session_sha256=hashlib.sha256(raw).hexdigest(),
        audition_level_matching="External trial matching is not exported. Re-match this new proposed shape before assessing preference.",
    )
    if report["accepted"]:
        try:
            report["native"] = qualify(
                report["parameters"],
                data["context"]["sample_rate"],
                args.renderer,
                args.work,
            )
        except (
            ValueError,
            OSError,
            KeyError,
            TypeError,
            subprocess.TimeoutExpired,
        ) as error:
            report.update(accepted=False, parameters=None)
            report["reasons"].append(str(error))
    args.output.mkdir(parents=True, exist_ok=False)
    (args.output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    if report["accepted"]:
        (args.output / "preset.json").write_text(
            json.dumps(report["parameters"], indent=2, allow_nan=False) + "\n"
        )
    print(json.dumps({"accepted": report["accepted"], "reasons": report["reasons"]}))
    if not report["accepted"]:
        raise SystemExit(2)


if __name__ == "__main__":
    main()
