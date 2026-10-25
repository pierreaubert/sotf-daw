"""Verify native model parity and independently recompute near-end oracle labels."""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import time
from pathlib import Path

import numpy as np
from aec_residual_model import BANDS, FEATURES, FLOOR, ResidualModel
from train_aec_residual import load_row, load_source


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--model-sha256", required=True)
    parser.add_argument("--native", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        raise ValueError("preserve existing verification")
    args.output.mkdir(parents=True)
    model = ResidualModel(args.model, args.model_sha256)
    manifest = json.loads((args.data / "manifest.json").read_text())
    probes = [
        np.zeros((1, BANDS, FEATURES), np.float32),
        np.ones((1, BANDS, FEATURES), np.float32),
        np.random.default_rng(20273025).random(
            (256, BANDS, FEATURES), dtype=np.float32
        ),
    ]
    label_checks = []
    for rows in manifest["splits"].values():
        for row in rows:
            blocks = row["native"]["blocks"]
            features = load_row(row, "features").reshape(blocks, BANDS, FEATURES)
            probes.append(features[:8])
            near = (
                load_source(args.data, row, "near").astype(float).reshape(blocks, 256)
            )
            near_spectrum = np.fft.rfft(
                np.column_stack([np.zeros_like(near), near]), axis=1
            )
            error = load_row(row, "error_spectra").reshape(blocks, 257, 2).astype(float)
            error = error[:, :, 0] + 1j * error[:, :, 1]
            targets = load_row(row, "targets").reshape(blocks, 2, BANDS)
            gain_error, weight_error = 0.0, 0.0
            bin_weights = np.full(257, 2.0)
            bin_weights[[0, -1]] = 1.0
            for band in range(BANDS):
                bins = slice(band * 257 // BANDS, (band + 1) * 257 // BANDS)
                e, s, weight = error[:, bins], near_spectrum[:, bins], bin_weights[bins]
                cross = np.sum(weight * np.real(s * np.conj(e)), axis=1)
                ep = np.sum(weight * np.abs(e) ** 2, axis=1)
                sp = np.sum(weight * np.abs(s) ** 2, axis=1)
                gain = np.where(
                    ep > 1e-20, np.clip(cross / np.maximum(ep, 1e-30), FLOOR, 1), 1
                )
                importance = 1 + 9 * np.clip(sp / np.maximum(ep, 1e-20), 0, 1)
                gain_error = max(
                    gain_error, float(np.max(np.abs(gain - targets[:, 0, band])))
                )
                weight_error = max(
                    weight_error,
                    float(np.max(np.abs(importance - targets[:, 1, band]))),
                )
            label_checks.append(
                {
                    "id": row["id"],
                    "max_gain_error": gain_error,
                    "max_importance_error": weight_error,
                }
            )
    batch = np.concatenate(probes)
    features_path = args.output / "features.f32le"
    features_path.write_bytes(batch.astype("<f4").tobytes())
    command = [str(args.native), str(args.model), args.model_sha256, str(features_path)]
    start = time.perf_counter()
    process = subprocess.run(command, capture_output=True, text=True, check=True)
    native = np.asarray(json.loads(process.stdout), dtype=np.float32)
    reference = model.predict(batch)
    if native.shape != reference.shape:
        raise ValueError("native prediction shape mismatch")
    error = float(np.max(np.abs(native - reference)))
    report = {
        "model_sha256": model.sha256,
        "native_sha256": hashlib.sha256(args.native.read_bytes()).hexdigest(),
        "frames": len(batch),
        "band_predictions": int(native.size),
        "max_native_numpy_gain_error": error,
        "native_numpy_tolerance": 1e-5,
        "command_seconds": time.perf_counter() - start,
        "command": command,
        "native_stderr": process.stderr,
        "native_prediction_allocation_assertions": "active in QA executable for every frame",
        "oracle_checks": label_checks,
        "oracle_gain_tolerance": 1e-4,
        "oracle_importance_tolerance": 1e-3,
        "passed": error <= 1e-5
        and all(
            row["max_gain_error"] <= 1e-4 and row["max_importance_error"] <= 1e-3
            for row in label_checks
        ),
    }
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(
        json.dumps(
            {
                k: v
                for k, v in report.items()
                if k not in ["oracle_checks", "command", "native_stderr"]
            }
        )
    )
    if not report["passed"]:
        raise ValueError("native inference or independent oracle verification failed")


if __name__ == "__main__":
    main()
