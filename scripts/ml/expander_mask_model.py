"""Fixed native expander feature contract and experimental gain-network format."""

import hashlib
import json
from pathlib import Path

import numpy as np
from source_remix_model import objective, unpack
from source_remix_model import predict as network_predict

# Only the generic 6→8→1 network arithmetic/gradient is shared with source remix.
# Its spatial features, training data, policy and model loader are not reused.
FORMAT = "sotf-expander-denoising-mask-v1"
PROVENANCE = "project-generated music/noise; GPL-3.0-or-later; no external audio"


def features(power, previous):
    power, previous = np.asarray(power, float), np.asarray(previous, float)
    if (
        power.shape != (513,)
        or previous.shape != (513,)
        or not np.isfinite(power).all()
        or not np.isfinite(previous).all()
        or np.any((power < 0) | (power > 4096))
        or np.any((previous < 0) | (previous > 4096))
        or np.mean(power) <= 1e-20
    ):
        raise ValueError("Invalid or silent fixed-FFT expander evidence")
    p = np.maximum(power, 1e-20)
    windows = np.lib.stride_tricks.sliding_window_view(np.pad(p, 4, mode="edge"), 9)
    local = windows.mean(axis=1)
    geometric = np.exp(np.log(windows).mean(axis=1))
    return np.clip(
        np.column_stack(
            [
                (10 * np.log10(p) + 100) / 100,
                (10 * np.log10(p / local) + 20) / 40,
                geometric / local,
                (10 * np.log10(p / np.maximum(previous, 1e-20)) + 30) / 60,
                np.arange(513) / 512,
                np.full(513, (10 * np.log10(power.mean()) + 100) / 100),
            ]
        ),
        0,
        1,
    )


def export(theta):
    theta = np.asarray(theta, float)
    if theta.shape != (65,) or not np.isfinite(theta).all() or np.max(abs(theta)) > 16:
        raise ValueError("Use 65 finite bounded coefficients")
    w, b, v, c = unpack(theta)
    return {
        "format": FORMAT,
        "weights": w.tolist(),
        "bias": b.tolist(),
        "output_weights": v.tolist(),
        "output_bias": float(c),
        "provenance": PROVENANCE,
        "deployment_admitted": False,
    }


def load(path, sha):
    with Path(path).open("rb") as f:
        raw = f.read(65537)
    if not raw or len(raw) > 65536 or hashlib.sha256(raw).hexdigest() != sha:
        raise ValueError("Expander mask identity/size mismatch")
    d = json.loads(raw)
    if (
        not isinstance(d, dict)
        or set(d) != set(export(np.zeros(65)))
        or d["format"] != FORMAT
        or d["provenance"] != PROVENANCE
        or d["deployment_admitted"] is not False
    ):
        raise ValueError("Unsupported experimental expander model")

    def numeric(v):
        if isinstance(v, list):
            return all(numeric(x) for x in v)
        return type(v) in (int, float) and np.isfinite(v)

    keys = ["weights", "bias", "output_weights", "output_bias"]
    if not all(numeric(d[k]) for k in keys):
        raise ValueError("Coefficients must be finite numbers")
    w, b, v = [np.asarray(d[k], float) for k in keys[:3]]
    if (
        w.shape != (6, 8)
        or b.shape != (8,)
        or v.shape != (8,)
        or np.ndim(d["output_bias"])
    ):
        raise ValueError("Fixed expander network dimensions required")
    theta = np.r_[w.ravel(), b, v, d["output_bias"]]
    if np.max(abs(theta)) > 16:
        raise ValueError("Coefficient bound exceeded")
    return theta


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
        raise ValueError("Invalid normalized fixed-network evidence")
    return network_predict(theta, x)


__all__ = ["export", "features", "load", "objective", "predict"]
