"""Generated desired event tags, steady backgrounds and explicitly nuisance shifts."""

import hashlib

import numpy as np


def fixture(seed, rate=48000, kind="matched"):
    rng = np.random.default_rng(seed)
    n = 2 * rate + 17
    t = np.arange(n) / rate
    signal = rng.uniform(0.002, 0.008) * np.sin(
        2 * np.pi * rng.uniform(90, 800) * t + rng.uniform(0, 6)
    )
    signal += rng.normal(0, 0.0007, n)
    events = []
    count = 12 if kind == "dense_events" else 6
    if kind not in {"steady_only", "silence"}:
        for i in range(count):
            start = int(
                (0.18 + 1.55 * i / max(count - 1, 1) + rng.uniform(-0.025, 0.025))
                * rate
            )
            if kind == "phase_late":
                hop = round(0.002 * rate)
                start = (start // hop) * hop + hop - 1
            events.append(start)
            at = np.arange(n) - start
            attack = 0.008 if kind == "slow_attack" else rng.uniform(0.0002, 0.0012)
            envelope = (
                np.clip((at + 1) / (attack * rate), 0, 1)
                * np.exp(-np.maximum(at, 0) / (rng.uniform(0.025, 0.065) * rate))
                * (at >= 0)
            )
            carrier = (
                rng.normal(size=n)
                if i % 2
                else np.sin(
                    2 * np.pi * rng.uniform(180, 2400) * at / rate + rng.uniform(0, 6)
                )
            )
            signal += rng.uniform(0.04, 0.12) * envelope * carrier
    nuisance = []
    if kind == "noise_bursts":
        for start in [int(x * rate) for x in [0.42, 0.91, 1.41]]:
            nuisance.append(start)
            at = np.arange(n) - start
            signal += (
                0.09
                * rng.normal(size=n)
                * np.exp(-np.maximum(at, 0) / (0.018 * rate))
                * (at >= 0)
            )
    scale = 2.0 if kind == "level_shift" else 1.0
    signal *= scale
    if kind == "silence":
        signal.fill(0)
    if np.max(abs(signal)) > 0.9:
        correction = 0.9 / np.max(abs(signal))
        signal *= correction
        scale *= correction
    signal = signal.astype("<f4")
    return signal, {
        "seed": seed,
        "sample_rate": rate,
        "frames": n,
        "kind": kind,
        "desired_event_frames": events,
        "nuisance_event_frames": nuisance,
        "effective_gain_scale": scale,
        "input_sha256": hashlib.sha256(signal.tobytes()).hexdigest(),
        "source": "project-generated desired plucks/percussion and steady/noise signals; no recorded audio",
    }
