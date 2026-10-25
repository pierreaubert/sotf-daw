"""Fresh synthetic native paired targets; no hidden controls enter fitter data."""

import json
from pathlib import Path

import numpy as np
from dynamic_eq_native import preset, render
from fit_dynamic_eq import FORMAT, digest


def case(root, renderer, kind="near", seed=20591026):
    root = Path(root)
    root.mkdir(parents=True, exist_ok=False)
    bands = [{"frequency": 1000, "q": 0.7, "gain": -6}]
    if kind == "two-band":
        bands = [
            {"frequency": 400, "q": 1, "gain": -4},
            {"shape": "high_shelf", "frequency": 4000, "shelf_slope": 0.7, "gain": 3},
        ]
    template = preset(bands)
    true = {"threshold": -23.0, "ratio": 4.5, "attack_ms": 3.5, "release_ms": 80.0}
    if kind == "far":
        true = {"threshold": -32.0, "ratio": 7.0, "attack_ms": 0.8, "release_ms": 180.0}
    hidden = preset(bands, **true)
    clips = []
    for i in range(3):
        rng = np.random.default_rng(seed + i)
        rate = 48000
        frames = 24001
        t = np.arange(frames) / rate
        levels = rng.choice([0.006, 0.02, 0.06, 0.18, 0.5], size=16)
        envelope = np.repeat(levels, 1501)[:frames]
        carrier = (
            0.65 * np.sin(2 * np.pi * 1000 * t + rng.uniform(0, 6))
            + 0.2 * np.sin(2 * np.pi * 400 * t)
            + 0.1 * np.sin(2 * np.pi * 5100 * t)
            + rng.normal(size=frames) * 0.05
        )
        x = (envelope * carrier).astype(np.float32)[:, None]
        source = root / f"clip-{i}.f32le"
        source.write_bytes(x.astype("<f4").tobytes())
        reference, _meta = render(x, renderer, root / f"truth-{i}", hidden)
        if kind == "dry":
            reference = x.copy()
        if kind == "shifted":
            reference = np.vstack(
                [np.zeros((48, 1), dtype=np.float32), reference[:-48]]
            )
        path = root / f"reference-{i}.f32le"
        path.write_bytes(reference.astype("<f4").tobytes())
        clips.append(
            {
                "id": f"source-{seed + i}",
                "role": "validation" if i == 2 else "train",
                "frames": frames,
                "input_file": str(source.resolve()),
                "input_sha256": digest(source.read_bytes()),
                "reference_file": str(path.resolve()),
                "reference_sha256": digest(path.read_bytes()),
            }
        )
    data = {
        "format": FORMAT,
        "data_origin": "synthetic_validation",
        "alignment": "sample_aligned_zero_latency_fresh_state",
        "sample_rate": 48000,
        "channels": 1,
        "template": template,
        "clips": clips,
    }
    (root / "target.json").write_text(json.dumps(data, indent=2) + "\n")
    (root / "truth.json").write_text(
        json.dumps(
            {
                "kind": kind,
                "hidden_parameters": hidden,
                "seed": seed,
                "scope": "Native synthetic reconstruction only; held validation signal has a separate seed. Hidden dynamics are not included in target input.",
            },
            indent=2,
        )
        + "\n"
    )
    return data
