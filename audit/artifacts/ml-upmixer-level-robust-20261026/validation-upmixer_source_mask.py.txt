"""Offline direct/ambient masks; generated-source research, never speech labels."""

import hashlib
import json
from pathlib import Path

import numpy as np
from scipy.special import expit

FFT = 512
HOP = 256
RATE = 48000
FORMAT = "sotf-upmixer-direct-ambient-mask-v1"


def contract():
    return {
        "format": FORMAT,
        "policy": {
            "sample_rates": [RATE],
            "fft_size": FFT,
            "hop_size": HOP,
            "band_bins": 8,
            "features": 6,
            "hidden": 8,
            "window": "periodic_sqrt_hann",
            "model_lookahead_samples": 0,
            "label": "generated_direct_stem_power_fraction",
            "fallback": "analytic_directness",
            "mask_bounds": [0.0, 1.0],
        },
        "provenance": {
            "origin": "project-generated panned direct and stereo diffuse stems",
            "license": "GPL-3.0-or-later",
            "recorded_sources": False,
            "speech_identity": False,
        },
        "deployment_admitted": False,
    }


def unpack(theta):
    t = np.asarray(theta, float)
    if t.shape != (65,) or not np.isfinite(t).all() or np.max(abs(t)) > 16:
        raise ValueError("Expected65 finite coefficients bounded by16")
    return t[:48].reshape(6, 8), t[48:56], t[56:64], t[64]


def predict(theta, x):
    x = np.asarray(x, float)
    if (
        x.ndim != 2
        or x.shape[1] != 6
        or not np.isfinite(x).all()
        or np.any((x < 0) | (x > 1))
    ):
        raise ValueError("Expected bounded normalized six-feature rows")
    w, b, v, c = unpack(theta)
    return expit(np.tanh((2 * x - 1) @ w + b) @ v + c)


def objective(theta, x, y):
    w, b, v, c = unpack(theta)
    centered = 2 * x - 1
    h = np.tanh(centered @ w + b)
    z = h @ v + c
    p = expit(z)
    loss = np.mean(np.logaddexp(0, z) - y * z) + 1e-5 * np.dot(theta, theta)
    delta = (p - y) / len(x)
    back = delta[:, None] * v * (1 - h * h)
    grad = np.r_[
        (centered.T @ back).ravel(), back.sum(axis=0), h.T @ delta, delta.sum()
    ]
    return float(loss), grad + 2e-5 * theta


def save(theta, path):
    unpack(theta)
    obj = contract() | {"coefficients": np.asarray(theta).tolist()}
    raw = (json.dumps(obj, indent=2, allow_nan=False) + "\n").encode()
    Path(path).write_bytes(raw)
    return hashlib.sha256(raw).hexdigest()


def load(path, sha):
    with Path(path).open("rb") as f:
        raw = f.read(65537)
    if len(raw) > 65536 or hashlib.sha256(raw).hexdigest() != sha:
        raise ValueError("Source-mask model size/hash mismatch")
    obj = json.loads(raw)
    if not isinstance(obj, dict):
        raise ValueError("Source-mask contract must be an object")  # noqa: TRY004 -- malformed external JSON value
    coefficients = obj.pop("coefficients", None)
    if json.dumps(obj, sort_keys=True) != json.dumps(contract(), sort_keys=True):
        raise ValueError("Unsupported source-mask policy/provenance/admission")
    if not isinstance(coefficients, list) or any(
        type(v) not in (int, float) for v in coefficients
    ):
        raise ValueError("Coefficients require numeric scalars")
    unpack(coefficients)
    return np.asarray(coefficients)


def analyze(signal):
    """Independent Python reference: padded sqrt-Hann analysis, two channels."""
    x = np.asarray(signal, float)
    if x.ndim != 2 or x.shape[1] != 2 or not np.isfinite(x).all():
        raise ValueError("Finite stereo input required")
    padded = np.pad(x, ((HOP, FFT), (0, 0)))
    window = np.sqrt(0.5 - 0.5 * np.cos(2 * np.pi * np.arange(FFT) / FFT))
    starts = range(0, len(padded) - FFT + 1, HOP)
    return np.asarray(
        [np.fft.rfft(padded[s : s + FFT] * window[:, None], axis=0) for s in starts]
    )


def synthesize(spectra, frames):
    window = np.sqrt(0.5 - 0.5 * np.cos(2 * np.pi * np.arange(FFT) / FFT))
    result = np.zeros((len(spectra) * HOP + FFT, 2))
    for i, s in enumerate(spectra):
        result[i * HOP : i * HOP + FFT] += (
            np.fft.irfft(s, n=FFT, axis=0) * window[:, None]
        )
    return result[HOP : HOP + frames]


def evidence(spectra):
    """Band covariance cues; absence/invalid evidence falls back independently."""
    features, baseline, valid = [], [], []
    for start in range(1, FFT // 2, 8):
        s = spectra[:, start : min(start + 8, FFT // 2)]
        pl = np.sum(abs(s[:, :, 0]) ** 2, axis=1)
        pr = np.sum(abs(s[:, :, 1]) ** 2, axis=1)
        cross = np.sum(s[:, :, 0] * s[:, :, 1].conj(), axis=1)
        total = pl + pr
        ok = np.isfinite(total) & (total > 1e-16) & (total < 1e12)
        safe = np.maximum(total, 1e-16)
        balance = abs(pl - pr) / safe
        coherence = np.clip(2 * cross.real / safe, -1, 1)
        baseline.append(np.maximum(balance, np.maximum(coherence, 0)))
        features.append(
            np.column_stack(
                [
                    balance,
                    (coherence + 1) / 2,
                    np.clip(2 * abs(cross.imag) / safe, 0, 1),
                    np.clip(2 * abs(cross) / safe, 0, 1),
                    np.full(len(s), (start + min(start + 8, FFT // 2) - 1) / FFT),
                    np.clip((10 * np.log10(safe / FFT) + 100) / 100, 0, 1),
                ]
            )
        )
        valid.append(ok)
    return (
        np.stack(features, axis=1),
        np.stack(baseline, axis=1),
        np.stack(valid, axis=1),
    )


def targets(direct, ambient):
    columns = []
    for start in range(1, FFT // 2, 8):
        end = min(start + 8, FFT // 2)
        pd = np.sum(abs(direct[:, start:end]) ** 2, axis=(1, 2))
        pa = np.sum(abs(ambient[:, start:end]) ** 2, axis=(1, 2))
        columns.append(pd / np.maximum(pd + pa, 1e-16))
    return np.stack(columns, axis=1)


def apply_mask(spectra, masks):
    result = np.zeros_like(spectra)
    # DC/Nyquist are deliberately retained by the independent analytic path.
    result[:, [0, -1]] = spectra[:, [0, -1]]
    for i, start in enumerate(range(1, FFT // 2, 8)):
        result[:, start : min(start + 8, FFT // 2)] = (
            spectra[:, start : min(start + 8, FFT // 2)] * masks[:, i, None, None]
        )
    return result


def select_masks(theta, spectra):
    """Missing, invalid or unavailable model evidence keeps analytic masks."""
    x, baseline, valid = evidence(spectra)
    selected = baseline.copy()
    if theta is not None:
        try:
            selected[valid] = predict(theta, x[valid])
        except ValueError:
            pass
    return selected, baseline, valid
