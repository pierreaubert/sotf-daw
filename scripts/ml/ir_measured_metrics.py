"""Finite-record measured-reference IR metrics without alignment or level fitting."""

from __future__ import annotations

import numpy as np


def decay_error(reference, output, rate, arrival):
    """Compare normalized backward energy profiles where reference exceeds -40 dB."""
    remaining = len(reference) - arrival
    positions = arrival + np.unique(
        np.r_[
            0,
            np.round(
                np.geomspace(max(1, round(rate * 0.005)), remaining - 1, 64)
            ).astype(int),
        ]
    )
    target = np.cumsum(np.sum(reference.astype(float) ** 2, axis=1)[::-1])[::-1]
    observed = np.cumsum(np.sum(output.astype(float) ** 2, axis=1)[::-1])[::-1]
    target_db = 10 * np.log10(
        np.maximum(target[positions] / max(target[arrival], 1e-30), 1e-30)
    )
    observed_db = 10 * np.log10(
        np.maximum(observed[positions] / max(observed[arrival], 1e-30), 1e-30)
    )
    selected = target_db >= -40
    difference = observed_db[selected] - target_db[selected]
    return {
        "rmse_db": float(np.sqrt(np.mean(difference**2))),
        "max_abs_db": float(np.max(np.abs(difference))),
        "points": int(np.sum(selected)),
    }


def transfer_error(reference, output, rate):
    """Compare complex responses on a fixed log grid, excluding deep target nulls."""
    count = 1 << (len(reference) - 1).bit_length()
    grid = np.fft.rfftfreq(count, 1 / rate)
    positions = np.unique(
        np.searchsorted(grid, np.geomspace(40, min(16000, rate * 0.45), 192))
    )
    target = np.fft.rfft(reference.astype(float), n=count, axis=0)[positions]
    observed = np.fft.rfft(output.astype(float), n=count, axis=0)[positions]
    energetic = np.abs(target) >= np.max(np.abs(target), axis=0, keepdims=True) * 0.1
    ratio = observed / np.where(np.abs(target) > 1e-30, target, 1)
    magnitude = 20 * np.log10(np.maximum(np.abs(ratio[energetic]), 1e-30))
    phase = np.angle(ratio[energetic], deg=True)
    return {
        "magnitude_rmse_db": float(np.sqrt(np.mean(magnitude**2))),
        "phase_rmse_degrees": float(np.sqrt(np.mean(phase**2))),
        "magnitude_max_abs_db": float(np.max(np.abs(magnitude))),
        "phase_max_abs_degrees": float(np.max(np.abs(phase))),
        "points": int(np.sum(energetic)),
    }
