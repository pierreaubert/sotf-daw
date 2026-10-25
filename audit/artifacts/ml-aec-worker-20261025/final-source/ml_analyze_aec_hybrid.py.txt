"""Recompute fresh native hybrid scores and verify downstream-only processing."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np


def power(samples):
    return float(np.sum(samples * samples, dtype=np.float64))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evaluation", type=Path, required=True)
    parser.add_argument("--temporary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    protocol = json.loads((args.evaluation / "protocol.json").read_text())
    rows = [
        json.loads(p.read_text()) for p in sorted(args.evaluation.glob("case-*.json"))
    ]
    if len(rows) != protocol["cases"] or len({r["index"] for r in rows}) != len(rows):
        raise ValueError("incomplete or repeated final cases")
    comparisons, unchanged_near = [], 0
    for row in rows:
        index, meta = row["index"], row["provenance"]
        rate, frames = meta["rate"], meta["frames"]
        arrays = {}
        for name in ["input", "near", "echo"]:
            raw = (
                args.temporary / f"source_{index:03d}" / (name + ".f32le")
            ).read_bytes()
            if hashlib.sha256(raw).hexdigest() != meta[name + "_sha256"]:
                raise ValueError("source channel changed")
            arrays[name] = np.frombuffer(raw, dtype="<f4").astype(float)
        audio = {}
        for method in ["classical", "hybrid"]:
            native = row["native"][method]
            raw = (Path(native["directory"]) / (method + ".f32le")).read_bytes()
            if hashlib.sha256(raw).hexdigest() != native[method]["sha256"]:
                raise ValueError("native audio changed")
            audio[method] = np.frombuffer(raw, dtype="<f4").astype(float)
            if len(audio[method]) != frames or native["latency_samples"] != 256:
                raise ValueError("latency or geometry mismatch")
        for name in ["features", "targets", "adaptive", "error_spectra"]:
            if (
                row["native"]["classical"][name]["sha256"]
                != row["native"]["hybrid"][name]["sha256"]
            ):
                raise ValueError("learned stage changed adaptation or analysis")
        diagnostic = row["native"]["hybrid"]["residual"]
        if (
            diagnostic["availability"] != "prepared"
            or not diagnostic["offline"]
            or diagnostic["native_blocks"] != frames // 256
            or diagnostic["dropped_blocks"] != 0
            or diagnostic["applied_blocks"] + diagnostic["fallback_blocks"]
            != diagnostic["native_blocks"]
            or not 0
            < diagnostic["accepted_observations"]
            <= diagnostic["submitted_blocks"]
        ):
            raise ValueError("model not exercised or block accounting mismatch")
        intervals = {
            "steady": (5 * rate, frames),
            "onset": (3 * rate, 3 * rate + rate // 8),
        }
        if meta["scene"] in ["double_talk", "quiet_double_talk"]:
            intervals["double_talk"] = (13 * rate // 4, 17 * rate // 4)
        elif meta["scene"] == "near_only":
            intervals = {"near_only": (0, frames)}
            if (
                not np.array_equal(audio["classical"], audio["hybrid"])
                or diagnostic["applied_blocks"] != 0
            ):
                raise ValueError("near-only reference-silent fallback changed audio")
            unchanged_near += 1
        elif meta["scene"] == "near_after_echo":
            intervals["near_after_echo"] = (13 * rate // 4, frames)
        for phase, (first, last) in intervals.items():
            near, echo = arrays["near"][first:last], arrays["echo"][first:last]
            near_present = power(near) > 1e-20
            scores = {}
            for method in ["classical", "hybrid"]:
                output = audio[method][first:last]
                target, error = (
                    (near, output - near) if near_present else (echo, output)
                )
                scores[method] = float(
                    10 * np.log10(max(power(target), 1e-30) / max(power(error), 1e-30))
                )
                if not np.isclose(
                    scores[method], row["quality"][phase][method], atol=1e-9
                ):
                    raise ValueError("independent native quality score disagrees")
            comparisons.append(
                {
                    "case": index,
                    "scene": meta["scene"],
                    "rate": rate,
                    "phase": phase,
                    "metric": "near_snr_db" if near_present else "erle_db",
                    **scores,
                    "delta_db": scores["hybrid"] - scores["classical"],
                }
            )
    echo = [
        x for x in comparisons if x["phase"] == "steady" and x["metric"] == "erle_db"
    ]
    near = [
        x
        for x in comparisons
        if x["metric"] == "near_snr_db"
        and x["phase"] in ["onset", "double_talk", "near_after_echo"]
    ]
    result = {
        "cases": len(rows),
        "rates": protocol["rates"],
        "unchanged_near_only_cases": unchanged_near,
        "steady_echo_cases": len(echo),
        "steady_echo_delta_range_db": [
            min(x["delta_db"] for x in echo),
            max(x["delta_db"] for x in echo),
        ],
        "steady_echo_improvements_above_1db": sum(x["delta_db"] > 1 for x in echo),
        "worst_near_snr_delta_db": min(x["delta_db"] for x in near),
        "comparisons": comparisons,
        "scope": "Synthetic offline pipeline only. Preserve all negative deltas; numerical-noise differences are not substantive improvements. Natural/device/realtime quality acceptance remains open.",
    }
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({k: v for k, v in result.items() if k != "comparisons"}))


if __name__ == "__main__":
    main()
