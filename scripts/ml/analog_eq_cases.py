"""Generated native EQ/color targets with explicit synthetic calibration labels."""

import copy
import hashlib
import json
from pathlib import Path

import numpy as np
from analog_eq_native import preset, render
from fit_analog_eq_hardware import FORMAT
from test_analog_hardware import corpus


def session(work, renderer, kind="matched"):
    work = Path(work)
    work.mkdir(parents=True, exist_ok=False)
    template = corpus(work)
    data = copy.deepcopy(template)
    data["captures"] = []
    data["context"]["hardware"]["model"] = "synthetic-analog-eq-" + kind
    target = preset(
        analog_model="Static",
        low_gain=2.25,
        high_gain=-1.75,
        analog_drive=6,
        analog_color=0.45,
        analog_character=0.31,
        analog_trim=-1,
    )
    for i, split in enumerate(["train"] * 8 + ["validation"] * 2 + ["test"] * 2):
        rng = np.random.default_rng(20261113 + i)
        frames = 12001 + i * 7
        t = np.arange(frames) / 48000
        x = (
            rng.normal(0, 0.01 + 0.002 * i, frames)
            + 0.015 * np.sin(2 * np.pi * (173 + 31 * i) * t)
        ).astype("<f4")
        y, _ = render(x[:, None], renderer, work / f"reference-{i}", target)
        wet = y[:, 0].copy()
        if kind == "delay":
            wet = np.r_[np.zeros(10, dtype=np.float32), wet[:-10]]
        elif kind != "matched":
            raise ValueError("Unknown generated EQ target")
        row = {
            "id": f"record-{i}",
            "source_group": f"source-{i}",
            "split": split,
            "frames": frames,
        }
        for name, values in [("dry", x), ("wet", wet)]:
            path = work / f"{name}-record-{i}.f32le"
            path.write_bytes(values.astype("<f4").tobytes())
            row[name + "_file"] = str(path.resolve())
            row[name + "_sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
        data["captures"].append(row)
    manifest = work / "captures.json"
    manifest.write_text(json.dumps(data, indent=2) + "\n")
    initial = preset(
        analog_model="Harmonics",
        analog_drive=6,
        analog_color=0.2,
        analog_character=0.31,
        analog_trim=-1,
    )
    spec = {
        "format": FORMAT,
        "start_state": "quiescent",
        "capture_manifest": str(manifest.resolve()),
        "capture_manifest_sha256": hashlib.sha256(manifest.read_bytes()).hexdigest(),
        "renderer_sha256": hashlib.sha256(Path(renderer).read_bytes()).hexdigest(),
        "template": initial,
        "models": ["Harmonics", "Static"],
        "free": {
            "low_gain": {"minimum": -4.0, "maximum": 4.0, "scale": "linear"},
            "high_gain": {"minimum": -4.0, "maximum": 4.0, "scale": "linear"},
            "analog_color": {"minimum": 0.0, "maximum": 1.0, "scale": "linear"},
        },
    }
    (work / "fit-spec.json").write_text(json.dumps(spec, indent=2) + "\n")
    return spec, data, target
