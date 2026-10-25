"""Bound offline IR attenuation using per-channel noise-reference energy.

The mask floor is 1 - min(0.95, alpha * reference_noise_energy / observed_energy)
inside each short window. A common floor is the largest channel floor. Removed
energy in each channel/window is therefore no greater than alpha times its
reference noise energy in exact arithmetic. The exported f32 result is checked
again. This is a stationary-noise-relative bound, not proof of clean IR identity.
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import numpy as np

FORMAT = "sotf_ir_preservation_policy_v1"


def validate_policy(policy):
    if not isinstance(policy, dict) or set(policy) != {
        "format",
        "window_ms",
        "noise_fraction",
    }:
        raise ValueError("invalid preservation policy fields")
    if policy["format"] != FORMAT:
        raise ValueError("unsupported preservation policy format")
    for name, low, high in [("window_ms", 1.0, 20.0), ("noise_fraction", 0.01, 1.0)]:
        value = policy[name]
        if (
            isinstance(value, bool)
            or not isinstance(value, (float, int))
            or not np.isfinite(value)
            or not low <= value <= high
        ):
            raise ValueError("invalid preservation policy bounds")
    return policy


def load_policy(path, expected):
    path = Path(path)
    with path.open("rb") as handle:
        raw = handle.read(8193)
    if len(raw) > 8192 or hashlib.sha256(raw).hexdigest() != expected:
        raise ValueError("preservation policy identity or size mismatch")
    return validate_policy(json.loads(raw))


def removed_noise_ratio(original, candidate, rate, arrival, noise_frames, policy):
    """Measure actual f32 exported changes against each prefix-derived budget."""
    validate_policy(policy)
    width = max(1, round(rate * policy["window_ms"] / 1000))
    protected = arrival + round(rate * 0.05)
    noise = np.mean(original[:noise_frames] ** 2, axis=0)
    maximum, windows = 0.0, 0
    for first in range(protected, len(original), width):
        last = min(first + width, len(original))
        removed = np.sum(
            (candidate[first:last].astype(float) - original[first:last]) ** 2, axis=0
        )
        budget = noise * (last - first) * policy["noise_fraction"]
        ratio = np.divide(removed, budget, out=np.zeros_like(removed), where=budget > 0)
        if np.any((budget == 0) & (removed > 0)):
            return float("inf"), windows + 1
        maximum = max(maximum, float(np.max(ratio)))
        windows += 1
    return maximum, windows


def preserve_tail(samples, rate, arrival, noise_frames, mask, policy):
    """Raise linked gains to meet local, per-channel noise-energy budgets."""
    validate_policy(policy)
    samples = np.asarray(samples, dtype=float)
    mask = np.asarray(mask, dtype=float).copy()
    if (
        samples.ndim != 2
        or mask.shape != (len(samples),)
        or not np.isfinite(samples).all()
        or not np.isfinite(mask).all()
        or np.any(mask < 0.05 - 1e-12)
        or np.any(mask > 1 + 1e-12)
        or not 0 < noise_frames <= arrival < len(samples)
        or not 8000 <= rate <= 96000
    ):
        raise ValueError("invalid preservation input")
    width = max(1, round(rate * policy["window_ms"] / 1000))
    protected = arrival + round(rate * 0.05)
    noise = np.mean(samples[:noise_frames] ** 2, axis=0)
    mask[:protected] = 1
    for first in range(protected, len(samples), width):
        last = min(first + width, len(samples))
        observed = np.sum(samples[first:last] ** 2, axis=0)
        budget = noise * (last - first) * policy["noise_fraction"]
        fraction = np.divide(
            budget, observed, out=np.zeros_like(observed), where=observed > 0
        )
        floor = float(np.max(1 - np.minimum(0.95, fraction)))
        mask[first:last] = np.maximum(mask[first:last], floor)
    candidate = (samples * mask[:, None]).astype("<f4")
    maximum, windows = removed_noise_ratio(
        samples, candidate, rate, arrival, noise_frames, policy
    )
    return (
        candidate,
        mask,
        {
            "policy": policy,
            "max_removed_noise_ratio": maximum if np.isfinite(maximum) else None,
            "windows": windows,
            "within_budget": bool(np.isfinite(maximum) and maximum <= 1.00001),
            "scope": "Per-channel, nonoverlapping windows after protected early response; prefix noise must represent the IR noise. Not a clean-signal error bound.",
        },
    )
