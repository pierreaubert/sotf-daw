"""Synthetic cross-memory positive control, never a physical hardware oracle."""

import copy
import hashlib
import json
from pathlib import Path

import numpy as np
from test_analog_hardware import corpus


def session(work, kind="cross-memory"):
    work = Path(work)
    work.mkdir(parents=True, exist_ok=False)
    template = corpus(work)
    data = copy.deepcopy(template)
    data["context"]["hardware"]["model"] = "synthetic-" + kind
    data["captures"] = []
    for i, split in enumerate(["train"] * 8 + ["validation"] * 2 + ["test"] * 2):
        rng = np.random.default_rng(20261102 + i)
        x = np.clip(rng.normal(0, 0.12, 12001 + i), -0.4, 0.4).astype("<f4")
        if i == 0:
            x[:2] = [0.42, -0.42]  # Explicit generated operating-domain anchors.
        previous = np.r_[0.0, x[:-1]]
        older = np.r_[0.0, 0.0, x[:-2]]
        if kind == "cross-memory":
            y = (
                0.6 * np.tanh(6 * x + 4 * previous)
                - 0.3 * np.tanh(5 * x - 3 * older)
                + 0.2 * x
                + 0.005
            )
        elif kind == "hammerstein":
            y = 0.7 * x + 0.15 * previous + 0.4 * x**3
        else:
            raise ValueError("Unknown generated functional target")
        y = y.astype("<f4")
        r = {
            "id": f"record-{i}",
            "source_group": f"source-{i}",
            "split": split,
            "frames": len(x),
        }
        for name, signal in [("dry", x), ("wet", y)]:
            path = work / f"{name}-record-{i}.f32le"
            path.write_bytes(signal.tobytes())
            r[name + "_file"] = str(path.resolve())
            r[name + "_sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
        data["captures"].append(r)
    (work / "manifest.json").write_text(json.dumps(data, indent=2) + "\n")
    return data
