"""Fixed physical-rate/layout synthetic shifts and explicit negative controls."""

import json
from pathlib import Path

import numpy as np
from dynamic_eq_native import preset, render
from fit_dynamic_eq import FORMAT, digest

SPECS = [
    {
        "id": "mono-192k",
        "rate": 192000,
        "channels": 1,
        "bands": [{"frequency": 1500, "gain": -6}],
    },
    {
        "id": "mid-tilt-96k",
        "rate": 96000,
        "channels": 2,
        "bands": [{"shape": "tilt", "frequency": 1500, "gain": 4, "placement": "mid"}],
    },
    {
        "id": "side-high-shelf-44k1",
        "rate": 44100,
        "channels": 2,
        "linked": False,
        "bands": [
            {
                "shape": "high_shelf",
                "frequency": 4000,
                "gain": -5,
                "placement": "side",
                "shelf_slope": 0.7,
            }
        ],
    },
    {
        "id": "left-low-shelf-48k",
        "rate": 48000,
        "channels": 2,
        "bands": [
            {
                "shape": "low_shelf",
                "frequency": 500,
                "gain": 3,
                "placement": "left",
                "shelf_slope": 0.7,
            }
        ],
    },
    {
        "id": "custom-pairs-six-channel-96k",
        "rate": 96000,
        "channels": 6,
        "linked": False,
        "mix": 0.75,
        "pairs": [[0, 2], [1, 3], [4, 5]],
        "bands": [
            {"frequency": 800, "gain": -4, "placement": "mid"},
            {"shape": "tilt", "frequency": 1800, "gain": 2, "placement": "side"},
        ],
    },
    {
        "id": "eight-band-stereo-44k1",
        "rate": 44100,
        "channels": 2,
        "linked": False,
        "mix": 0.85,
        "knee": 4.0,
        "bands": [
            {
                "shape": ["peak", "low_shelf", "high_shelf", "tilt"][i % 4],
                "placement": ["stereo", "left", "right", "mid", "side"][i % 5],
                "frequency": 450 + 600 * i,
                "q": 1.0,
                "shelf_slope": 0.7,
                "gain": 2 * (-1 if i % 2 else 1),
            }
            for i in range(8)
        ],
    },
    {
        "id": "silent-channel-corruption",
        "rate": 48000,
        "channels": 8,
        "linked": False,
        "known_initial": True,
        "bands": [{"frequency": 1000, "gain": -6}],
    },
    {
        "id": "unexcited-side",
        "rate": 48000,
        "channels": 2,
        "bands": [{"frequency": 1000, "gain": -6, "placement": "side"}],
    },
]


def case(root, renderer, spec, seed):
    root = Path(root)
    root.mkdir(parents=True, exist_ok=False)
    common = {
        "linked": spec.get("linked", True),
        "mix": spec.get("mix", 1.0),
        "knee": spec.get("knee", 0.0),
        "pairs": spec.get("pairs"),
    }
    hidden = preset(
        spec["bands"],
        threshold=-23.0,
        ratio=4.5,
        attack_ms=3.5,
        release_ms=80.0,
        **common,
    )
    template = hidden if spec.get("known_initial") else preset(spec["bands"], **common)
    clips = []
    rate = spec["rate"]
    channels = spec["channels"]
    frames = rate // 4 + 1
    for i in range(3):
        rng = np.random.default_rng(seed + i)
        t = np.arange(frames) / rate
        x = np.zeros((frames, channels), dtype=np.float32)
        for ch in range(channels):
            levels = rng.choice([0.006, 0.02, 0.06, 0.18, 0.5], size=16)
            envelope = np.repeat(levels, int(np.ceil(frames / 16)))[:frames]
            carrier = (
                0.55 * np.sin(2 * np.pi * 1000 * t + rng.uniform(0, 6))
                + 0.25 * np.sin(2 * np.pi * 400 * t + rng.uniform(0, 6))
                + 0.15 * np.sin(2 * np.pi * 5100 * t + rng.uniform(0, 6))
                + rng.normal(size=frames) * 0.05
            )
            x[:, ch] = (envelope * carrier).astype(np.float32)
        if spec["id"] == "unexcited-side":
            x[:, 1] = x[:, 0]
        if spec["id"] == "silent-channel-corruption":
            x[:, -1] = 0
        source = root / f"input-{i}.f32le"
        source.write_bytes(x.astype("<f4").tobytes())
        reference, _ = render(x, renderer, root / f"truth-{i}", hidden, rate=rate)
        if spec["id"] == "silent-channel-corruption":
            reference[:, -1] = rng.normal(size=frames) * 0.00015
        target = root / f"reference-{i}.f32le"
        target.write_bytes(reference.astype("<f4").tobytes())
        clips.append(
            {
                "id": f"whole-source-{seed + i}",
                "role": "validation" if i == 2 else "train",
                "frames": frames,
                "input_file": str(source.resolve()),
                "input_sha256": digest(source.read_bytes()),
                "reference_file": str(target.resolve()),
                "reference_sha256": digest(target.read_bytes()),
            }
        )
    data = {
        "format": FORMAT,
        "data_origin": "synthetic_validation",
        "alignment": "sample_aligned_zero_latency_fresh_state",
        "sample_rate": rate,
        "channels": channels,
        "template": template,
        "clips": clips,
    }
    (root / "target.json").write_text(json.dumps(data, indent=2) + "\n")
    (root / "truth.json").write_text(
        json.dumps(
            {
                "spec": spec,
                "seed": seed,
                "hidden_parameters": hidden,
                "qualification": "Fixed synthetic shifts; silent-channel corruption is an impossible-output negative control for independent Stereo bands and starts at true dynamics to isolate admission. Orthogonal Side input has no Side excitation.",
            },
            indent=2,
        )
        + "\n"
    )
    return data
