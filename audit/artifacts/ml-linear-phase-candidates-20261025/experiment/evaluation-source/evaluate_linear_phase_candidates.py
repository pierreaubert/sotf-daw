"""Fixed fresh FIR-aware candidate probes and legacy EQ output compatibility."""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
import time
from pathlib import Path

import eq_multiband as eq
import numpy as np
from evaluate_linear_phase_fit import parameters
from scipy.signal import sosfreqz


def write(path, data):
    path.write_text(json.dumps(data, indent=2, allow_nan=False) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--temporary", type=Path, required=True)
    parser.add_argument("--previous-evidence", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists() or args.temporary.exists():
        raise ValueError("preserve existing evidence")
    args.output.mkdir(parents=True)
    args.temporary.mkdir(parents=True)
    cases = [
        ("peak-1800-48k", 48000, parameters([(1800, 0.8, 5)]), "fit"),
        ("bass-90-96k", 96000, parameters([(90, 0.9, 5)]), "fit"),
        (
            "shelves-44100",
            44100,
            parameters([(400, 1, -2)], [(160, 2), (8500, -3)]),
            "fit",
        ),
        (
            "rate-shift-192k",
            192000,
            parameters([(4200, 1.1, -4)], [(220, 1), (1000, 0)]),
            "fit",
        ),
        ("jagged-rejected", 48000, parameters(), "jagged"),
        ("native-failure-isolated", 96000, parameters(), "alternatives"),
    ]
    weights = args.model / "weights.json"
    expected = "671fbe820883249c00c228d5497d4585a1836afba62f6e2f86d0e39ecbdf3015"
    if eq.sha(weights) != expected:
        raise ValueError("model identity mismatch")
    protocol = {
        "cases": [
            {"name": n, "rate": r, "source_parameters": p.tolist(), "mode": m}
            for n, r, p, m in cases
        ],
        "model_sha256": expected,
        "renderer_sha256": eq.sha(args.renderer),
        "selection_policy": "All individually qualified IIR seeds undergo native and original-target FIR checks. Choose shortest actual support, then final target RMS, target max, input order. No model/threshold tuning from this probe.",
        "source_sha256": {
            n: eq.sha(Path(__file__).parent / n)
            for n in [
                Path(__file__).name,
                "eq_multiband.py",
                "fit_linear_phase_eq.py",
                "evaluate_linear_phase_fit.py",
            ]
        },
        "scope": "Fixed synthetic functional probes, not statistical quality or speed acceptance. Required alternative/failure fixture is explicit.",
    }
    write(args.output / "protocol.json", protocol)
    snapshot = args.output / "evaluation-source"
    snapshot.mkdir()
    for name in protocol["source_sha256"]:
        (snapshot / name).write_bytes((Path(__file__).parent / name).read_bytes())
    data = json.loads((args.model / "data-manifest.json").read_text())
    excluded = {item for values in data["split_ids"].values() for item in values}
    excluded.update(
        json.loads((args.model / "final-holdout.json").read_text())["fresh_holdout_ids"]
    )
    excluded.update(
        json.loads(path.read_text())["source_parameter_identity"]
        for path in args.previous_evidence.glob("case-*.json")
    )
    rows = []
    for index, (name, rate, encoded, mode) in enumerate(cases):
        if (
            mode == "fit"
            and hashlib.sha256(encoded.astype("<f8").tobytes()).hexdigest() in excluded
        ):
            raise ValueError("new fit identity overlaps prior training/evaluation")
        directory = args.temporary / name
        directory.mkdir()
        frequencies = np.unique(
            np.r_[eq.GRID, np.exp(encoded[:12:3]), np.exp(encoded[12::2])]
        )
        sections = eq.coefficients(encoded, np.asarray(rate))
        sections /= sections[:, 3, None]
        target = 20 * np.log10(
            np.maximum(np.abs(sosfreqz(sections, worN=frequencies, fs=rate)[1]), 1e-30)
        )
        if mode == "jagged":
            target = np.where(np.arange(len(target)) % 2, -3.0, 3.0)
        target_path = args.output / (name + "-target.json")
        write(
            target_path,
            {
                "sample_rate": rate,
                "frequencies_hz": frequencies.tolist(),
                "target_db": target.tolist(),
            },
        )
        command = [
            sys.executable,
            str(Path(__file__).with_name("fit_linear_phase_eq.py")),
            "--target",
            str(target_path),
            "--renderer",
            str(args.renderer),
            "--output",
            str(directory / "output"),
            "--work-dir",
            str(directory / "work"),
        ]
        if mode == "alternatives":
            header = {"accepted": True, "target_sha256": eq.sha(target_path)}
            identity = {
                **header,
                "method": "identity",
                "parameters": {"filters": [], "auto_gain": {"enabled": False}},
            }
            narrow = {
                **header,
                "method": "narrow-residue",
                "parameters": {
                    "filters": [
                        {
                            "filter_type": "peak",
                            "freq": 30,
                            "q": 8,
                            "db_gain": 0.45,
                            "order": 2,
                        }
                    ],
                    "auto_gain": {"enabled": False},
                },
            }
            proposal = {**identity, "qualified_proposals": [narrow, identity]}
            path = directory / "proposal.json"
            write(path, proposal)
            command += ["--proposal", str(path)]
        else:
            command += ["--weights", str(weights), "--weights-sha256", expected]
        start = time.monotonic()
        result = subprocess.run(
            command, check=False, capture_output=True, text=True, timeout=1000
        )
        report_path = directory / "output/report.json"
        if result.returncode not in [0, 2] or not report_path.exists():
            raise ValueError(f"pipeline failed: {name}: {result.stderr[-2000:]}")
        report = json.loads(report_path.read_text())
        if report["accepted"] != (result.returncode == 0):
            raise ValueError("acceptance/status mismatch")
        if not report["accepted"] and (
            (directory / "output/preset.json").exists()
            or (directory / "output/fir.wav").exists()
            or report["parameters"] is not None
        ):
            raise ValueError("rejected export leaked")
        if mode == "alternatives" and (
            not report["accepted"]
            or report["native_candidates"][0]["native"]["status"] != "rejected"
            or report["fit_method"] != "identity"
        ):
            raise ValueError("earlier native failure prevented qualified alternative")
        if mode == "jagged" and report["accepted"]:
            raise ValueError("jagged target qualified")
        row = {
            "case": name,
            "index": index,
            "command": command,
            "exit_code": result.returncode,
            "elapsed_seconds": time.monotonic() - start,
            "directory": str(directory.resolve()),
            "target_sha256": eq.sha(target_path),
            "source_parameter_identity": hashlib.sha256(
                encoded.astype("<f8").tobytes()
            ).hexdigest(),
            "report": report,
        }
        write(args.output / f"case-{index:03d}.json", row)
        rows.append(row)
        print(
            json.dumps(
                {
                    "case": name,
                    "accepted": report["accepted"],
                    "iir_selected": report.get("iir_selected_method"),
                    "fir_selected": report.get("fit_method"),
                    "candidate_count": len(report.get("native_candidates", [])),
                    "taps": report.get("native", {}).get("selected_taps"),
                }
            ),
            flush=True,
        )
    # Identical target, fitter, weights and budget: new parameter payload must
    # not alter legacy selection or expose extra fields without explicit opt-in.
    first = rows[0]
    legacy = args.temporary / "legacy-proposal.json"
    command = [
        sys.executable,
        str(Path(__file__).with_name("eq_multiband.py")),
        "fit",
        "--weights",
        str(weights),
        "--target",
        str(args.output / (first["case"] + "-target.json")),
        "--output",
        str(legacy),
        "--max-evaluations",
        "200",
    ]
    result = subprocess.run(
        command, check=False, capture_output=True, text=True, timeout=600
    )
    default = json.loads(legacy.read_text())
    extended = json.loads((Path(first["directory"]) / "work/fit.json").read_text())
    if result.returncode != 0 or "qualified_proposals" in default:
        raise ValueError("default EQ output contract changed")
    for key in [
        "accepted",
        "method",
        "parameters",
        "sampled_rms_db",
        "sampled_max_db",
        "weights_sha256",
    ]:
        if default[key] != extended[key]:
            raise ValueError("opt-in payload changed existing IIR selection")
    write(
        args.output / "legacy-compatibility.json",
        {
            "command": command,
            "exit_code": result.returncode,
            "same_original_selection_controls_errors": True,
            "default_has_no_extra_proposals": True,
            "extended_qualified_candidates": len(extended["qualified_proposals"]),
        },
    )
    write(
        args.output / "summary.json",
        {
            "cases": len(rows),
            "accepted": sum(r["report"]["accepted"] for r in rows),
            "candidate_realizations": sum(
                len(r["report"].get("native_candidates", [])) for r in rows
            ),
            "rows": [
                {
                    "case": r["case"],
                    "accepted": r["report"]["accepted"],
                    "iir_method": r["report"].get("iir_selected_method"),
                    "fir_method": r["report"].get("fit_method"),
                    "taps": r["report"].get("native", {}).get("selected_taps"),
                }
                for r in rows
            ],
        },
    )


if __name__ == "__main__":
    main()
