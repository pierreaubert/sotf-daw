"""Freeze an expander gain candidate before source-disjoint synthetic shift probes."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from expander_mask_model import export, features, load, objective, predict
from music_mask_cases import fixture
from scipy.optimize import minimize


def scene(seed, rate=48000, kind="matched", rows=256):
    mixture, clean, noise, metadata = fixture(seed, rate, kind=kind)
    if kind == "noise_only":
        clean.fill(0)
        mixture = noise.copy()
    if kind == "level_shift":
        mixture, clean, noise = [x * 4 for x in (mixture, clean, noise)]
    # Recompute hashes after explicit scene modifications, never retain stale IDs.
    for name, values in [("mixture", mixture), ("clean", clean), ("noise", noise)]:
        metadata[name + "_sha256"] = hashlib.sha256(
            values.astype("<f4").tobytes()
        ).hexdigest()
    window = 0.5 - 0.5 * np.cos(2 * np.pi * np.arange(1024) / 1024)
    window_sum = window.sum()
    previous = np.zeros(513)
    history = np.exp(-256 / (0.030 * rate))
    all_x, all_y, coordinates = [], [], []
    # Exact causal schedule: three prefix windows, source classification only
    # when a complete 256-frame source hop has been accepted; no EOF learning.
    for hop in range(len(mixture) // 256):
        origin = hop * 256 - 768
        start, stop = max(0, origin), min(len(mixture), origin + 1024)

        def spectrum(signal, start=start, stop=stop, origin=origin):
            frame = np.zeros(1024)
            frame[start - origin : stop - origin] = signal[start:stop]
            return np.fft.rfft(frame * window)

        observed, desired, corruption = [spectrum(x) for x in (mixture, clean, noise)]
        power = (abs(observed) * 2 / window_sum) ** 2
        if power.mean() <= 1e-20:
            continue
        all_x.append(features(power, previous)[1:-1])
        all_y.append(
            (
                abs(desired) ** 2
                / np.maximum(abs(desired) ** 2 + abs(corruption) ** 2, 1e-20)
            )[1:-1]
        )
        coordinates.extend((hop, bin) for bin in range(1, 512))
        previous = power + history * (previous - power)
    x, y = np.vstack(all_x), np.concatenate(all_y)
    # Equal sampled-row count per source, uniform within source. Loss is the
    # ordinary row mean with a fixed coefficient regularizer.
    indices = np.sort(
        np.random.default_rng(seed + 19).choice(len(y), rows, replace=False)
    )
    x, y = x[indices], y[indices]
    metadata.update(
        rows=rows,
        feature_sha256=hashlib.sha256(x.astype("<f8").tobytes()).hexdigest(),
        target_sha256=hashlib.sha256(y.astype("<f8").tobytes()).hexdigest(),
        coordinate_sha256=hashlib.sha256(
            np.asarray(coordinates, dtype="<i8")[indices].tobytes()
        ).hexdigest(),
    )
    return x, y, metadata


def corpus(first, groups, kind="matched", rate=48000):
    data = [scene(first + i, rate, kind) for i in range(groups)]
    return (
        np.vstack([x[0] for x in data]),
        np.concatenate([x[1] for x in data]),
        [x[2] for x in data],
    )


def metrics(theta, x, y):
    p = predict(theta, x)
    confident = (p <= 0.15) | (p >= 0.85)
    # This simple flatness mask is only a feature baseline. Actual classical
    # expander quality requires native waveform evaluation, still outstanding.
    flatness = np.clip(1 - x[:, 2], 0.1, 1)
    return {
        "rows": len(y),
        "mask_mse": float(np.mean((p - y) ** 2)),
        "flatness_mask_mse": float(np.mean((flatness - y) ** 2)),
        "confidence_coverage": float(confident.mean()),
        "confident_attenuation_when_desired_dominant_fraction": float(
            np.mean((p <= 0.15) & (y >= 0.5))
        ),
        "confident_preservation_when_noise_dominant_fraction": float(
            np.mean((p >= 0.85) & (y < 0.5))
        ),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    x, y, training = corpus(20275120, 32)
    vx, vy, validation = corpus(20276120, 8)
    fit = minimize(
        objective,
        np.random.default_rng(20275119).normal(0, 0.2, 65),
        args=(x, y),
        jac=True,
        method="L-BFGS-B",
        bounds=[(-16, 16)] * 65,
        options={"maxiter": 300, "ftol": 1e-10, "gtol": 1e-6},
    )
    model = args.output / "model.json"
    model.write_text(json.dumps(export(fit.x), indent=2, allow_nan=False) + "\n")
    sha = hashlib.sha256(model.read_bytes()).hexdigest()
    theta = load(model, sha)
    report = {
        "model_sha256": sha,
        "optimizer_success": bool(fit.success),
        "optimizer_message": str(fit.message),
        "iterations": int(fit.nit),
        "training_groups": training,
        "validation_groups": validation,
        "training": metrics(theta, x, y),
        "validation": metrics(theta, vx, vy),
        "held": {},
        "deployment_admitted": False,
        "scope": "Generated monaural music/noise only. Model frozen before final groups. Shift variants reuse final source seeds and are correlated. No native audio-quality or generalization admission.",
    }
    for kind, rate in [
        ("matched", 48000),
        ("matched", 44100),
        ("matched", 96000),
        ("step_noise", 48000),
        ("tone_noise", 48000),
        ("dense_percussion", 48000),
        ("vibrato", 48000),
        ("clean_only", 48000),
        ("noise_only", 48000),
        ("level_shift", 48000),
    ]:
        hx, hy, groups = corpus(20277120, 16, kind, rate)
        report["held"][f"{kind}-{rate}"] = dict(metrics(theta, hx, hy), groups=groups)
    if hashlib.sha256(model.read_bytes()).hexdigest() != sha:
        raise RuntimeError("Frozen model changed during final evaluation")
    (args.output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    print(
        json.dumps(
            {
                k: v
                for k, v in report.items()
                if k not in {"training_groups", "validation_groups", "held"}
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
