"""Causal generated-music mask with explicit independent feature/gain history."""

import hashlib
import json
from pathlib import Path

import numpy as np
from scipy.ndimage import uniform_filter1d
from scipy.special import expit

FORMAT = "sotf-music-noise-mask-v1"
POLICY = {
    "sample_rates": [44100, 48000, 96000],
    "fft_sizes": [512, 2048],
    "maximum_channels": 2,
    "feature_history_ms": 30.0,
    "gain_attack_ms": 5.0,
    "gain_release_ms": 30.0,
    "reference_reduction_db": 20.0,
    "maximum_added_lookahead_samples": 0,
    "fallback": "independent_native_denoiser",
    "drain": "freeze_last_learned_gains",
}
PROVENANCE = {
    "origin": "project-generated polyphonic and percussive music/noise mixtures",
    "license": "GPL-3.0-or-later",
    "external_training_audio": False,
    "speech_pretrained": False,
}


def features(power, previous, fft_size):
    power = np.asarray(power, float)
    previous = np.asarray(previous, float)
    if (
        power.ndim != 1
        or power.shape != (fft_size // 2 + 1,)
        or previous.shape != power.shape
        or not np.isfinite(power).all()
        or not np.isfinite(previous).all()
        or np.any(power < 0)
        or np.any(previous < 0)
        or np.max(power) > 1e12
    ):
        raise ValueError("Invalid bounded music-mask spectrum/history")
    epsilon = 1e-20
    safe = np.maximum(power, epsilon)
    local = uniform_filter1d(safe, size=5, mode="nearest")
    broad = max(float(power.mean()), epsilon)
    arithmetic = uniform_filter1d(safe, size=9, mode="nearest")
    geometric = np.exp(uniform_filter1d(np.log(safe), size=9, mode="nearest"))
    temporal = np.maximum(previous, epsilon)
    return np.clip(
        np.column_stack(
            [
                (np.log(safe / np.maximum(local, epsilon)) + 4) / 8,
                (np.log(safe / broad) + 8) / 16,
                geometric / np.maximum(arithmetic, epsilon),
                (np.log(safe / temporal) + 4) / 8,
                np.arange(len(power)) / (len(power) - 1),
                np.full(
                    len(power), (10 * np.log10(broad / (fft_size / 2)) + 100) / 100
                ),
            ]
        ),
        0,
        1,
    )


def unpack(theta):
    return theta[:48].reshape(6, 8), theta[48:56], theta[56:64], theta[64]


def predict(theta, x):
    w, b, v, c = unpack(np.asarray(theta, float))
    return expit(np.tanh((2 * np.asarray(x) - 1) @ w + b) @ v + c)


def loss_gradient(theta, x, target):
    w, b, v, c = unpack(theta)
    centered = 2 * x - 1
    hidden = np.tanh(centered @ w + b)
    probability = expit(hidden @ v + c)
    weight = 0.25 + target**2
    denominator = float(weight.sum())
    error = probability - target
    loss = float(np.dot(weight, error**2) / denominator + 1e-5 * np.dot(theta, theta))
    delta = 2 * weight * error * probability * (1 - probability) / denominator
    back = delta[:, None] * v * (1 - hidden**2)
    gradient = np.r_[
        (centered.T @ back).ravel(), back.sum(axis=0), hidden.T @ delta, delta.sum()
    ]
    return loss, gradient + 2e-5 * theta


def export(theta):
    w, b, v, c = unpack(np.asarray(theta))
    return {
        "format": FORMAT,
        "features": 6,
        "hidden": 8,
        "weights": w.tolist(),
        "bias": b.tolist(),
        "output_weights": v.tolist(),
        "output_bias": float(c),
        "policy": POLICY,
        "provenance": PROVENANCE,
        "deployment_admitted": False,
    }


def load(path, sha):
    with Path(path).open("rb") as stream:
        raw = stream.read(65537)
    if len(raw) > 65536 or hashlib.sha256(raw).hexdigest() != sha:
        raise ValueError("Music-mask size/hash mismatch")
    value = json.loads(raw)
    template = export(np.zeros(65))
    if (
        not isinstance(value, dict)
        or set(value) != set(template)
        or any(
            json.dumps(value[k], sort_keys=True)
            != json.dumps(template[k], sort_keys=True)
            for k in [
                "format",
                "features",
                "hidden",
                "policy",
                "provenance",
                "deployment_admitted",
            ]
        )
    ):
        raise ValueError("Unsupported unadmitted music-mask schema/policy")

    def numeric(v):
        return (
            all(numeric(x) for x in v)
            if isinstance(v, list)
            else type(v) in (int, float) and np.isfinite(v)
        )

    if not all(
        numeric(value[k]) for k in ["weights", "bias", "output_weights", "output_bias"]
    ):
        raise ValueError("Use finite numeric coefficients")
    w, b, v = (
        np.asarray(value[k], float) for k in ["weights", "bias", "output_weights"]
    )
    if w.shape != (6, 8) or b.shape != (8,) or v.shape != (8,):
        raise ValueError("Music-mask dimensions mismatch")
    theta = np.r_[w.ravel(), b, v, value["output_bias"]]
    if not np.isfinite(theta).all() or np.max(abs(theta)) > 16:
        raise ValueError("Music-mask coefficient bounds exceeded")
    return theta


def analysis(signal, fft_size):
    """Native periodic sqrt-Hann, half-window prefix and unnormalized forward FFT."""
    x = np.asarray(signal, float)
    if x.ndim != 1 or not len(x) or not np.isfinite(x).all():
        raise ValueError("Use finite mono source")
    hop = fft_size // 2
    window = np.sqrt(0.5 - 0.5 * np.cos(2 * np.pi * np.arange(fft_size) / fft_size))
    padded = np.pad(x, (hop, fft_size + hop))
    starts = np.arange(0, len(x) + hop, hop)
    spectra = np.fft.rfft(
        np.lib.stride_tricks.sliding_window_view(padded, fft_size)[starts] * window,
        axis=1,
    )
    return spectra


def feature_sequence(spectra, rate, fft_size):
    previous = np.zeros(fft_size // 2 + 1)
    retention = np.exp(-(fft_size / 2) / rate / 0.03)
    rows = []
    for i, spectrum in enumerate(spectra):
        power = abs(spectrum) ** 2
        if i == 0:
            previous = power.copy()
        rows.append(features(power, previous, fft_size))
        previous = retention * previous + (1 - retention) * power
    return np.asarray(rows)
