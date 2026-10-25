"""Independently validate all realized candidates and the FIR-aware selection rule."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

import numpy as np
from analyze_linear_phase_fit import digest, dtft, reference
from scipy.fft import rfft


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--temporary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists() or args.temporary.exists():
        raise ValueError("preserve prior analysis")
    args.temporary.mkdir(parents=True)
    selected_report = args.temporary / "selected.json"
    subprocess.run(
        [
            sys.executable,
            str(Path(__file__).with_name("analyze_linear_phase_fit.py")),
            "--evidence",
            str(args.evidence),
            "--renderer",
            str(args.renderer),
            "--temporary",
            str(args.temporary / "native"),
            "--output",
            str(selected_report),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    protocol = json.loads((args.evidence / "protocol.json").read_text())
    records, chosen_rows = [], []
    for index in range(len(protocol["cases"])):
        row = json.loads((args.evidence / f"case-{index:03d}.json").read_text())
        target = json.loads(
            (args.evidence / (row["case"] + "-target.json")).read_text()
        )
        frequencies, rate = np.asarray(target["frequencies_hz"]), target["sample_rate"]
        eligible = []
        for candidate in row["report"].get("native_candidates", []):
            if "request_path" not in candidate:
                if candidate["accepted"]:
                    raise ValueError("qualified candidate lacks provenance")
                continue
            request_path = Path(candidate["request_path"])
            if digest(request_path) != candidate["request_sha256"]:
                raise ValueError("candidate request changed")
            request = json.loads(request_path.read_text())
            ideal = np.abs(reference(request["params"], frequencies, rate))
            iir_error = 20 * np.log10(np.maximum(ideal, 1e-30)) - target["target_db"]
            if (
                np.sqrt(np.mean(iir_error**2)) > 0.1 + 1e-7
                or np.max(np.abs(iir_error)) > 0.5 + 1e-7
            ):
                raise ValueError("candidate was not individually IIR-qualified")
            native = candidate["native"]
            if native["status"] != "ok":
                if candidate["accepted"]:
                    raise ValueError("native rejection marked eligible")
                records.append(
                    {
                        "case": row["case"],
                        "index": candidate["index"],
                        "method": candidate["method"],
                        "native_rejected": True,
                    }
                )
                continue
            path = Path(native["directory"]) / "fir.f32le"
            if digest(path) != candidate["checks"]["fir_sha256"]:
                raise ValueError("candidate FIR changed")
            fir = np.fromfile(path, dtype="<f4").astype(float)
            h = dtft(fir, frequencies, rate)
            error = 20 * np.log10(np.maximum(np.abs(h), 1e-30)) - target["target_db"]
            rms, maximum = (
                float(np.sqrt(np.mean(error**2))),
                float(np.max(np.abs(error))),
            )
            if (
                abs(rms - candidate["checks"]["target_rms_db"]) > 1e-7
                or abs(maximum - candidate["checks"]["target_max_db"]) > 1e-7
            ):
                raise ValueError("candidate target metrics differ")
            length = 262144
            grid = np.arange(length // 2 + 1) * rate / length
            keep = (grid >= 20) & (grid <= 20000)
            grid = grid[keep]
            spectrum = rfft(fir, n=length)[keep]
            ideal = np.abs(reference(request["params"], grid, rate))
            deviation = float(
                np.max(
                    np.abs(
                        20
                        * np.log10(
                            np.maximum(np.abs(spectrum), 1e-30)
                            / np.maximum(ideal, 1e-30)
                        )
                    )
                )
            )
            phase = float(
                np.max(
                    np.abs(
                        np.angle(
                            spectrum * np.exp(2j * np.pi * grid * len(fir) / 2 / rate),
                            deg=True,
                        )
                    )
                )
            )
            if candidate["accepted"]:
                if rms > 0.1 or maximum > 0.5 or deviation > 0.0501 or phase > 0.001:
                    raise ValueError(
                        "eligible candidate failed independent qualification"
                    )
                eligible.append(candidate)
            records.append(
                {
                    "case": row["case"],
                    "index": candidate["index"],
                    "method": candidate["method"],
                    "accepted": candidate["accepted"],
                    "taps": len(fir),
                    "independent_target_rms_db": rms,
                    "independent_dense_deviation_db": deviation,
                }
            )
        if row["report"]["accepted"]:
            chosen = min(
                eligible,
                key=lambda c: (
                    c["native"]["selected_taps"],
                    c["checks"]["target_rms_db"],
                    c["checks"]["target_max_db"],
                    c["index"],
                ),
            )
            if (
                chosen["index"] != row["report"]["selected_candidate_index"]
                or chosen["method"] != row["report"]["fit_method"]
            ):
                raise ValueError("selection did not follow declared policy")
            chosen_rows.append(
                {
                    "case": row["case"],
                    "iir_method": row["report"]["iir_selected_method"],
                    "fir_method": chosen["method"],
                    "taps": chosen["native"]["selected_taps"],
                    "eligible_candidates": len(eligible),
                }
            )
        elif eligible:
            raise ValueError("qualified alternatives were discarded")
    result = {
        "cases": len(protocol["cases"]),
        "candidate_checks": len(records),
        "selected": chosen_rows,
        "candidate_records": records,
        "saved_preset_reopen_validation": json.loads(selected_report.read_text()),
        "scope": "Independent direct DTFT, Tustin/FFT candidate checks, selection ranking, and native preset reopening. Functional evidence only; no general learned superiority.",
    }
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(
        json.dumps(
            {
                k: v
                for k, v in result.items()
                if k not in ["candidate_records", "saved_preset_reopen_validation"]
            }
        )
    )


if __name__ == "__main__":
    main()
