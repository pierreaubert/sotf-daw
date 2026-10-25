"""Recompute frozen mask and native waveform results without fitter metrics."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from train_beamformer_noise_mask import scene


def analyze(model, training, waveform):
    data = json.loads(model.read_text())
    identity = hashlib.sha256(model.read_bytes()).hexdigest()
    assert identity == training["model_sha256"] == waveform["model_sha256"]
    w = np.asarray(data["weights"]).T
    b = np.asarray(data["bias"])
    v = np.asarray(data["output_weights"])
    held = []
    groups = {r["seed"] for r in training["train_groups"]}
    development = {r["seed"] for r in training["validation_groups"]}
    assert not groups & development
    for shift, result in training["held"].items():
        final = {r["seed"] for r in result["groups"]}
        assert not (groups | development) & final
        x, y = [], []
        for row in result["groups"]:
            features, label, metadata = scene(row["seed"], shift=shift)
            assert metadata == row
            x.append(features)
            y.append(label)
        x = np.vstack(x)
        y = np.concatenate(y)
        z = v @ np.tanh(w @ (2 * x.T - 1) + b[:, None]) + data["output_bias"]
        probability = 1 / (1 + np.exp(-z))
        mask_error = float(np.dot(probability - y, probability - y) / len(y))
        assert abs(mask_error - result["metrics"]["mask_mse"]) < 1e-12
        held.append(
            {
                "shift": shift,
                "groups": len(final),
                "rows": len(y),
                "mask_mse_recomputed": mask_error,
            }
        )
    maximum_error = 0.0
    deltas = []
    for row in waveform["cases"]:
        target_raw = Path(row["target_file"]).read_bytes()
        assert hashlib.sha256(target_raw).hexdigest() == row["target_sha256"]
        target = np.frombuffer(target_raw, dtype="<f4").astype(float)
        for method, results in row["methods"].items():
            raw = Path(results["output_file"]).read_bytes()
            metadata = results["metadata"]
            assert hashlib.sha256(raw).hexdigest() == metadata["output_sha256"]
            output = np.frombuffer(raw, dtype="<f4").astype(float)
            assert np.all(output[:512] == 0)
            assert len(output) == metadata["output_frames"]
            aligned = output[512 : 512 + len(target)]
            for name, start in [
                ("full", 0),
                ("steady_after_one_second", row["sample_rate"]),
            ]:
                a, t = aligned[start:], target[start:]
                error = np.linalg.norm(a - t)
                reference = np.linalg.norm(t)
                reconstructed = {
                    "relative_rms": float(error / reference),
                    "snr_db": float(20 * np.log10(reference / error)),
                    "max_abs_error": float(np.max(abs(a - t))),
                    "projection_gain": float(np.dot(a, t) / (reference**2)),
                }
                for key, value in reconstructed.items():
                    maximum_error = max(maximum_error, abs(value - results[name][key]))
            frames = metadata["output_frames"] // 256 - 1
            counts = metadata["counts"]
            if method == "classical":
                assert counts["disabled_frames"] == frames
            else:
                assert (
                    sum(
                        counts[k]
                        for k in [
                            "noise_bins",
                            "target_bins",
                            "uncertain_bins",
                            "invalid_bins",
                        ]
                    )
                    == 257 * frames
                )
        delta = (
            row["methods"]["neural"]["steady_after_one_second"]["snr_db"]
            - row["methods"]["classical"]["steady_after_one_second"]["snr_db"]
        )
        assert delta == row["steady_neural_minus_classical_snr_db"]
        deltas.append(delta)
    assert maximum_error < 1e-9
    return {
        "model_sha256": identity,
        "held_mask_recomputation": held,
        "native_waveform_cases": len(deltas),
        "waveform_metric_max_error": maximum_error,
        "native_latency_samples": 512,
        "added_mask_lookahead_samples": 0,
        "steady_snr_delta_range_db": [min(deltas), max(deltas)],
        "deployment_admitted": False,
        "scope": "Independent arithmetic and hashes on generated held scenes and native PCM, with prefix/frame-counter accounting. No real-recording, perceptual or platform admission.",
    }


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--model", type=Path, required=True)
    p.add_argument("--training", type=Path, required=True)
    p.add_argument("--waveform", type=Path, required=True)
    p.add_argument("--report", type=Path, required=True)
    args = p.parse_args()
    result = analyze(
        args.model,
        json.loads(args.training.read_text()),
        json.loads(args.waveform.read_text()),
    )
    with args.report.open("x") as out:
        out.write(json.dumps(result, indent=2, allow_nan=False) + "\n")
    print(json.dumps(result))


if __name__ == "__main__":
    main()
