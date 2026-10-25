"""Synthetic clean/corrupt pairs; signal identity is split before variants."""

import hashlib

import numpy as np


def case(seed, kind, rate=48000, channels=1):
    rng = np.random.default_rng(seed)
    frames = round(rate * 0.2)
    t = np.arange(frames) / rate
    frequency = rng.uniform(80, 5000)
    background = rng.uniform(0.005, 0.08) * np.sin(
        2 * np.pi * frequency * t + rng.uniform(-np.pi, np.pi)
    )
    background += rng.uniform(0, 0.01) * rng.normal(size=frames)
    centre = frames // 2
    event = np.zeros(frames)
    if kind in ("click", "ambiguous-programme"):
        width = int(rng.integers(1, 5))
        event[centre : centre + width] = rng.choice([-1, 1]) * rng.uniform(0.3, 2)
    elif kind == "percussion":
        dt = np.arange(frames - centre) / rate
        amplitude = rng.uniform(0.3, 2)
        event[centre:] = (
            amplitude
            * rng.uniform(0.01, 0.06)
            * np.exp(-dt / rng.uniform(0.005, 0.04))
            * np.sin(2 * np.pi * rng.uniform(200, 6000) * dt)
        )
        width = int(rng.integers(1, 5))
        event[centre : centre + width] = rng.choice([-1, 1]) * amplitude
    elif kind == "attack":
        decay = rng.uniform(0.00005, 0.02)
        dt = np.arange(frames - centre) / rate
        event[centre:] = (
            rng.uniform(0.2, 1.5) * np.exp(-dt / decay) * rng.normal(size=len(dt))
        )
    elif kind == "step":
        event[centre : centre + round(rate * 0.01)] = rng.choice([-1, 1]) * rng.uniform(
            0.2, 1
        )
    elif kind != "clean":
        raise ValueError("Unknown synthetic case")
    clean = background if kind == "click" else background + event
    observed = background + event
    levels = np.linspace(1, 0.7, channels)
    clean = (clean[:, None] * levels).astype(np.float32)
    observed = (observed[:, None] * levels).astype(np.float32)
    identity = hashlib.sha256(background.astype("<f8").tobytes()).hexdigest()
    return (
        clean,
        observed,
        {
            "seed": seed,
            "kind": kind,
            "sample_rate": rate,
            "channels": channels,
            "signal_identity": identity,
            "centre_frame": centre,
            "frames": frames,
            "qualification": "Synthetic mathematical signal; no recording or perceptual annotation",
        },
    )
