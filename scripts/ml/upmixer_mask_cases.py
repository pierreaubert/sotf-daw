"""Disjoint generated source identities for direct/ambient separation probes."""

import hashlib

import numpy as np
from upmixer_source_mask import RATE


def fixture(seed, kind="matched"):
    rng = np.random.default_rng(seed)
    frames = RATE // 2 + 17
    t = np.arange(frames) / RATE
    mono = np.zeros(frames)
    base = rng.uniform(110, 700)
    for h in range(1, 9):
        mono += np.sin(2 * np.pi * h * base * t + rng.uniform(-3, 3)) / h
    envelope = 0.3 + 0.7 * np.sin(2 * np.pi * rng.uniform(1, 4) * t) ** 2
    mono *= envelope * 0.04
    pan = rng.uniform(-0.7, 0.7)
    if kind == "hard_pan":
        pan = rng.choice([-1.0, 1.0])
    direct = mono[:, None] * np.sqrt([(1 - pan) / 2, (1 + pan) / 2])
    ambient = rng.normal(0, 1, (frames, 2))
    # Smooth independent broadband beds; no use of desired-source labels in features.
    for ch in range(2):
        ambient[:, ch] = np.convolve(ambient[:, ch], np.ones(5) / 5, mode="same")
    ambient *= rng.uniform(0.015, 0.08)
    if kind == "correlated_bed":
        ambient[:, 1] = ambient[:, 0]
    if kind == "antiphase_bed":
        ambient[:, 1] = -ambient[:, 0]
    if kind == "direct_only":
        ambient.fill(0)
    if kind == "ambient_only":
        direct.fill(0)
    if kind == "silence":
        direct.fill(0)
        ambient.fill(0)
    if kind == "level_shift":
        direct *= 0.01
        ambient *= 0.01
    source = direct + ambient
    identity = {
        "seed": seed,
        "kind": kind,
        "frames": frames,
        "sample_rate": RATE,
        "pan": float(pan),
        "input_sha256": hashlib.sha256(source.astype("<f8").tobytes()).hexdigest(),
        "direct_sha256": hashlib.sha256(direct.astype("<f8").tobytes()).hexdigest(),
        "ambient_sha256": hashlib.sha256(ambient.astype("<f8").tobytes()).hexdigest(),
    }
    return source, direct, ambient, identity
