"""Known-phase mono pitch trajectories; generated probes, not held-out music."""

import hashlib

import numpy as np

KINDS = (
    "offset",
    "wow",
    "flutter",
    "vibrato",
    "step",
    "weak_fundamental",
    "noisy_wow",
    "lone_octave",
)


def fixture(seed, kind, rate=48000, seconds=3):
    rng = np.random.default_rng(seed)
    frames = int(rate * seconds) + 17
    t = np.arange(frames) / rate
    reference = 220.0 if seed % 2 == 0 else 440.0
    cents = np.zeros(frames)
    if kind == "offset":
        cents += 15
    elif kind in {"wow", "weak_fundamental", "noisy_wow"}:
        cents = 20 * np.sin(2 * np.pi * 0.7 * t)
    elif kind == "flutter":
        cents = 5 * np.sin(2 * np.pi * 6 * t)
    elif kind == "vibrato":
        cents = 20 * np.sin(2 * np.pi * 5 * t)
    elif kind == "step":
        cents[t >= 1] = 25
    elif kind != "lone_octave":
        raise ValueError("Unknown PND comparison condition")
    phase = (
        rng.uniform(0, 2 * np.pi)
        + np.r_[0, np.cumsum(2 * np.pi * reference * 2 ** (cents[:-1] / 1200) / rate)]
    )
    partial = 1
    if kind == "weak_fundamental":
        x = 0.0001 * np.sin(phase) + 0.2 * np.sin(2 * phase) + 0.1 * np.sin(3 * phase)
        partial = 2
    elif kind == "lone_octave":
        x = 0.2 * np.sin(2 * phase)
        partial = 2
    else:
        x = 0.2 * np.sin(phase)
    if kind == "noisy_wow":
        x += rng.normal(0, 0.03, frames)
    fade = np.minimum(1, np.minimum(t, t[-1] - t) / 0.02)
    x = np.asarray(x * fade, dtype="<f4")
    intended = cents.copy() if kind == "vibrato" else np.zeros(frames)
    identity = {
        "seed": seed,
        "kind": kind,
        "sample_rate": rate,
        "frames": frames,
        "reference_hz": reference,
        "measurement_partial": partial,
        "supported_fundamental_truth": kind != "lone_octave",
        "input_sha256": hashlib.sha256(x.tobytes()).hexdigest(),
        "truth_cents_sha256": hashlib.sha256(cents.astype("<f8").tobytes()).hexdigest(),
    }
    return x, cents, intended, identity
