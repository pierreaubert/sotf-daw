"""A bounded centered-source energy estimator for an explicit stereo remix stage."""

import hashlib
import json
from pathlib import Path

import numpy as np
from scipy.special import expit

FORMAT = "sotf-centered-source-mask-v1"
POLICY = {
    "sample_rates": [44100, 48000, 96000],
    "fft_size": 512,
    "channels": 2,
    "confidence": 0.85,
    "gain_limit_db": 6.0,
    "gain_smoothing_ms": 5.0,
    "maximum_model_lookahead_samples": 0,
    "fallback": "neutral_remix",
    "label": "centered_foreground_mid_energy_fraction",
}
PROVENANCE = {
    "origin": "project-generated centered foreground and stereo bed mixtures",
    "license": "GPL-3.0-or-later",
    "recorded_sources": False,
    "speech_identity": False,
}


def features(left, right, frequency):
    left, right = np.asarray(left, complex), np.asarray(right, complex)
    frequency = np.asarray(frequency, float)
    if (
        left.shape != right.shape
        or left.shape != frequency.shape
        or not np.isfinite(left).all()
        or not np.isfinite(right).all()
        or not np.isfinite(frequency).all()
        or np.any((frequency < 0) | (frequency > 1))
    ):
        raise ValueError("Invalid stereo spectral evidence")
    pl, pr = abs(left) ** 2, abs(right) ** 2
    total = pl + pr
    if np.any(total <= 1e-20) or np.any(total > 1e12):
        raise ValueError("Stereo evidence outside bounded excitation domain")
    cross = left * right.conj()
    denominator = np.sqrt(pl * pr)
    agreement = np.divide(
        cross.real, denominator, out=np.zeros_like(total), where=denominator > 1e-20
    )
    quadrature = np.divide(
        abs(cross.imag),
        denominator,
        out=np.zeros_like(total),
        where=denominator > 1e-20,
    )
    return np.clip(
        np.column_stack(
            [
                abs(left + right) ** 2 / (2 * total),
                (agreement + 1) / 2,
                np.minimum(pl, pr) / np.maximum(pl, pr),
                quadrature,
                frequency,
                (10 * np.log10(total / 512) + 100) / 100,
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


def objective(theta, x, target):
    w, b, v, c = unpack(theta)
    centered = 2 * x - 1
    hidden = np.tanh(centered @ w + b)
    z = hidden @ v + c
    p = expit(z)
    loss = float(np.mean(np.logaddexp(0, z) - target * z) + 1e-5 * np.dot(theta, theta))
    delta = (p - target) / len(x)
    back = delta[:, None] * v * (1 - hidden**2)
    return loss, np.r_[
        (centered.T @ back).ravel(), back.sum(axis=0), hidden.T @ delta, delta.sum()
    ] + 2e-5 * theta


def export(theta):
    w, b, v, c = unpack(theta)
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
    with Path(path).open("rb") as f:
        raw = f.read(65537)
    if len(raw) > 65536 or hashlib.sha256(raw).hexdigest() != sha:
        raise ValueError("Centered-source model size/hash mismatch")
    d = json.loads(raw)
    template = export(np.zeros(65))
    if (
        not isinstance(d, dict)
        or set(d) != set(template)
        or any(
            json.dumps(d[k], sort_keys=True) != json.dumps(template[k], sort_keys=True)
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
        raise ValueError("Unsupported unadmitted centered-source contract")

    def numeric(v):
        return (
            all(numeric(x) for x in v)
            if isinstance(v, list)
            else type(v) in (int, float) and np.isfinite(v)
        )

    if not all(
        numeric(d[k]) for k in ["weights", "bias", "output_weights", "output_bias"]
    ):
        raise ValueError("Use finite numeric coefficients")
    w, b, v = (np.asarray(d[k], float) for k in ["weights", "bias", "output_weights"])
    if w.shape != (6, 8) or b.shape != (8,) or v.shape != (8,):
        raise ValueError("Centered-source dimensions mismatch")
    theta = np.r_[w.ravel(), b, v, d["output_bias"]]
    if np.max(abs(theta)) > 16:
        raise ValueError("Centered-source coefficient bound exceeded")
    return theta
