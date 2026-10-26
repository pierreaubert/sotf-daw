"""Off-thread horizontal HRIR kernel regression and bounded inverse proposals."""

import hashlib
import json
from pathlib import Path

import numpy as np

FORMAT = "sotf-xtc-horizontal-hrir-kernel-v1"
POLICY = {
    "sample_rate": 44100,
    "ir_frames": 512,
    "fft_size": 2048,
    "elevation_deg": 0.0,
    "radius_m": 1.4,
    "kernel": "unit_circle_gaussian",
    "coefficient_limit": 10000.0,
    "maximum_inverse_operator_gain": 4.0,
    "relative_tikhonov_beta": 0.001,
    "deployment": "offline_proposal_only",
}


def angles_vector(degrees):
    a = np.asarray(degrees, float)
    if a.ndim != 1 or len(a) > 4096 or not np.isfinite(a).all():
        raise ValueError("Bounded finite azimuth vector required")
    a = np.deg2rad(a % 360)
    return np.column_stack([np.cos(a), np.sin(a)])


def kernel(a, b, length_deg):
    if not np.isfinite(length_deg) or not 5 <= length_deg <= 90:
        raise ValueError("Kernel angular scale must be5..90 degrees")
    av, bv = angles_vector(a), angles_vector(b)
    d2 = np.sum((av[:, None] - bv[None]) ** 2, axis=2)
    return np.exp(-d2 / (2 * np.deg2rad(length_deg) ** 2))


def fit(angles, ir, length_deg, ridge):
    x = np.asarray(ir, float)
    if (
        x.shape != (len(angles), 2, 512)
        or not 4 <= len(angles) <= 128
        or not np.isfinite(x).all()
        or np.max(abs(x)) > 16
    ):
        raise ValueError("Finite bounded horizontal calibration HRIRs required")
    if not np.isfinite(ridge) or not 1e-6 <= ridge <= 0.1:
        raise ValueError("Ridge must be1e-6..0.1")
    matrix = kernel(angles, angles, length_deg)
    weights = np.linalg.solve(
        matrix + ridge * np.eye(len(angles)), x.reshape(len(angles), -1)
    )
    if not np.isfinite(weights).all() or np.max(abs(weights)) > 10000:
        raise ValueError("Learned transfer coefficients exceed bounded domain")
    return {
        "angles_deg": np.asarray(angles, float).tolist(),
        "length_deg": float(length_deg),
        "ridge": float(ridge),
        "weights": weights.reshape(x.shape).tolist(),
    }


def predict(model, angles, *, elevation_deg=0.0, radius_m=1.4):
    if elevation_deg != 0.0 or not np.isfinite(radius_m) or abs(radius_m - 1.4) > 1e-6:
        raise ValueError("Model has no measured elevation/radial coverage")
    anchors = model["angles_deg"]
    weights = np.asarray(model["weights"], float)
    if (
        not 4 <= len(anchors) <= 128
        or weights.shape != (len(anchors), 2, 512)
        or not np.isfinite(weights).all()
        or np.max(abs(weights)) > 10000
    ):
        raise ValueError("Invalid transfer-model weights")
    output = (
        kernel(angles, anchors, model["length_deg"]) @ weights.reshape(len(anchors), -1)
    ).reshape(-1, 2, 512)
    if not np.isfinite(output).all() or np.max(abs(output)) > 16:
        raise ValueError("Predicted transfer outside calibrated amplitude domain")
    return output


def save(model, path, provenance):
    document = {
        "format": FORMAT,
        "policy": POLICY,
        "model": model,
        "provenance": provenance,
        "deployment_admitted": False,
    }
    raw = (json.dumps(document, indent=2, allow_nan=False) + "\n").encode()
    if len(raw) > 8 * 1024 * 1024:
        raise ValueError("Transfer model exceeds8MiB")
    Path(path).write_bytes(raw)
    return hashlib.sha256(raw).hexdigest()


def load(path, sha):
    with Path(path).open("rb") as f:
        raw = f.read(8 * 1024 * 1024 + 1)
    if len(raw) > 8 * 1024 * 1024 or hashlib.sha256(raw).hexdigest() != sha:
        raise ValueError("Transfer model size/hash mismatch")
    obj = json.loads(raw)
    if not isinstance(obj, dict) or set(obj) != {
        "format",
        "policy",
        "model",
        "provenance",
        "deployment_admitted",
    }:
        raise ValueError("Unsupported transfer document")
    if (
        obj["format"] != FORMAT
        or json.dumps(obj["policy"], sort_keys=True)
        != json.dumps(POLICY, sort_keys=True)
        or obj["deployment_admitted"] is not False
    ):
        raise ValueError("Unsupported transfer policy/admission")
    m = obj["model"]
    if not isinstance(m, dict) or set(m) != {
        "angles_deg",
        "length_deg",
        "ridge",
        "weights",
    }:
        raise ValueError("Unsupported transfer model fields")

    def numeric(x):
        return (
            all(numeric(v) for v in x)
            if isinstance(x, list)
            else type(x) in (int, float) and np.isfinite(x)
        )

    if not all(numeric(v) for v in m.values()) or not 1e-6 <= m["ridge"] <= 0.1:
        raise ValueError("Finite numeric model fields required")
    predict(m, np.array([0.0]))
    provenance = obj["provenance"]
    if (
        not isinstance(provenance, dict)
        or not isinstance(provenance.get("dataset_sha256"), str)
        or len(provenance["dataset_sha256"]) != 64
        or not provenance.get("authors")
        or not provenance.get("terms")
    ):
        raise ValueError("Transfer model needs explicit measured provenance")
    return m, provenance


def plant(left_ir, right_ir):
    """Rows are ears, columns speakers; measured HRIRs use positive-left azimuth."""
    inputs = np.stack([left_ir, right_ir], axis=-1)
    return np.fft.rfft(inputs, n=2048, axis=1).transpose(1, 0, 2)


def bounded_inverse(transfer):
    """Propose a regularized inverse with a common per-bin operator-gain cap."""
    h = np.asarray(transfer, complex)
    if h.shape != (1025, 2, 2) or not np.isfinite(h).all() or np.max(abs(h)) > 10000:
        raise ValueError("Finite bounded FFT2048 transfer matrix required")
    u, s, vh = np.linalg.svd(h)
    beta = 0.001 * np.maximum(np.mean(s * s, axis=1), 1e-12)
    factors = s / (s * s + beta[:, None])
    w = vh.conj().transpose(0, 2, 1) @ (
        factors[:, :, None] * u.conj().transpose(0, 2, 1)
    )
    # One real matrix scale preserves both column steering ratios and caps the
    # largest singular value; every speaker-row L2 norm is consequently <=4.
    w *= np.minimum(1.0, 4.0 / np.maximum(np.max(factors, axis=1), 1e-30))[
        :, None, None
    ]
    return w


def log_ratio_db(numerator, denominator):
    """Represent signed/unbounded metrics without clipping or nonstandard JSON."""
    if (
        not np.isfinite(numerator)
        or not np.isfinite(denominator)
        or numerator < 0
        or denominator < 0
    ):
        raise ValueError("Metric magnitudes must be finite and nonnegative")
    if numerator == 0 and denominator == 0:
        return {"kind": "undefined", "value": None}
    if denominator == 0:
        return {"kind": "positive_infinity", "value": None}
    if numerator == 0:
        return {"kind": "negative_infinity", "value": None}
    return {
        "kind": "finite",
        "value": float(20 * (np.log10(numerator) - np.log10(denominator))),
    }


def cancellation_metrics(actual, filters, bins):
    response = actual @ filters
    records = []
    for b in bins:
        for cue in range(2):
            desired, unintended = (
                abs(response[b, cue, cue]),
                abs(response[b, 1 - cue, cue]),
            )
            records.append(
                {
                    "bin": int(b),
                    "frequency_hz": float(b * 44100 / 2048),
                    "cue": cue,
                    "intended_gain_db": log_ratio_db(desired, 1.0),
                    "normalized_separation_db": log_ratio_db(desired, unintended),
                    "raw_crosstalk_attenuation_db": log_ratio_db(
                        abs(actual[b, 1 - cue, cue]), unintended
                    ),
                }
            )
    return records
