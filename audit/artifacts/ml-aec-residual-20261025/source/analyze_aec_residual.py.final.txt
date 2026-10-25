"""Independently score frozen AEC outputs and conditional near-end attenuation."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import numpy as np
from aec_residual_model import BANDS, FEATURES, ResidualModel, apply_masks
from train_aec_residual import load_row, load_source


def snr(target, error):
    signal_power = float(np.dot(target, target))
    error_power = float(np.dot(error, error))
    return 10 * np.log10(max(signal_power, 1e-30) / max(error_power, 1e-30))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--model-sha256", required=True)
    parser.add_argument("--reference-report", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    model = ResidualModel(args.model, args.model_sha256)
    manifest = json.loads((args.data / "manifest.json").read_text())
    reference = json.loads(args.reference_report.read_text())
    reports = {row["id"]: row for row in reference["cases"]}
    comparisons = []
    component_checks = []
    for row in manifest["splits"]["holdout"]:
        rate, count, blocks = row["rate"], row["frames"], row["native"]["blocks"]
        near = load_source(args.data, row, "near").astype(float)
        echo = load_source(args.data, row, "echo").astype(float)
        features = load_row(row, "features").reshape(blocks, BANDS, FEATURES)
        error = load_row(row, "error_spectra").reshape(blocks, 257, 2).astype(float)
        spectrum = error[:, :, 0] + 1j * error[:, :, 1]
        neural, gains = apply_masks(spectrum, features, model, rate)
        methods = {
            "adaptive": load_row(row, "adaptive").astype(float),
            "classical": load_row(row, "classical").astype(float),
            "neural": neural,
        }
        intervals = {
            "steady": (5 * rate, count),
            "onset": (3 * rate, 3 * rate + rate // 8),
        }
        if row["scene"] in ["double_talk", "quiet_double_talk"]:
            intervals["double_talk"] = (13 * rate // 4, 17 * rate // 4)
        elif row["scene"] == "near_only":
            intervals = {"near_only": (0, count)}
        elif row["scene"] == "near_after_echo":
            intervals["near_after_echo"] = (13 * rate // 4, count)
        # Conditional decomposition: freeze the gains computed from the mixture,
        # then apply them to known clean near-end FFTs. This separates actual
        # near attenuation from residual echo in the mixture-to-near SNR.
        near_blocks = near.reshape(blocks, 256)
        near_spectrum = np.fft.rfft(
            np.column_stack([np.zeros_like(near_blocks), near_blocks]), axis=1
        )
        for band in range(BANDS):
            bins = slice(band * 257 // BANDS, (band + 1) * 257 // BANDS)
            near_spectrum[:, bins] *= gains[:, band, None]
        near_component = np.fft.irfft(near_spectrum, n=512, axis=1)[:, 256:].reshape(-1)
        for phase, (start, end) in intervals.items():
            target = near[start:end]
            near_present = np.dot(target, target) > 1e-20
            scores = {}
            for method, audio in methods.items():
                y = audio[start:end]
                score = (
                    float(snr(target, y - target))
                    if near_present
                    else float(snr(echo[start:end], y))
                )
                key = "near_snr_db" if near_present else "erle_db"
                expected = reports[row["id"]]["methods"][method][phase][key]
                if not np.isclose(score, expected, atol=1e-9):
                    raise ValueError(
                        "independent quality reducer disagrees with reported score"
                    )
                scores[method] = score
            comparisons.append(
                {
                    "id": row["id"],
                    "scene": row["scene"],
                    "rate": rate,
                    "phase": phase,
                    "metric": "near_snr_db" if near_present else "erle_db",
                    "scores": scores,
                    "neural_minus_classical_db": scores["neural"] - scores["classical"],
                }
            )
            if near_present:
                component = near_component[start:end]
                component_checks.append(
                    {
                        "id": row["id"],
                        "scene": row["scene"],
                        "phase": phase,
                        "conditional_near_snr_db": float(
                            snr(target, component - target)
                        ),
                        "conditional_near_projection_gain_db": float(
                            20
                            * np.log10(
                                max(
                                    abs(
                                        np.dot(component, target)
                                        / np.dot(target, target)
                                    ),
                                    1e-15,
                                )
                            )
                        ),
                    }
                )
    steady_echo = [
        x for x in comparisons if x["phase"] == "steady" and x["metric"] == "erle_db"
    ]
    report = {
        "model_sha256": model.sha256,
        "cases": len(reports),
        "comparisons": comparisons,
        "conditional_near_checks": component_checks,
        "steady_echo_neural_better_than_classical": sum(
            x["neural_minus_classical_db"] > 0 for x in steady_echo
        ),
        "steady_echo_comparisons": len(steady_echo),
        "adoption_recommended": False,
        "reason": "Classical post-filter wins most steady echo comparisons; 48 kHz double-talk onset still has much more residual echo with neural-only masking. No live worker integration or natural-speech acceptance yet.",
        "scope": "Conditional neural near component only; classical near attenuation is not decomposed. Do not rank near-only numerical noise floors as model improvements.",
    }
    args.output.write_text(json.dumps(report, indent=2, allow_nan=False) + "\n")
    print(
        json.dumps(
            {
                k: v
                for k, v in report.items()
                if k not in ["comparisons", "conditional_near_checks"]
            }
        )
    )


if __name__ == "__main__":
    main()
