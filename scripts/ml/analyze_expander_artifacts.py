"""Descriptive onset, envelope and residual spectral-isolation diagnostics."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from calibrate_expander_mask import reopen
from expander_mask_native import reference


def frame_energy(signal, length):
    x = np.asarray(signal, float)
    frames = len(x) // length
    return np.mean(x[: frames * length].reshape(frames, length, -1) ** 2, axis=(1, 2))


def onset_envelope(clean, component, rate):
    clean, component = np.asarray(clean, float), np.asarray(component, float)
    if (
        type(rate) is not int
        or rate <= 0
        or clean.shape != component.shape
        or clean.ndim != 2
        or not len(clean)
        or not np.isfinite(clean).all()
        or not np.isfinite(component).all()
    ):
        raise ValueError("Use matching finite time/channel arrays")
    length = max(1, round(0.010 * rate))
    original, processed = [frame_energy(x, length) for x in [clean, component]]
    if not np.any(original > 1e-20):
        return {"onsets": [], "active_envelope_error_std_db": None, "active_frames": 0}
    active = original > max(original.max() * 1e-4, 1e-20)
    error_db = 10 * np.log10(np.maximum(processed, 1e-20) / np.maximum(original, 1e-20))
    # Labels come only from original desired energy: >=6 dB rise, above -35 dB
    # of its peak. Group within 100 ms; no output-driven onset selection.
    previous = np.r_[1e-20, original[:-1]]
    candidates = np.flatnonzero(
        (original >= 4 * previous) & (original >= original.max() * 10 ** (-3.5))
    )
    selected = []
    for index in candidates:
        if not selected or (index - selected[-1]) * length / rate >= 0.100:
            selected.append(int(index))
    rows = []
    for index in selected:
        start = index * length
        end = min(len(clean), start + round(0.030 * rate))
        x, y = clean[start:end], component[start:end]
        energy = float(np.sum(x * x))
        rows.append(
            {
                "source_time_seconds": start / rate,
                "window_frames": end - start,
                "relative_rms_error": float(
                    np.sqrt(np.sum((y - x) ** 2) / max(energy, 1e-20))
                ),
                "projection_gain": float(np.sum(x * y) / max(energy, 1e-20)),
            }
        )
    return {
        "onsets": rows,
        "active_envelope_error_std_db": float(np.std(error_db[active])),
        "active_frames": int(active.sum()),
    }


def spectral_isolation(signal):
    x = np.asarray(signal, float)
    window = 0.5 - 0.5 * np.cos(2 * np.pi * np.arange(1024) / 1024)
    if len(x) < 1024 or not np.any(x):
        return None
    frames = np.lib.stride_tricks.sliding_window_view(x, 1024, axis=0)[::256]
    power = abs(np.fft.rfft(frames * window, axis=-1)) ** 2
    # Replicate edges; compare each interior bin with eight surrounding bins.
    neighbors = np.lib.stride_tricks.sliding_window_view(
        np.pad(power, [(0, 0), (0, 0), (4, 4)], mode="edge"), 9, axis=-1
    )
    average = (neighbors.sum(axis=-1) - power) / 8
    evidence = power > max(float(power.max()) * 1e-8, 1e-20)
    evidence[..., :4] = False
    evidence[..., -4:] = False
    contrast = 10 * np.log10(np.maximum(power, 1e-20) / np.maximum(average, 1e-20))
    return (
        {
            "positive_local_contrast_p99_db": float(
                np.percentile(np.maximum(contrast[evidence], 0), 99)
            ),
            "above_10db_fraction": float(np.mean(contrast[evidence] > 10)),
            "eligible_bins": int(evidence.sum()),
        }
        if evidence.any()
        else None
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    source = json.loads(args.report.read_text())
    result = {
        "source_report_sha256": hashlib.sha256(args.report.read_bytes()).hexdigest(),
        "scope": "Post-hoc descriptive diagnostics on frozen generated outputs. Clean onsets use independent 10 ms source energy rises, assessed over 30 ms. Envelope-error standard deviation is a modulation proxy; residual spectral isolation is a local contrast proxy. Neither is a validated audibility, pumping or musical-noise score. No perceptual or production admission.",
        "deployment_admitted": False,
        "cases": [],
    }
    for case in source["cases"]:
        channels, rate = case["channels"], case["sample_rate"]
        clean, noise = [
            reopen(case[name + "_file"]).reshape(-1, channels)
            for name in ["clean", "noise"]
        ]
        item = {
            "case": case["case"],
            "original_noise_spectral_isolation": spectral_isolation(noise),
            "methods": {},
        }
        for mode, data in case["methods"].items():
            metadata = data["metadata"]
            output = reopen(metadata["files"]["output"]).reshape(-1, channels)
            gains = reopen(metadata["files"]["gain_trace_output"]).reshape(
                -1, channels, 513
            )
            desired, residual = [
                reference(x, metadata["analysis_window"], gains, len(output))
                for x in [clean, noise]
            ]
            if np.max(abs(output - desired - residual)) > 2e-7:
                raise RuntimeError(
                    "Frozen conditional decomposition no longer matches output"
                )
            item["methods"][mode] = {
                "onset_envelope": onset_envelope(
                    clean, desired[1024 : 1024 + len(clean)], rate
                ),
                "residual_spectral_isolation": spectral_isolation(
                    residual[1024 : 1024 + len(noise)]
                ),
            }
        result["cases"].append(item)
    with args.output.open("x") as f:
        f.write(json.dumps(result, indent=2, allow_nan=False) + "\n")
    print(json.dumps({"cases": len(result["cases"]), "deployment_admitted": False}))


if __name__ == "__main__":
    main()
