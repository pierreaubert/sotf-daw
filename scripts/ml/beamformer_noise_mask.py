"""Causal spatial target/noise mask prototype; no speech identity inference."""

import hashlib
import json
from pathlib import Path

import numpy as np
from scipy.special import expit

FORMAT = "sotf-beamformer-spatial-mask-v1"
FEATURES = 6
HIDDEN = 8
POLICY = {
    "confidence": 0.85,
    "maximum_added_lookahead_samples": 0,
    "fallback": "native_frame_coherence_gate",
    "sample_rates": [16000, 48000, 96000],
    "minimum_mics": 2,
    "maximum_mics": 8,
    "minimum_spacing_cm": 1.0,
    "maximum_spacing_cm": 10.0,
    "maximum_abs_steer_degrees": 60.0,
}


def features(x, direction, normalized_frequency):
    x, direction = np.asarray(x, complex), np.asarray(direction, complex)
    if x.ndim != 2 or x.shape != direction.shape or not 2 <= x.shape[1] <= 8:
        raise ValueError("Use rows of matched2..8-microphone spectra/steering")
    frequency = np.asarray(normalized_frequency, float)
    if frequency.shape != (len(x),) or np.any((frequency < 0) | (frequency > 1)):
        raise ValueError("Frequency must be normalized to Nyquist")
    if (
        not np.isfinite(x).all()
        or not np.isfinite(direction).all()
        or not np.isfinite(frequency).all()
    ):
        raise ValueError("Invalid spatial mask evidence")
    if np.max(abs(abs(direction) - 1)) > 1e-4:
        raise ValueError("Mask requires physical unit-magnitude steering")
    power = abs(x) ** 2
    total = power.sum(axis=1)
    if np.any(total <= 1e-20) or np.any(total > 1e12):
        raise ValueError("Spatial mask evidence outside bounded excitation domain")
    aligned = x * direction.conj()
    look = abs(aligned.sum(axis=1)) ** 2 / (x.shape[1] * total)
    pair = np.zeros(len(x))
    pairs = 0
    for i in range(x.shape[1]):
        for j in range(i):
            denominator = np.sqrt(power[:, i] * power[:, j])
            pair += np.divide(
                (aligned[:, i] * aligned[:, j].conj()).real,
                denominator,
                out=np.zeros(len(x)),
                where=denominator > 1e-20,
            )
            pairs += 1
    return np.clip(
        np.column_stack(
            [
                look,
                (pair / pairs + 1) / 2,
                power.min(axis=1) / power.max(axis=1),
                power[:, 0] / total,
                frequency,
                np.full(len(x), (x.shape[1] - 2) / 6),
            ]
        ),
        0,
        1,
    )


def unpack(theta):
    return theta[:48].reshape(FEATURES, HIDDEN), theta[48:56], theta[56:64], theta[64]


def predict(theta, x):
    w, b, v, c = unpack(np.asarray(theta, float))
    return expit(np.tanh((2 * np.asarray(x) - 1) @ w + b) @ v + c)


def loss_gradient(theta, x, target):
    w, b, v, c = unpack(theta)
    centered = 2 * x - 1
    hidden = np.tanh(centered @ w + b)
    logits = hidden @ v + c
    probability = expit(logits)
    regularization = 1e-5
    loss = float(
        np.mean(np.logaddexp(0, logits) - target * logits)
        + regularization * np.dot(theta, theta)
    )
    error = (probability - target) / len(x)
    back = error[:, None] * v * (1 - hidden**2)
    gradient = np.r_[
        (centered.T @ back).ravel(), back.sum(axis=0), hidden.T @ error, error.sum()
    ]
    return loss, gradient + 2 * regularization * theta


def export(theta):
    w, b, v, c = unpack(np.asarray(theta))
    return {
        "format": FORMAT,
        "features": FEATURES,
        "hidden": HIDDEN,
        "weights": w.tolist(),
        "bias": b.tolist(),
        "output_weights": v.tolist(),
        "output_bias": float(c),
        "policy": POLICY,
        "deployment_admitted": False,
        "provenance": {
            "origin": "project-generated complex plane-wave mixtures",
            "license": "GPL-3.0-or-later",
            "recorded_speech": False,
        },
    }


def load(path, sha):
    with Path(path).open("rb") as source:
        raw = source.read(65537)
    if len(raw) > 65536 or hashlib.sha256(raw).hexdigest() != sha:
        raise ValueError("Spatial mask file size/hash mismatch")
    data = json.loads(raw)
    if (
        not isinstance(data, dict)
        or set(data) != set(export(np.zeros(65)))
        or data["format"] != FORMAT
        or type(data["features"]) is not int
        or type(data["hidden"]) is not int
        or data["features"] != FEATURES
        or data["hidden"] != HIDDEN
        or data["policy"] != POLICY
        or data["deployment_admitted"] is not False
        or json.dumps(data["provenance"], sort_keys=True)
        != json.dumps(export(np.zeros(65))["provenance"], sort_keys=True)
        or json.dumps(data["policy"], sort_keys=True)
        != json.dumps(POLICY, sort_keys=True)
    ):
        raise ValueError("Unsupported unadmitted mask schema/policy")

    def numeric(value):
        if isinstance(value, list):
            return all(numeric(v) for v in value)
        return type(value) in (int, float) and np.isfinite(value)

    if not all(
        numeric(data[k]) for k in ["weights", "bias", "output_weights", "output_bias"]
    ):
        raise ValueError("Use finite numeric spatial mask coefficients")
    w, b, v = (
        np.asarray(data[k], float) for k in ["weights", "bias", "output_weights"]
    )
    if w.shape != (FEATURES, HIDDEN) or b.shape != (HIDDEN,) or v.shape != (HIDDEN,):
        raise ValueError("Spatial mask dimensions mismatch")
    theta = np.r_[w.ravel(), b, v, data["output_bias"]]
    if not np.isfinite(theta).all() or np.max(abs(theta)) > 16:
        raise ValueError("Spatial mask coefficients exceed bounded domain")
    return theta
