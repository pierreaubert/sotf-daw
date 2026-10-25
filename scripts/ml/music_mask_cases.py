"""Reproducible generated musical parts and noise; no recorded-music claim."""

import hashlib

import numpy as np
from scipy.signal import lfilter


def fixture(seed, rate=48000, seconds=2.25, kind="matched"):
    rng = np.random.default_rng(seed)
    frames = int(rate * seconds) + 17
    t = np.arange(frames) / rate
    clean = np.zeros(frames)
    root = rng.integers(45, 66)
    notes = root + np.array([0, 4, 7, 12, 16, 19])
    for i in range(12):
        start = 0.25 + rng.uniform(0, max(seconds - 0.65, 0.1))
        duration = rng.uniform(0.12, 0.65)
        position = t - start
        active = (position >= 0) & (position < duration)
        frequency = 440 * 2 ** ((rng.choice(notes) - 69) / 12)
        vibrato = 5 * np.sin(2 * np.pi * 5.7 * position) if kind == "vibrato" else 0
        phase = (
            2 * np.pi * np.cumsum(frequency + vibrato) / rate
            if kind == "vibrato"
            else 2 * np.pi * frequency * position
        )
        envelope = np.minimum(np.maximum(position, 0) / 0.008, 1) * np.exp(
            -np.maximum(position, 0) / (0.1 + duration)
        )
        harmonics = sum(
            np.sin(h * phase + rng.uniform(0, 2 * np.pi)) / h ** rng.uniform(1.1, 2.1)
            for h in range(1, 9)
        )
        clean += active * envelope * harmonics * rng.uniform(0.3, 0.9)
    # Wideband percussive content is desired music, not corruption to be removed.
    for i in range(8 if kind == "dense_percussion" else 3):
        start = 0.3 + rng.uniform(0, max(seconds - 0.5, 0.1))
        position = t - start
        envelope = np.exp(-np.maximum(position, 0) / 0.025) * (position >= 0)
        clean += rng.normal(size=frames) * envelope * 0.4
    clean *= rng.uniform(0.015, 0.04) / max(np.sqrt(np.mean(clean**2)), 1e-12)
    noise = rng.normal(size=frames)
    if seed % 3 == 1:
        noise = lfilter([1], [1, -0.94], noise)
    if seed % 3 == 2:
        noise = (
            0.6 * noise
            + 0.4 * np.sin(2 * np.pi * 60 * t)
            + 0.2 * np.sin(2 * np.pi * 120 * t)
        )
    noise *= rng.uniform(0.005, 0.025) / max(np.sqrt(np.mean(noise**2)), 1e-12)
    if kind == "step_noise":
        noise[t >= seconds / 2] *= 4
    if kind == "tone_noise":
        noise += 0.02 * np.sin(2 * np.pi * 1379 * t)
    if kind == "clean_only":
        noise.fill(0)
    mixture = (clean + noise).astype("<f4")
    clean = clean.astype("<f4")
    noise = noise.astype("<f4")
    if max(np.max(abs(clean)), np.max(abs(mixture))) >= 0.999:
        raise ValueError("Generated fixture lacks headroom")
    return (
        mixture,
        clean,
        noise,
        {
            "seed": seed,
            "sample_rate": rate,
            "frames": frames,
            "kind": kind,
            "source": "generated polyphonic harmonic parts and desired percussive noise bursts",
            "mixture_sha256": hashlib.sha256(mixture.tobytes()).hexdigest(),
            "clean_sha256": hashlib.sha256(clean.tobytes()).hexdigest(),
            "noise_sha256": hashlib.sha256(noise.tobytes()).hexdigest(),
        },
    )
