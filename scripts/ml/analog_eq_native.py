"""Actual native four-band Analog EQ renderer for paired hardware fitting."""

import hashlib
import json
import math
import subprocess
from pathlib import Path

import numpy as np
from analog_color_native import NAMES

RANGES = {
    "low_freq": (20, 500),
    "low_gain": (-24, 24),
    "mid1_freq": (100, 5000),
    "mid1_gain": (-24, 24),
    "mid1_q": (0.1, 10),
    "mid2_freq": (500, 12000),
    "mid2_gain": (-24, 24),
    "mid2_q": (0.1, 10),
    "high_freq": (2000, 20000),
    "high_gain": (-24, 24),
    "analog_drive": (-60, 36),
    "analog_color": (0, 1),
    "analog_character": (0, 1),
    "analog_trim": (-24, 24),
}


def preset(**changes):
    p = {
        "low_freq": 100.0,
        "low_gain": 0.0,
        "mid1_freq": 800.0,
        "mid1_gain": 0.0,
        "mid1_q": 1.0,
        "mid2_freq": 3000.0,
        "mid2_gain": 0.0,
        "mid2_q": 1.0,
        "high_freq": 10000.0,
        "high_gain": 0.0,
        "analog_model": "Harmonics",
        "analog_drive": 0.0,
        "analog_color": 0.0,
        "analog_character": 0.5,
        "analog_trim": 0.0,
    }
    if set(changes) - set(p):
        raise ValueError("Unknown native Analog EQ controls")
    return dict(p, **changes)


def validate(parameters):
    if not isinstance(parameters, dict) or set(parameters) != set(preset()):
        raise ValueError("Supply complete native Analog EQ parameters")
    if parameters["analog_model"] not in NAMES:
        raise ValueError("Use canonical native coloration models")
    for key, (low, high) in RANGES.items():
        value = parameters[key]
        if (
            isinstance(value, bool)
            or not isinstance(value, (int, float))
            or not math.isfinite(value)
            or not low <= value <= high
        ):
            raise ValueError("Invalid native Analog EQ parameter: " + key)


def render(signal, renderer, work, parameters, *, rate=48000, callback=193, zeros=0):
    validate(parameters)
    x = np.asarray(signal, dtype=np.float32)
    if (
        x.ndim != 2
        or not 1 <= x.shape[1] <= 32
        or not len(x)
        or not np.isfinite(x).all()
        or np.max(abs(x)) > 16
        or type(rate) is not int
        or not 44100 <= rate <= 192000
        or type(callback) is not int
        or not 1 <= callback <= 4096
        or type(zeros) is not int
        or not 0 <= zeros <= 65536
        or (len(x) + zeros) * x.shape[1] > 4_000_000
    ):
        raise ValueError("Unsupported bounded native Analog EQ geometry")
    work = Path(work)
    work.mkdir(parents=True, exist_ok=False)
    raw = x.astype("<f4").tobytes()
    (work / "input.f32le").write_bytes(raw)
    request = {
        "input": str((work / "input.f32le").resolve()),
        "output": str((work / "output.f32le").resolve()),
        "sample_rate": rate,
        "channels": x.shape[1],
        "frames": len(x),
        "callback_frames": callback,
        "appended_zero_frames": zeros,
        "parameters": parameters,
    }
    source = work / "request.json"
    source.write_text(json.dumps(request, allow_nan=False) + "\n")
    p = subprocess.run(
        [str(Path(renderer).resolve()), str(source)],
        check=False,
        capture_output=True,
        text=True,
        timeout=120,
    )
    (work / "stderr.txt").write_text(p.stderr)
    if p.returncode:
        raise ValueError("Native Analog EQ failed: " + p.stderr)
    m = json.loads(p.stdout)
    expected = {
        "status": "ok",
        "sample_rate": rate,
        "channels": x.shape[1],
        "input_frames": len(x),
        "output_frames": len(x) + zeros,
        "callback_frames": callback,
        "appended_zero_frames": zeros,
        "latency_samples": 0,
        "tail": "unknown_recursive",
    }
    if any(m.get(k) != v for k, v in expected.items()):
        raise ValueError("Native Analog EQ context mismatch")
    effective = dict(parameters)
    for k in ["analog_drive", "analog_color", "analog_character", "analog_trim"]:
        effective[k] = float(np.float32(effective[k]))
    if m["effective_params"] != effective:
        raise ValueError("Native Analog EQ controls mismatch")
    path = work / "output.f32le"
    if path.stat().st_size != (len(x) + zeros) * x.shape[1] * 4:
        raise ValueError("Native Analog EQ PCM length mismatch")
    y = np.fromfile(path, dtype="<f4").reshape(-1, x.shape[1])
    if not np.isfinite(y).all() or np.max(abs(y)) > 128:
        raise ValueError("Native Analog EQ violates software rails")
    m.update(
        input_sha256=hashlib.sha256(raw).hexdigest(),
        output_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
    )
    (work / "metadata.json").write_text(json.dumps(m, indent=2, allow_nan=False) + "\n")
    return y, m
