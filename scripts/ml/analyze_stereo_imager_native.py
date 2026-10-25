"""Reconstruct retained native probes through independently discretized analog sections."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from scipy.signal import bilinear, lfilter


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        raise ValueError("Refuse to overwrite an existing analysis")
    report = json.loads(args.report.read_text())
    rows = []
    for record in report["cases"]:
        root = Path(record["work"])
        metadata = record["metadata"]
        rate = record["rate"]
        p = metadata["effective_params"]
        raw = (root / "input.f32le").read_bytes()
        output = (root / "output.f32le").read_bytes()
        if (
            hashlib.sha256(raw).hexdigest() != metadata["input_sha256"]
            or hashlib.sha256(output).hexdigest() != metadata["output_sha256"]
        ):
            raise ValueError("Retained PCM identity mismatch")
        original = np.frombuffer(raw, dtype="<f4").reshape(-1, 2).astype(float)
        x = np.pad(original, ((0, metadata["appended_zero_frames"]), (0, 0)))
        y = np.frombuffer(output, dtype="<f4").reshape(-1, 2).astype(float)
        middle = np.mean(x, axis=1)
        side = (x[:, 0] - x[:, 1]) / 2
        filtered = []
        for cutoff in [p["low_mid_freq"], p["mid_high_freq"]]:
            # Analog first-order lowpass with prewarping, then independently
            # discretized by SciPy; no native crossover implementation is used.
            omega = 2 * rate * np.tan(np.pi * cutoff / rate)
            b, a = bilinear([omega], [1, omega], fs=rate)
            filtered.append(lfilter(b, a, side))
        low, upper = filtered
        bands = np.column_stack([low, upper - low, side - upper])
        weights = [
            0 if p["mono_bass"] else p["low_width"],
            p["mid_width"],
            p["high_width"],
        ]
        wet = bands @ np.asarray(weights) * p["width"]
        final = (1 - p["mix"]) * side + p["mix"] * wet
        reference = np.column_stack([middle + final, middle - final])
        error = float(np.max(abs(reference - y)))
        mono_error = float(np.max(abs(y.mean(axis=1) - middle)))
        neutral = record["setting"] in {"neutral", "dry"}
        exact = output[: len(raw)] == raw and not np.any(y[len(original) :])
        row = {k: record[k] for k in ["rate", "source", "setting"]}
        row.update(
            maximum_waveform_error=error,
            maximum_mono_error=mono_error,
            exact_transparent_route=exact if neutral else None,
            input_peak=float(np.max(abs(original))),
            output_peak=float(np.max(abs(y))),
            tail_rms=float(np.sqrt(np.mean(y[len(original) :] ** 2))),
        )
        row["passed"] = error <= 2e-6 and mono_error <= 5e-7 and (not neutral or exact)
        rows.append(row)
    summary = {
        "cases": len(rows),
        "passed": sum(r["passed"] for r in rows),
        "maximum_waveform_error": max(r["maximum_waveform_error"] for r in rows),
        "maximum_mono_error": max(r["maximum_mono_error"] for r in rows),
        "maximum_output_peak": max(r["output_peak"] for r in rows),
        "qualification": "Static native DSP qualification; explicit zero extension is not a finite IIR tail. No learned model, setting desirability, clipping prevention or perceptual benefit claim.",
        "rows": rows,
    }
    args.output.write_text(json.dumps(summary, indent=2, allow_nan=False) + "\n")
    if summary["passed"] != summary["cases"]:
        raise ValueError("Native reference qualification failed; retain all cases")


if __name__ == "__main__":
    main()
