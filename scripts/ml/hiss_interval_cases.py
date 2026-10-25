"""Synthetic source-grouped labels for functional interval classifier checks."""

import json
from pathlib import Path

import numpy as np
from hiss_intervals import FORMAT, digest
from scipy.signal import lfilter

CONTEXT = {
    "sample_rate": 48000,
    "channels": 1,
    "frequency_hz": 4000.0,
    "capture_setup_id": "synthetic-development-chain",
    "noise_definition": "unwanted_stationary_hiss",
}


def signal(kind, seed):
    rng = np.random.default_rng(seed)
    t = np.arange(48000) / 48000
    if kind == "noise_only":
        noise = rng.normal(size=48000)
        # Mildly coloured, stationary random hiss; gains are not label features.
        noise = lfilter([1, -rng.uniform(0, 0.4)], [1], noise)
        return (noise * rng.uniform(0.002, 0.02)).astype(np.float32)[:, None]
    frequency = rng.uniform(300, 7000)
    carrier = (
        0.7 * np.sin(2 * np.pi * frequency * t + rng.uniform(0, 6))
        + 0.25 * np.sin(2 * np.pi * frequency * 1.3 * t)
        + rng.normal(size=48000) * 0.01
    )
    levels = rng.uniform(0.05, 1, size=8)
    envelope = np.repeat(levels, 6000)
    return (carrier * envelope * rng.uniform(0.002, 0.02)).astype(np.float32)[:, None]


def session(root, seed=20721026, groups=8):
    root = Path(root)
    root.mkdir(parents=True, exist_ok=False)
    records = []
    for i in range(groups):
        for j, label in enumerate(["noise_only", "programme"]):
            x = signal(label, seed + i * 10 + j)
            path = root / f"source-{i}-{label}.f32le"
            raw = x.astype("<f4").tobytes()
            path.write_bytes(raw)
            records.append(
                {
                    "id": f"source-{i}-{label}",
                    "source_group": f"source-{seed + i}",
                    "label": label,
                    "audio_file": str(path.resolve()),
                    "audio_sha256": digest(raw),
                }
            )
    data = {
        "format": FORMAT,
        "data_origin": "synthetic_validation",
        "context": dict(CONTEXT),
        "protocol": {"whole_source_grouping": True},
        "records": records,
    }
    (root / "session.json").write_text(json.dumps(data, indent=2) + "\n")
    return data
