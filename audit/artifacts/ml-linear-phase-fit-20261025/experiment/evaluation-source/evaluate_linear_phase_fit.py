"""Frozen end-to-end learned initialization and native FIR realization probes."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import subprocess
import sys
import time
from pathlib import Path

import eq_multiband as eq
import numpy as np
from scipy.signal import sosfreqz


def write_json(path, data):
    path.write_text(json.dumps(data, indent=2, allow_nan=False) + "\n")


def parameters(peaks=(), shelves=((1000, 0), (1000, 0))):
    result = np.array(
        [math.log(20000), 0, 0] * 4 + [math.log(1000), 0] * 2, dtype=float
    )
    for index, (frequency, q, gain) in enumerate(peaks):
        result[index * 3 : index * 3 + 3] = [math.log(frequency), math.log(q), gain]
    for index, (frequency, gain) in enumerate(shelves):
        result[12 + index * 2 : 14 + index * 2] = [math.log(frequency), gain]
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--temporary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists() or args.temporary.exists():
        raise ValueError("preserve prior evaluation")
    args.output.mkdir(parents=True)
    args.temporary.mkdir(parents=True)
    weights = args.model / "weights.json"
    expected = "671fbe820883249c00c228d5497d4585a1836afba62f6e2f86d0e39ecbdf3015"
    if eq.sha(weights) != expected:
        raise ValueError("model identity mismatch")
    cases = [
        ("flat-44100", 44100, parameters(), "fit"),
        ("peak-48000", 48000, parameters([(1500, 0.9, 4.5)]), "fit"),
        ("bass-96000", 96000, parameters([(120, 1.0, 6.0)]), "fit"),
        (
            "six-band-44100",
            44100,
            parameters(
                [(250, 0.75, 3), (1700, 1.2, -4), (6500, 2, 2)],
                [(180, 1.5), (9000, -2)],
            ),
            "fit",
        ),
        (
            "rate-shift-192000",
            192000,
            parameters([(3000, 0.7, -5)], [(1000, 0), (10000, 3)]),
            "fit",
        ),
        ("jagged-rejection", 48000, parameters(), "jagged"),
        ("native-tap-limit", 96000, parameters([(30, 8, 12)]), "proposal"),
        ("bad-model-identity", 48000, parameters([(1500, 0.9, 4.5)]), "bad-hash"),
    ]
    protocol = {
        "model_sha256": expected,
        "renderer_sha256": eq.sha(args.renderer),
        "cases": [
            {"name": name, "rate": rate, "source_parameters": p.tolist(), "mode": mode}
            for name, rate, p, mode in cases
        ],
        "source_sha256": {
            name: eq.sha(Path(__file__).parent / name)
            for name in [
                Path(__file__).name,
                "fit_linear_phase_eq.py",
                "eq_multiband.py",
            ]
        },
        "selection": "Fixed cases before evaluation; no training or policy selection. Native tap-limit and bad-identity are required rejection probes. Previous learned-versus-classical negative adoption remains in force.",
        "target_generation": "Independent scipy.signal.sosfreqz using RBJ sections; FIR validation uses analog/bilinear reference and SciPy FIR DTFT. Neither target alignment nor gain fitting.",
        "max_solver_evaluations_per_seed": 200,
        "manual_baseline_taps": 1024,
        "native_accuracy_mode": True,
    }
    write_json(args.output / "protocol.json", protocol)
    snapshot = args.output / "evaluation-source"
    snapshot.mkdir()
    for name in protocol["source_sha256"]:
        (snapshot / name).write_bytes((Path(__file__).parent / name).read_bytes())
    data = json.loads((args.model / "data-manifest.json").read_text())
    excluded = {identity for ids in data["split_ids"].values() for identity in ids}
    excluded.update(
        json.loads((args.model / "final-holdout.json").read_text())["fresh_holdout_ids"]
    )
    rows = []
    for index, (name, rate, p, mode) in enumerate(cases):
        source_id = hashlib.sha256(p.astype("<f8").tobytes()).hexdigest()
        if source_id in excluded:
            raise ValueError("declared source overlaps model data")
        directory = args.temporary / name
        directory.mkdir()
        frequencies = np.unique(np.r_[eq.GRID, np.exp(p[:12:3]), np.exp(p[12::2])])
        coefficients = eq.coefficients(p, np.asarray(rate))
        _, response = sosfreqz(
            coefficients / coefficients[:, 3, None], worN=frequencies, fs=rate
        )
        target = 20 * np.log10(np.maximum(np.abs(response), 1e-30))
        if mode == "jagged":
            target = np.where(np.arange(len(frequencies)) % 2, 3.0, -3.0)
        target_path = args.output / (name + "-target.json")
        write_json(
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
            "--max-evaluations",
            "200",
        ]
        if mode == "proposal":
            path = directory / "input-proposal.json"
            write_json(
                path,
                {
                    "accepted": True,
                    "target_sha256": eq.sha(target_path),
                    "method": "declared-native-limit-proposal",
                    "parameters": eq.proposal_parameters(p),
                },
            )
            command += ["--proposal", str(path)]
        elif name != "flat-44100":
            command += [
                "--weights",
                str(weights),
                "--weights-sha256",
                "0" * 64 if mode == "bad-hash" else expected,
            ]
        start = time.monotonic()
        completed = subprocess.run(
            command, check=False, capture_output=True, text=True, timeout=720
        )
        elapsed = time.monotonic() - start
        (directory / "stdout.txt").write_text(completed.stdout)
        (directory / "stderr.txt").write_text(completed.stderr)
        report_path = directory / "output/report.json"
        if not report_path.exists() or completed.returncode not in [0, 2]:
            raise ValueError(
                f"unexpected pipeline failure: {name}: {completed.stderr[-2000:]}"
            )
        report = json.loads(report_path.read_text())
        if report["accepted"] != (completed.returncode == 0):
            raise ValueError("status/acceptance mismatch")
        if mode in ["jagged", "proposal", "bad-hash"] and report["accepted"]:
            raise ValueError("declared rejection probe exported a preset")
        if not report["accepted"] and (
            report["parameters"] is not None
            or (directory / "output/preset.json").exists()
            or (directory / "output/fir.wav").exists()
        ):
            raise ValueError("rejected case leaked an applicable export")
        row = {
            "case": name,
            "index": index,
            "source_parameter_identity": source_id,
            "target_sha256": eq.sha(target_path),
            "command": command,
            "exit_code": completed.returncode,
            "elapsed_seconds_including_import_fit_native_validation": elapsed,
            "directory": str(directory.resolve()),
            "report": report,
        }
        write_json(args.output / f"case-{index:03d}.json", row)
        rows.append(row)
        print(
            json.dumps(
                {
                    "case": name,
                    "accepted": report["accepted"],
                    "fit_method": report.get("fit_method"),
                    "taps": report.get("native", {}).get("selected_taps"),
                    "error": report.get("error"),
                }
            ),
            flush=True,
        )
    summary = {
        "cases": len(rows),
        "accepted": sum(r["report"]["accepted"] for r in rows),
        "rows": [
            {
                "case": r["case"],
                "accepted": r["report"]["accepted"],
                "fit_method": r["report"].get("fit_method"),
                "latency_samples": r["report"].get("latency_samples"),
                "added_latency_samples": r["report"].get("added_latency_samples"),
                "target_rms_db": r["report"].get("checks", {}).get("target_rms_db"),
            }
            for r in rows
        ],
        "no_automatic_apply": True,
        "no_training_or_model_adoption_change": True,
    }
    write_json(args.output / "summary.json", summary)
    print(json.dumps(summary), flush=True)


if __name__ == "__main__":
    main()
