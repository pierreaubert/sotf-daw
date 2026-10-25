"""Tiny offline repair discriminator; only whole native groups can be accepted."""

import hashlib
import json
from pathlib import Path

import numpy as np
from declick_native import features, groups
from scipy.special import expit

FORMAT = "sotf-declick-repair-discriminator-v1"


class Model:
    def __init__(self, path, expected_sha256):
        with Path(path).open("rb") as source:
            raw = source.read(131073)
        if len(raw) > 131072 or hashlib.sha256(raw).hexdigest() != expected_sha256:
            raise ValueError("Declick model size/hash mismatch")
        data = json.loads(raw)
        if (
            not isinstance(data, dict)
            or data.get("format") != FORMAT
            or data.get("domain")
            != {
                "sample_rate": 48000,
                "channels": 1,
                "native_params": "legacy-sensitivity5-linked-v1",
            }
        ):
            raise ValueError("Unsupported Declick model contract")
        arrays = {}
        for key, shape in [
            ("mean", (16,)),
            ("scale", (16,)),
            ("w1", (16, 16)),
            ("b1", (16,)),
            ("w2", (16,)),
            ("b2", ()),
        ]:
            value = np.asarray(data[key], dtype=np.float64)
            if (
                value.shape != shape
                or not np.isfinite(value).all()
                or np.max(np.abs(value)) > 100
            ):
                raise ValueError("Invalid Declick model array")
            arrays[key] = value
        if np.any(arrays["scale"] < 1e-3):
            raise ValueError("Invalid feature scale")
        self.__dict__.update(arrays)
        self.threshold = data["threshold"]
        if (
            isinstance(self.threshold, bool)
            or not isinstance(self.threshold, (int, float))
            or not 0.5 <= self.threshold <= 1
        ):
            raise ValueError("Invalid admission threshold")
        self.digest = expected_sha256

    def probability(self, values):
        values = np.asarray(values, dtype=np.float64)
        if values.shape != (16,) or not np.isfinite(values).all():
            raise ValueError("Invalid Declick features")
        normalized = (values - self.mean) / self.scale
        if np.max(np.abs(normalized)) > 12:
            raise ValueError("Features outside training normalization support")
        return float(expit(np.tanh(self.w1 @ normalized + self.b1) @ self.w2 + self.b2))


def qualify(dry, native, rate, model):
    dry, native = (
        np.asarray(dry, dtype=np.float32),
        np.asarray(native, dtype=np.float32),
    )
    if (
        dry.shape != native.shape
        or dry.ndim != 2
        or not np.isfinite(dry).all()
        or not np.isfinite(native).all()
    ):
        raise ValueError("Use finite aligned frame/channel arrays")
    output, decisions = dry.copy(), []
    for start, stop in groups(dry, native):
        reason, probability, accepted = "unsupported_domain", None, False
        if rate == 48000 and dry.shape[1] == 1 and model is not None:
            try:
                probability = model.probability(
                    features(dry, native, start, stop, rate)
                )
                accepted = probability >= model.threshold
                reason = (
                    "accepted_native_group" if accepted else "insufficient_model_score"
                )
            except ValueError:
                reason = "unsupported_context_or_features"
        elif model is None:
            reason = "model_unavailable"
        if accepted:
            output[start:stop] = native[start:stop]
        decisions.append(
            {
                "start": start,
                "stop": stop,
                "probability": probability,
                "accepted": accepted,
                "reason": reason,
            }
        )
    return output, decisions
