#!/usr/bin/env python3
"""Independently reconstruct recorded losses, key chronology and expiry bounds."""

import argparse
import json
from pathlib import Path

import numpy as np
from scipy.fft import rfft
from scipy.signal import lfilter

RATE, HOP, WINDOW = 48000, 480, 960


def read_render(folder):
    metadata = json.loads((folder / "metadata.json").read_text())
    raw = np.fromfile(folder / "output.f32le", dtype="<f4").reshape(
        -1, metadata["channels"]
    )
    if (
        not np.isfinite(raw).all()
        or len(raw) != metadata["output_frames"]
        or metadata["output_frames"]
        != metadata["input_frames"] + metadata["drained_frames"]
    ):
        raise ValueError("Invalid raw native stream")
    delay = metadata["latency_samples"]
    return raw[delay : delay + metadata["input_frames"]], metadata


def loss(a, b):
    before, after = np.dot(a.astype(np.float64), a), np.dot(b.astype(np.float64), b)
    return float(10 * np.log10(before / max(after, 1e-30))) if before > 1e-12 else None


def kernel(low, high):
    samples = np.arange(-64, 65, dtype=float)
    weights = (
        2 * high / RATE * np.sinc(2 * high / RATE * samples)
        - 2 * low / RATE * np.sinc(2 * low / RATE * samples)
    ) * (0.5 + 0.5 * np.cos(np.pi * samples / 64))
    center = (low + high) / 2 if low else 0
    return weights / np.dot(weights, np.cos(2 * np.pi * center / RATE * samples))


def key_checks(folder, name):
    request = json.loads((folder / "request.json").read_text())
    dry = np.fromfile(request["input"], dtype="<f4").reshape(-1, 2)[:, 0]
    actual = np.fromfile(request["key"], dtype="<f4").reshape(-1, 2)[:, 0]
    case = folder.parent.parent
    decisions = json.loads((case / (name + "-decisions.json")).read_text())
    trace = (
        []
        if name == "relative"
        else json.loads((case / ("observe-" + name) / "trace.json").read_text())["rows"]
    )
    targets = np.ones(len(dry))
    opened = False
    lows = 0
    relative = 1.0
    cursor = 0
    latest = None
    max_fraction = 0
    max_age = 0
    window = 0.5 - 0.5 * np.cos(2 * np.pi * np.arange(WINDOW) / (WINDOW - 1))
    band = (np.arange(WINDOW // 2 + 1) * RATE / WINDOW >= 4000) & (
        np.arange(WINDOW // 2 + 1) * RATE / WINDOW <= 10000
    )
    for decision in decisions:
        first, last = decision["start"], decision["end"]
        while cursor < len(trace) and trace[cursor]["available_at_frame"] <= first:
            if trace[cursor]["new_observation"]:
                latest = trace[cursor]
            cursor += 1
        target = relative if name == "relative" else 1.0
        if name != "relative" and latest is not None:
            score, age = latest["score"], latest["source_age_ms_at_snapshot"]
            if (
                score is not None
                and age is not None
                and latest["warmup_complete"]
                and np.isfinite(score)
                and np.isfinite(age)
                and 0 <= score <= 1
                and age >= 0
            ):
                age = age + (first - latest["available_at_frame"]) * 1000 / RATE
                if age + 10 <= 150:
                    if score <= 0.2:
                        target = 0.0
                    elif score >= 0.8:
                        target = relative
                    if score <= 0.2 or score >= 0.8:
                        max_age = max(max_age, age + 10)
        if target != decision["target"]:
            raise ValueError("Key decision violates causal context/deadline")
        targets[first:last] = target
        values = np.zeros(WINDOW)
        history = dry[max(0, last - WINDOW) : last]
        values[-len(history) :] = history
        power = np.abs(rfft(values * window)) ** 2
        fraction = float(power[band].sum() / max(power.sum(), 1e-20))
        max_fraction = max(
            max_fraction,
            abs(fraction - decision["trailing_band_fraction_for_next_packet"]),
        )
        if fraction >= 0.35:
            opened = True
            lows = 0
        elif fraction <= 0.25:
            lows += 1
            if lows >= 2:
                opened = False
        relative = float(opened)
    alpha = np.exp(-1 / (0.005 * RATE))
    smooth, _ = lfilter([1 - alpha], [1, -alpha], targets, zi=[alpha])
    expected = (dry.astype(float) * smooth).astype(np.float32)
    error = float(np.max(np.abs(actual.astype(float) - expected)))
    if error > 1e-7 or max_fraction > 1e-12 or max_age > 150:
        raise ValueError("Independent key reconstruction mismatch")
    return error, max_fraction, max_age


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evaluation", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        raise ValueError("Use new analysis output")
    report = json.loads(args.evaluation.read_text())
    max_metric = 0
    max_key = 0
    max_fraction = 0
    max_age = 0
    checked = set()
    summaries = []
    filters = {"low": kernel(0, 2000), "high": kernel(4000, 12000)}
    for row in report["rows"]:
        folder = Path(row["work"])
        out, metadata = read_render(folder)
        reference, _ = read_render(Path(row["reference_work"]))
        if (
            row["method"] != "absolute"
            and (folder.parent.parent, row["method"]) not in checked
        ):
            e, f, a = key_checks(folder, row["method"])
            max_key = max(max_key, e)
            max_fraction = max(max_fraction, f)
            max_age = max(max_age, a)
            checked.add((folder.parent.parent, row["method"]))
        filtered = {
            name: (
                np.convolve(reference[:, 0], weights, "same"),
                np.convolve(out[:, 0], weights, "same"),
            )
            for name, weights in filters.items()
        }
        for region in row["regions"]:
            begin, end = (
                round(region["start_seconds"] * RATE),
                round(region["end_seconds"] * RATE),
            )
            metrics = {
                "attenuation_db": loss(reference[begin:end, 0], out[begin:end, 0])
            }
            for name, (a, b) in filtered.items():
                metrics[name + "_band_attenuation_db"] = loss(
                    a[begin:end], b[begin:end]
                )
            for name, value in metrics.items():
                old = region[name]
                if (old is None) != (value is None):
                    raise ValueError("Measurement floor differs")
                if value is not None:
                    max_metric = max(max_metric, abs(value - old))
        if metadata["requested_params"]["mode"] == "Wideband":
            active = np.abs(reference[:, 0]) > 1e-5
            gain = out[active, 0].astype(float) / reference[active, 0]
            if (
                gain.min(initial=1) < 10 ** (-6 / 20) - 2e-6
                or gain.max(initial=0) > 1 + 2e-6
            ):
                raise ValueError("Independent range check failed")
        if row["input_gain"] == 1 and row["mode"] == "Wideband":
            for region in row["regions"]:
                summaries.append(
                    {
                        "fixture": row["fixture"],
                        "region": region["label"],
                        "target_sibilance": region["target_sibilance"],
                        "method": row["method"],
                        "attenuation_db": region["attenuation_db"],
                        "high_band_attenuation_db": region["high_band_attenuation_db"],
                    }
                )
    if max_metric > 1e-5:
        raise ValueError("Independent recorded measurement differs")
    summary = {
        "comparisons": len(report["rows"]),
        "real_model_prefix_checks": len(report["model_prefix_checks"]),
        "unique_key_streams_checked": len(checked),
        "max_independent_metric_error_db": max_metric,
        "max_independent_key_error": max_key,
        "max_independent_band_fraction_error": max_fraction,
        "maximum_used_score_age_through_packet_ms": max_age,
        "wideband_development_rows": summaries,
        "qualification": "Development-only source/label coverage. Raw one-sided bin-power feature is not Parseval-normalized PCM energy. VAD is not sibilance classification; no population or realtime adoption claim.",
    }
    args.output.write_text(json.dumps(summary, indent=2) + "\n")
    print(
        json.dumps(
            {k: v for k, v in summary.items() if k != "wideband_development_rows"}
        )
    )


if __name__ == "__main__":
    main()
