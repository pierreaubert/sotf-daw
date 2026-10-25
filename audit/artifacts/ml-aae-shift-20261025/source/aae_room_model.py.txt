"""Dependency-light inference for a bounded, monotone AAE forward surrogate."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import numpy as np
from aae_room_features import CONTROL_SPEC, FEATURE_SPEC
from scipy.special import expit


class Surrogate:
    def __init__(self, path: Path, sha256: str):
        with Path(path).open("rb") as source:
            raw = source.read(4 * 1024 * 1024 + 1)
        if len(raw) > 4 * 1024 * 1024 or hashlib.sha256(raw).hexdigest() != sha256:
            raise ValueError("AAE surrogate size/hash mismatch")
        model = json.loads(raw)
        if not isinstance(model, dict):
            raise TypeError("AAE surrogate must be a JSON object")
        if (
            model.get("format") != "sotf_aae_monotone_forward_v1"
            or model.get("feature_spec") != FEATURE_SPEC
        ):
            raise ValueError("AAE surrogate feature contract mismatch")
        expected_architecture = {
            "input": 19,
            "hidden": [128, 128],
            "hidden_activation": "silu",
            "output": "negative_cumsum_softplus_clamp40",
            "curve_shape": [7, 32],
        }
        if (
            model.get("control_spec") != CONTROL_SPEC
            or model.get("architecture") != expected_architecture
        ):
            raise ValueError("AAE surrogate control/architecture contract mismatch")
        self.layers = []
        dimensions = [19, 128, 128, 224]
        if not isinstance(model.get("layers"), list) or len(model["layers"]) != 3:
            raise ValueError("invalid AAE surrogate layers")
        for index, layer in enumerate(model["layers"]):
            if not isinstance(layer, dict):
                raise TypeError("invalid AAE surrogate layer object")
            try:
                weight = np.asarray(layer["weight"], dtype=np.float32)
                bias = np.asarray(layer["bias"], dtype=np.float32)
            except (KeyError, TypeError, ValueError, OverflowError) as error:
                raise ValueError("invalid AAE surrogate numeric arrays") from error
            if weight.shape != (
                dimensions[index + 1],
                dimensions[index],
            ) or bias.shape != (dimensions[index + 1],):
                raise ValueError("invalid AAE surrogate shape")
            if (
                not np.isfinite(weight).all()
                or not np.isfinite(bias).all()
                or np.max(np.abs(weight)) > 100
                or np.max(np.abs(bias)) > 100
            ):
                raise ValueError("invalid AAE surrogate weights")
            self.layers.append((weight, bias))
        self.sha256 = sha256
        self.metadata = model

    def predict(self, encoded: np.ndarray) -> np.ndarray:
        x = np.asarray(encoded, dtype=np.float32)
        single = x.ndim == 1
        if single:
            x = x[None, :]
        if (
            x.ndim != 2
            or x.shape[1] != 19
            or not np.isfinite(x).all()
            or np.any(x < 0)
            or np.any(x > 1)
        ):
            raise ValueError("AAE surrogate input outside encoded domain")
        categories = x[:, 6:]
        if (
            np.any((categories != 0) & (categories != 1))
            or not np.all(np.sum(x[:, 6:15], axis=1) == 1)
            or not np.all(np.sum(x[:, 15:19], axis=1) == 1)
        ):
            raise ValueError("AAE surrogate requires one layout and one preset")
        for index, (weight, bias) in enumerate(self.layers):
            x = x @ weight.T + bias
            if index < 2:
                x = x * expit(x)
        increments = np.logaddexp(np.float32(0), x).reshape(-1, 7, 32)
        output = -np.minimum(np.cumsum(increments, axis=2), np.float32(40)).reshape(
            -1, 224
        )
        if not np.isfinite(output).all():
            raise ValueError("AAE surrogate output is non-finite")
        return output[0] if single else output
