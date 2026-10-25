"""Train a bounded spatial mask on disjoint generated array scenes."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from beamformer_noise_mask import export, features, load, loss_gradient, predict
from scipy.optimize import minimize


def scene(seed, count=256, shift="matched"):
    rng = np.random.default_rng(seed)
    microphones = int(rng.integers(2, 9))
    rate = int(rng.choice([16000, 48000, 96000]))
    spacing = rng.uniform(0.01, 0.1)
    angle = rng.uniform(-60, 60)
    other = angle + rng.choice([-1, 1]) * rng.uniform(20, 100)
    if shift == "near_interference":
        other = angle + rng.choice([-1, 1]) * 5
    frequency = rng.integers(1, 257, count) * rate / 512
    positions = np.arange(microphones) * spacing

    def steering(azimuth):
        return np.exp(
            -2j
            * np.pi
            * frequency[:, None]
            * positions[None, :]
            * np.sin(np.deg2rad(azimuth))
            / 343
        )

    target_power = 10 ** rng.uniform(-4, 0, count)
    noise_power = target_power * 10 ** rng.uniform(-2, 2, count)
    target_power[: count // 10] = 0
    noise_power[count // 10 : count // 5] = 0
    target = (
        steering(angle)
        * np.sqrt(target_power[:, None])
        * np.exp(2j * np.pi * rng.random((count, 1)))
    )
    noise = (
        steering(other)
        * np.sqrt(0.8 * noise_power[:, None])
        * np.exp(2j * np.pi * rng.random((count, 1)))
    )
    noise += np.sqrt(0.1 * noise_power[:, None]) * (
        rng.normal(size=(count, microphones))
        + 1j * rng.normal(size=(count, microphones))
    )
    if shift == "reverberant_target":
        target += (
            0.7
            * steering(angle + 40)
            * np.sqrt(target_power[:, None])
            * np.exp(2j * np.pi * rng.random((count, 1)))
        )
    if shift == "sensor_gain":
        gains = np.exp(rng.normal(0, 0.35, microphones))
        target *= gains
        noise *= gains
    noise_energy, target_energy = (np.sum(abs(v) ** 2, axis=1) for v in [noise, target])
    label = noise_energy / (noise_energy + target_energy)
    x = features(target + noise, steering(angle), frequency / (rate / 2))
    return (
        x,
        label,
        {
            "seed": seed,
            "microphones": microphones,
            "sample_rate": rate,
            "spacing_m": spacing,
            "look_angle": angle,
            "interferer_angle": other,
            "shift": shift,
            "rows": count,
            "features_sha256": hashlib.sha256(x.astype("<f8").tobytes()).hexdigest(),
            "labels_sha256": hashlib.sha256(label.astype("<f8").tobytes()).hexdigest(),
        },
    )


def corpus(first, groups, shift="matched"):
    rows = [scene(first + i, shift=shift) for i in range(groups)]
    return (
        np.vstack([r[0] for r in rows]),
        np.concatenate([r[1] for r in rows]),
        [r[2] for r in rows],
    )


def scores(theta, x, y):
    p = predict(theta, x)
    # This is a per-bin coherence comparator; native baseline gates the full frame.
    baseline = (x[:, 0] < 0.65).astype(float)
    certain = (p >= 0.85) | (p <= 0.15)
    return {
        "rows": len(y),
        "mask_mse": float(np.mean((p - y) ** 2)),
        "per_bin_coherence_mse": float(np.mean((baseline - y) ** 2)),
        "confidence_coverage": float(np.mean(certain)),
        "confident_noise_with_target_dominant_fraction": float(
            np.mean((p >= 0.85) & (y < 0.5))
        ),
        "scope": "Generated target-direction/noise energy masks, not speech identity or waveform beamforming quality",
    }


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--output", type=Path, required=True)
    args = p.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    x, y, groups = corpus(20261120, 64)
    vx, vy, validation = corpus(20262120, 16)
    rng = np.random.default_rng(20261119)
    theta = rng.normal(0, 0.2, 65)
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
    # Model and fixed policy are frozen before final groups are generated/read.
    held = {}
    for shift in ["matched", "near_interference", "sensor_gain", "reverberant_target"]:
        hx, hy, metadata = corpus(20263120, 32, shift)
        held[shift] = {"metrics": scores(theta, hx, hy), "groups": metadata}
    report = {
        "model_sha256": identity,
        "optimizer_success": bool(fit.success),
        "optimizer_message": str(fit.message),
        "iterations": int(fit.nit),
        "train_groups": groups,
        "validation_groups": validation,
        "train": scores(theta, x, y),
        "validation": scores(theta, vx, vy),
        "held": held,
        "deployment_admitted": False,
        "scope": "64/16/32 disjoint generated scene seeds. Shift variants reuse final seeds and are correlated. No recorded speech, room/microphone recordings, waveform quality, native inference or deadline admission.",
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
