#!/usr/bin/env python3
"""Independent recurrence/DFT analysis of explicit native Hiss capture outputs."""

import argparse
import hashlib
import json
import math
from pathlib import Path

import numpy as np


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def high_power(signal, start, end):
    alpha = 1 - math.exp(-2 * math.pi * 4000 / 48000)
    low = 0.0
    energy = []
    for i, value in enumerate(signal[:end, 0]):
        x = float(value)
        low = alpha * x + (1 - alpha) * low
        if i >= start:
            energy.append((x - low) ** 2)
    return math.fsum(energy) / (end - start)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--evaluation", type=Path, required=True)
    p.add_argument("--initial", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True)
    args = p.parse_args()
    data = json.loads(args.evaluation.read_text())
    profiles = {}
    max_metric = max_floor = max_dft = 0.0
    verified = 0
    hashes = {}
    for row in data["cases"]:
        folder = Path(row["metadata"]).parent
        meta = json.loads(Path(row["metadata"]).read_text())
        raw = (folder / "input.f32le").read_bytes()
        out = (folder / "output.f32le").read_bytes()
        assert sha(raw) == meta["input_sha256"] and sha(out) == meta["output_sha256"]
        x = np.frombuffer(raw, dtype="<f4").reshape(-1, 1)
        y = np.frombuffer(out, dtype="<f4").reshape(-1, 1)[meta["latency_samples"] :]
        assert len(y) == meta["aligned_output_frames"] == row["aligned_output_frames"]
        start, end = (int(48000 * t) for t in data["region_seconds"])
        high = 10 * math.log10(high_power(x, start, end) / high_power(y, start, end))
        power = lambda v, start=start, end=end: (
            math.fsum(float(z) ** 2 for z in v[start:end, 0]) / (end - start)
        )
        full = 10 * math.log10(power(x) / power(y))
        max_metric = max(
            max_metric,
            abs(high - row["high_band_attenuation_db"]),
            abs(full - row["full_band_attenuation_db"]),
        )
        if row["capture"] != "none":
            captured = (folder / "capture.f32le").read_bytes()
            assert sha(captured) == meta["capture_sha256"]
            profile = meta["effective_params"]["captured_profile"]
            assert (
                profile["frames_analyzed"] == 48000
                and profile["sample_rate"] == 48000
                and profile["channels"] == 1
            )
            if row["capture"] in profiles:
                assert profile == profiles[row["capture"]]
            else:
                profiles[row["capture"]] = profile
                signal = np.frombuffer(captured, dtype="<f4").reshape(-1, 1)
                floor = 10 * math.log10(high_power(signal, 0, 48000))
                max_floor = max(
                    max_floor, abs(floor - profile["floor_db_per_channel"][0])
                )
                bins = np.array([0, 17, 85, 256, 512])
                n = np.arange(1024)
                basis = (0.5 - 0.5 * np.cos(2 * np.pi * n / 1024))[None, :] * np.exp(
                    -2j * np.pi * bins[:, None] * n / 1024
                )
                windows = np.lib.stride_tricks.sliding_window_view(
                    signal[:, 0].astype(float), 1024
                )[::256]
                expected = np.mean(abs(windows @ basis.T) ** 2, axis=0)
                stored = np.asarray(profile["spectral"]["power_per_channel_bin"])
                max_dft = max(
                    max_dft,
                    float(
                        np.max(abs(expected - stored[bins]))
                        / max(float(stored.max()), 1e-20)
                    ),
                )
                assert profile["spectral"]["hops_analyzed"] == len(windows)
        else:
            assert meta["effective_params"]["captured_profile"] is None
        hashes[
            (row["threshold_db"], row["spectral"], row["capture"], row["programme"])
        ] = meta["output_sha256"]
        verified += 1
    initial = json.loads(args.initial.read_text())
    same = 0
    for row in initial["cases"]:
        m = json.loads(Path(row["metadata"]).read_text())
        assert (
            hashes[(-60.0, row["spectral"], row["capture"], row["programme"])]
            == m["output_sha256"]
        )
        same += 1
    assert max_metric < 1e-10 and max_floor < 0.001 and max_dft < 2e-5
    result = {
        "native_outputs_verified": verified,
        "unique_profiles_verified": len(profiles),
        "selected_direct_dft_bins": [0, 17, 85, 256, 512],
        "max_attenuation_metric_difference_db": max_metric,
        "max_profile_floor_difference_db": max_floor,
        "max_selected_dft_power_difference_relative_to_profile_peak": max_dft,
        "initial_minimum_threshold_outputs_bit_identical": same,
        "profile_floor_inflation_from_speech_db": profiles["speech_contaminated"][
            "floor_db_per_channel"
        ][0]
        - profiles["noise_only"]["floor_db_per_channel"][0],
        "scope": "Recorded single-speaker development consequence only; no classifier, safe-interval or population quality claim.",
    }
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
