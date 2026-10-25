"""Identity-grouped synthetic native limiter pairs, never external recordings."""

import hashlib

import numpy as np
from limiter_native import PARAMS


def signal(seed, index, rate=48000, channels=None, family=None, seconds=4.0):
    rng = np.random.default_rng(np.random.SeedSequence([seed, index]))
    channels = 1 + index % 2 if channels is None else channels
    family = index % 3 if family is None else family
    t = np.arange(round(rate * seconds)) / rate
    frequency = rng.uniform(100, 900)
    phase = rng.uniform(-np.pi, np.pi)
    if family == 0:
        carrier = np.sin(2 * np.pi * frequency * t + phase)
    elif family == 1:
        carrier = (
            np.sin(2 * np.pi * frequency * t + phase)
            + 0.4 * np.sin(2 * np.pi * frequency * 2.23 * t)
        ) / 1.4
    elif family == 2:
        carrier = np.clip(rng.normal(0, 0.45, len(t)), -1, 1)
    elif family == 3:
        carrier = np.sin(2 * np.pi * frequency * t + 30 * np.sin(2 * np.pi * 2.1 * t))
    elif family == 4:
        carrier = np.where(np.sin(2 * np.pi * frequency * t) >= 0, 1.0, -1.0)
    else:
        raise ValueError("Unknown synthetic family")
    threshold = float(rng.uniform(-12, -1))
    ceiling = 10 ** (threshold / 20)
    amplitude = np.full(len(t), ceiling * rng.uniform(0.12, 0.5))
    for start, duration in [(0.1, 0.17), (0.8, 0.1), (1.5, 0.16)]:
        start += rng.uniform(-0.03, 0.03)
        active = (t >= start) & (t < start + duration)
        amplitude[active] = ceiling * rng.uniform(2, 5)
    x = carrier * amplitude
    # Asymmetric channels exercise full linking without introducing unpaired IDs.
    x = np.stack(
        [x * (1 - 0.09 * c) * (-1 if c % 2 else 1) for c in range(channels)], axis=1
    ).astype(np.float32)
    fixed = {k: v for k, v in PARAMS.items() if k != "release_ms"}
    fixed.update(
        threshold_db=threshold, lookahead_ms=float(rng.choice([0, 1, 5, 10, 20]))
    )
    identity = hashlib.sha256(x.astype("<f4").tobytes()).hexdigest()
    releases = 10 ** rng.uniform(1, 3, 2)
    return x, fixed, identity, releases.tolist()
