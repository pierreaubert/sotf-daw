"""Frozen pre-normalized activation corpus shared byte-for-byte by each runtime."""

import numpy as np


def corpus():
    rng = np.random.default_rng(20309120)
    t = np.arange(1024) / 16000
    frames, labels = [], []
    for hz in [65.4, 220.0, 440.0, 1318.5]:
        for kind in [
            "tone",
            "weak_fundamental",
            "vibrato",
            "noisy_tone",
            "onset",
            "chirp",
            "noise",
        ]:
            phase = 2 * np.pi * hz * t + rng.uniform(-np.pi, np.pi)
            x = np.sin(phase)
            if kind == "weak_fundamental":
                x = 0.001 * x + 0.7 * np.sin(2 * phase) + 0.3 * np.sin(3 * phase)
            elif kind == "vibrato":
                f = hz * 2 ** (20 * np.sin(2 * np.pi * 5 * t) / 1200)
                x = np.sin(np.cumsum(2 * np.pi * f / 16000))
            elif kind == "noisy_tone":
                x += rng.normal(0, 0.3, 1024)
            elif kind == "onset":
                x *= np.clip((t - 0.02) / 0.005, 0, 1)
            elif kind == "chirp":
                x = np.sin(phase + 2 * np.pi * hz * 0.1 * t**2)
            elif kind == "noise":
                x = rng.normal(0, 1, 1024)
            # Match the contract's sample-standard-deviation normalization.
            x = (x - x.mean()) / max(float(x.std(ddof=1)), 1e-10)
            frames.append(np.asarray(x, dtype="<f4"))
            labels.append({"kind": kind, "reference_hz": hz})
    for name, x in [
        ("zero", np.zeros(1024)),
        ("impulse", np.eye(1, 1024, 512)[0]),
        ("ramp", np.linspace(-1, 1, 1024)),
        ("alternating", (-1.0) ** np.arange(1024)),
    ]:
        x = (x - x.mean()) / max(float(x.std(ddof=1)), 1e-10)
        frames.append(np.asarray(x, dtype="<f4"))
        labels.append({"kind": name, "reference_hz": 220.0})
    return np.stack(frames), labels
