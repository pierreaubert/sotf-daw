"""Train a causal gain mask on source-disjoint generated music/noise fixtures."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from music_mask_cases import fixture
from music_noise_mask import (
    analysis,
    export,
    feature_sequence,
    load,
    loss_gradient,
    predict,
)
from scipy.optimize import minimize


def scene(seed, kind="matched"):
    rate = [44100, 48000, 96000][seed % 3]
    fft = [512, 2048][seed % 2]
    mixture, clean, noise, metadata = fixture(seed, rate, kind=kind)
    raw = analysis(mixture, fft)
    c, n = analysis(clean, fft), analysis(noise, fft)
    target = np.sqrt(abs(c) ** 2 / np.maximum(abs(c) ** 2 + abs(n) ** 2, 1e-20))
    x = feature_sequence(raw, rate, fft)
    metadata.update(
        fft_size=fft,
        analysis_frames=len(raw),
        features_sha256=hashlib.sha256(x.astype("<f8").tobytes()).hexdigest(),
    )
    return x.reshape(-1, 6), target.ravel(), metadata


def corpus(seed, count, sample=False, kind="matched"):
    xs, ys, rows = [], [], []
    for i in range(count):
        x, y, metadata = scene(seed + i, kind)
        if sample:
            rng = np.random.default_rng(seed + i + 100000)
            selected = []
            for eligible in [y > 0.5, y <= 0.5]:
                indices = np.flatnonzero(eligible)
                if not len(indices):
                    indices = np.arange(len(y))
                selected.extend(rng.choice(indices, 512, replace=len(indices) < 512))
            x, y = x[selected], y[selected]
        xs.append(x)
        ys.append(y)
        rows.append(metadata)
    return np.vstack(xs), np.concatenate(ys), rows


def score(theta, x, y):
    prediction = predict(theta, x)
    # A local spectral-flatness mask is a feature-only reference, not native MCRA.
    simple = np.clip(1 - x[:, 2], 0, 1)
    return {
        "rows": len(y),
        "gain_mse": float(np.mean((prediction - y) ** 2)),
        "flatness_gain_mse": float(np.mean((simple - y) ** 2)),
        "target_dominant_gain_mse": float(
            np.mean((prediction[y > 0.5] - y[y > 0.5]) ** 2)
        ),
        "gain_min": float(prediction.min()),
        "gain_max": float(prediction.max()),
    }


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--output", type=Path, required=True)
    args = p.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    x, y, groups = corpus(20265120, 32, sample=True)
    vx, vy, validation = corpus(20266120, 8, sample=True)
    theta = np.random.default_rng(20265119).normal(0, 0.2, 65)
    fit = minimize(
        loss_gradient,
        theta,
        args=(x, y),
        jac=True,
        method="L-BFGS-B",
        bounds=[(-16, 16)] * 65,
        options={"maxiter": 300, "ftol": 1e-10, "gtol": 1e-6},
    )
    model = args.output / "model.json"
    model.write_text(json.dumps(export(fit.x), indent=2, allow_nan=False) + "\n")
    identity = hashlib.sha256(model.read_bytes()).hexdigest()
    theta = load(model, identity)
    # Freeze model and policy before any final fixture is generated or scored.
    held = {}
    for kind in [
        "matched",
        "dense_percussion",
        "vibrato",
        "step_noise",
        "tone_noise",
        "clean_only",
    ]:
        hx, hy, metadata = corpus(20267120, 8, kind=kind)
        held[kind] = {"metrics": score(theta, hx, hy), "groups": metadata}
    report = {
        "model_sha256": identity,
        "optimizer_success": bool(fit.success),
        "optimizer_message": str(fit.message),
        "iterations": int(fit.nit),
        "training_groups": groups,
        "validation_groups": validation,
        "training": score(theta, x, y),
        "validation": score(theta, vx, vy),
        "held": held,
        "sampling": "1024 rows per training/validation source:512 target-dominant and512 others, equal sampled row counts; global weighted gain MSE emphasizes foreground and does not give exactly equal loss weight to sources. Held metrics use every frame/bin.",
        "deployment_admitted": False,
        "scope": "Generated polyphonic/percussive music and noise only.32/8/8 disjoint source seeds; final shift variants reuse final seeds. Mask gain MSE is not waveform/artifact, native latency or real-music validation.",
    }
    (args.output / "training-report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    print(
        json.dumps(
            {
                "model_sha256": identity,
                "optimizer_success": report["optimizer_success"],
                "held": {k: v["metrics"] for k, v in held.items()},
            }
        )
    )


if __name__ == "__main__":
    main()
