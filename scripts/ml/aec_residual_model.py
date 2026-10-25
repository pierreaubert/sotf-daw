"""Bounded NumPy reference inference for the small native AEC residual model."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import numpy as np
from scipy.special import expit

FORMAT = "sotf_aec_residual_mask_v1"
BANDS, FEATURES, HIDDEN = 16, 12, 24
FLOOR = 0.25


class ResidualModel:
    def __init__(self, path: Path, expected_sha256: str):
        with path.open("rb") as source:
            raw = source.read(262145)
        if len(raw) > 262144 or hashlib.sha256(raw).hexdigest() != expected_sha256:
            raise ValueError("AEC model size/hash mismatch")
        value = json.loads(raw)
        if not isinstance(value, dict):
            raise TypeError("AEC model must be a JSON object")
        if any(
            value.get(key) != expected
            for key, expected in {
                "format": FORMAT,
                "feature_version": 1,
                "bands": BANDS,
                "features": FEATURES,
                "hidden": HIDDEN,
                "gain_floor": FLOOR,
            }.items()
        ):
            raise ValueError("AEC feature/model contract mismatch")
        self.arrays = []
        for key, shape in [
            ("w1", (HIDDEN, FEATURES)),
            ("b1", (HIDDEN,)),
            ("w2", (HIDDEN,)),
            ("b2", ()),
        ]:
            try:
                array = np.asarray(value[key], dtype=np.float32)
            except (KeyError, TypeError, ValueError, OverflowError) as error:
                raise ValueError("invalid AEC model array") from error
            if (
                array.shape != shape
                or not np.isfinite(array).all()
                or np.any(np.abs(array) > 100)
            ):
                raise ValueError("invalid AEC model weights")
            self.arrays.append(array)
        self.sha256 = expected_sha256

    def predict(self, features):
        x = np.asarray(features, dtype=np.float32)
        if (
            x.ndim < 1
            or x.shape[-1] != FEATURES
            or not np.isfinite(x).all()
            or np.any((x < 0) | (x > 1))
        ):
            raise ValueError("AEC features must be finite and bounded")
        w1, b1, w2, b2 = self.arrays
        hidden = np.tanh(x @ w1.T + b1)
        return FLOOR + (1 - FLOOR) * expit(hidden @ w2 + b2)


def apply_masks(error_spectra, features, model, rate):
    """Causal band masks with 10 ms attack and immediate release, no lookahead."""
    spectrum = np.asarray(error_spectra).copy()
    proposal = model.predict(features)
    smooth = np.ones(BANDS)
    attack = np.exp(-256 / rate / 0.010)
    gains = np.ones_like(proposal)
    for block, values in enumerate(proposal):
        # Ref-silent blocks retain adaptive cancellation without learned cuts.
        # Feature 9 is reference power in [-120,0] dB mapped to [0,1].
        if features[block, 0, 9] <= 1 / 6:  # mean-square reference <= -100 dB
            values = np.ones(BANDS)
        smooth = np.where(
            values < smooth, attack * smooth + (1 - attack) * values, values
        )
        gains[block] = smooth
        for band, gain in enumerate(smooth):
            spectrum[block, band * 257 // BANDS : (band + 1) * 257 // BANDS] *= gain
    # Native error is [zeros(256), e(256)]; irfft includes exactly 1/512.
    audio = np.fft.irfft(spectrum, n=512, axis=1)[:, 256:].reshape(-1)
    return audio, gains
