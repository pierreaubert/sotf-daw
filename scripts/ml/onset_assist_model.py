"""Fixed causal onset model contract and class-balanced fitting arithmetic."""

import hashlib
import json
from pathlib import Path

import numpy as np
from scipy.special import expit
from source_remix_model import unpack

FORMAT = "sotf-transient-onset-assist-v1"
PROVENANCE = (
    "project-generated onset/steady/noise signals; GPL-3.0-or-later; no external audio"
)
POLICY = {
    "sample_rates": [44100, 48000, 96000],
    "analysis_ms": 2.0,
    "control_smoothing_ms": 5.0,
    "maximum_attack_modulation": 0.25,
    "confidence": 0.9,
    "added_lookahead_samples": 0,
    "fallback": "independent_native_attack",
    "label": "causal_hop_contains_generated_onset",
}


def features(power, peak, previous, difference, amplitude, fast, slow):
    values = np.array([power, peak, previous, difference, amplitude, fast, slow], float)
    if (
        not np.isfinite(values).all()
        or np.any(values < 0)
        or power <= 1e-20
        or max(power, peak, previous, amplitude, fast, slow) > 1
        or difference > 2
        or power > peak * peak + 1e-12
        or amplitude > peak + 1e-12
    ):
        raise ValueError("Invalid bounded onset statistics")
    rms = np.sqrt(power)
    return np.clip(
        [
            (20 * np.log10(max(rms, 1e-20)) + 100) / 100,
            20 * np.log10(max(peak / rms, 1)) / 24,
            (10 * np.log10(power / max(previous, 1e-20)) + 30) / 60,
            max(fast - slow, 0) / max(slow, 1e-10),
            difference / (2 * max(amplitude, 1e-20)),
            (20 * np.log10(max(fast / rms, 1e-20)) + 24) / 48,
        ],
        0,
        1,
    )


def predict(theta, x):
    theta, x = np.asarray(theta, float), np.asarray(x, float)
    if (
        theta.shape != (65,)
        or not np.isfinite(theta).all()
        or np.max(abs(theta)) > 16
        or x.ndim != 2
        or x.shape[1] != 6
        or not np.isfinite(x).all()
        or np.any((x < 0) | (x > 1))
    ):
        raise ValueError("Invalid normalized fixed onset-network evidence")
    w, b, v, c = unpack(theta)
    return expit(np.tanh((2 * np.asarray(x) - 1) @ w + b) @ v + c)


def objective(theta, x, y):
    positive = np.count_nonzero(y)
    if not 0 < positive < len(y):
        raise ValueError("Training needs both classes")
    weights = np.where(y > 0, 0.5 / positive, 0.5 / (len(y) - positive))
    w, b, v, c = unpack(theta)
    centered = 2 * x - 1
    hidden = np.tanh(centered @ w + b)
    z = hidden @ v + c
    loss = float(
        np.sum(weights * (np.logaddexp(0, z) - y * z)) + 1e-5 * np.dot(theta, theta)
    )
    delta = weights * (expit(z) - y)
    back = delta[:, None] * v * (1 - hidden**2)
    gradient = (
        np.r_[
            (centered.T @ back).ravel(), back.sum(axis=0), hidden.T @ delta, delta.sum()
        ]
        + 2e-5 * theta
    )
    return loss, gradient


def export(theta):
    theta = np.asarray(theta, float)
    if theta.shape != (65,) or not np.isfinite(theta).all() or np.max(abs(theta)) > 16:
        raise ValueError("Invalid fixed onset coefficients")
    w, b, v, c = unpack(theta)
    return {
        "format": FORMAT,
        "weights": w.tolist(),
        "bias": b.tolist(),
        "output_weights": v.tolist(),
        "output_bias": float(c),
        "policy": POLICY,
        "provenance": PROVENANCE,
        "deployment_admitted": False,
    }


def load(path, sha):
    with Path(path).open("rb") as f:
        raw = f.read(65537)
    if not raw or len(raw) > 65536 or hashlib.sha256(raw).hexdigest() != sha:
        raise ValueError("Onset model identity mismatch")
    data = json.loads(raw)
    template = export(np.zeros(65))
    if (
        not isinstance(data, dict)
        or set(data) != set(template)
        or any(
            json.dumps(data[k], sort_keys=True)
            != json.dumps(template[k], sort_keys=True)
            for k in ["format", "policy", "provenance", "deployment_admitted"]
        )
    ):
        raise ValueError("Unknown onset contract")

    def numeric(v):
        return (
            all(numeric(x) for x in v)
            if isinstance(v, list)
            else type(v) in (int, float) and np.isfinite(v)
        )

    if not all(
        numeric(data[k]) for k in ["weights", "bias", "output_weights", "output_bias"]
    ):
        raise ValueError("Use finite numeric coefficients")
    w, b, v = [
        np.asarray(data[k], float) for k in ["weights", "bias", "output_weights"]
    ]
    if (
        w.shape != (6, 8)
        or b.shape != (8,)
        or v.shape != (8,)
        or np.ndim(data["output_bias"])
    ):
        raise ValueError("Onset dimensions mismatch")
    theta = np.r_[w.ravel(), b, v, data["output_bias"]]
    if np.max(abs(theta)) > 16:
        raise ValueError("Onset coefficient bound exceeded")
    return theta
