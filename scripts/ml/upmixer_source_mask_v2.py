"""Explicit native-feature model contract with the frozen raw-power policy."""

import hashlib
import json
from pathlib import Path

import numpy as np
from upmixer_source_mask import contract as v1_contract
from upmixer_source_mask import unpack


def contract():
    obj = v1_contract()
    obj["format"] = "sotf-upmixer-direct-ambient-mask-v2"
    obj["policy"]["fallback"] = "ordinary_native_decomposition"
    obj["policy"]["excitation"] = {
        "band_power_interval_exclusive": [1e-16, 1e12],
        "relative_to_peak_band_min": 1e-6,
        "power_scale": "reference_fft",
        "dc_nyquist": "excluded",
    }
    obj["provenance"]["training_features"] = "native_f32_fft_f64_covariance"
    return obj


def save(theta, path):
    unpack(theta)
    data = contract() | {"coefficients": np.asarray(theta).tolist()}
    raw = (json.dumps(data, indent=2, allow_nan=False) + "\n").encode()
    Path(path).write_bytes(raw)
    return hashlib.sha256(raw).hexdigest()


def load(path, sha):
    with Path(path).open("rb") as stream:
        raw = stream.read(65537)
    if len(raw) > 65536 or hashlib.sha256(raw).hexdigest() != sha:
        raise ValueError("Source-mask v2 model size/hash mismatch")
    data = json.loads(raw)
    if not isinstance(data, dict):
        raise ValueError("Source-mask v2 contract must be an object")  # noqa: TRY004
    theta = data.pop("coefficients", None)
    if json.dumps(data, sort_keys=True) != json.dumps(contract(), sort_keys=True):
        raise ValueError("Unsupported source-mask v2 contract")
    if not isinstance(theta, list) or any(type(v) not in (int, float) for v in theta):
        raise ValueError("Coefficients require numeric scalars")
    unpack(theta)
    return np.asarray(theta)


def observations(metadata):
    rows = metadata["rows"]
    features = np.array(
        [
            [f if f is not None else [0.0] * 6 for f in row["state"]["features"]]
            for row in rows
        ]
    )
    available = np.array(
        [[f is not None for f in row["state"]["features"]] for row in rows]
    )
    power = np.array(
        [
            [p if p is not None else 0.0 for p in row["state"]["band_power"]]
            for row in rows
        ]
    )
    eligible = available & (power >= power.max(axis=1, keepdims=True) * 1e-6)
    return features, eligible, power
