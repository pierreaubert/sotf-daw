"""Actual native shared coloration baseline for calibrated hardware comparisons."""

import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np

NAMES = ["Harmonics", "Static", "Hammerstein", "Tape", "Transformer", "Console Preamp"]


def controls(**changes):
    p = {
        "model_id": 0,
        "drive_db": 0.0,
        "color": 1.0,
        "character": 0.5,
        "output_trim_db": 0.0,
    }
    if set(changes) - set(p):
        raise ValueError("Unknown native coloration controls")
    return dict(p, **changes)


def render(
    signal, renderer, work, parameters, rate=48000, callback=193, maximum=127, zeros=0
):
    x = np.asarray(signal, dtype=np.float32)
    if (
        x.ndim != 2
        or not 1 <= x.shape[1] <= 32
        or not len(x)
        or not np.isfinite(x).all()
        or np.max(abs(x)) > 16
        or type(rate) is not int
        or not 44100 <= rate <= 192000
        or any(type(v) is not int or not 1 <= v <= 4096 for v in [callback, maximum])
        or type(zeros) is not int
        or not 0 <= zeros <= 65536
        or (len(x) + zeros) * x.shape[1] > 4_000_000
    ):
        raise ValueError("Unsupported bounded native coloration geometry")
    if not isinstance(parameters, dict) or set(parameters) != set(controls()):
        raise ValueError("Supply complete native coloration controls")
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
        "prepared_maximum_frames": maximum,
        "appended_zero_frames": zeros,
        "controls": parameters,
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
        raise ValueError("Native coloration failed: " + p.stderr)
    m = json.loads(p.stdout)
    expected = {
        "status": "ok",
        "sample_rate": rate,
        "channels": x.shape[1],
        "input_frames": len(x),
        "output_frames": len(x) + zeros,
        "callback_frames": callback,
        "prepared_maximum_frames": maximum,
        "appended_zero_frames": zeros,
        "latency_samples": 0,
        "tail": "not_claimed_finite",
        "nominal_reference_level_dbfs": -18.0,
    }
    if any(m.get(k) != v for k, v in expected.items()):
        raise ValueError("Native coloration context mismatch")
    if set(m["effective_controls"]) != set(parameters) or any(
        np.float32(m["effective_controls"][k]) != np.float32(v)
        for k, v in parameters.items()
    ):
        raise ValueError("Native coloration controls mismatch")
    model = m["effective_controls"]["model_id"]
    if m["model_name"] != NAMES[model]:
        raise ValueError("Native model identity mismatch")
    path = work / "output.f32le"
    if path.stat().st_size != (len(x) + zeros) * x.shape[1] * 4:
        raise ValueError("Native PCM byte count mismatch")
    y = np.fromfile(path, dtype="<f4").reshape(-1, x.shape[1])
    if not np.isfinite(y).all() or np.max(abs(y)) > 128:
        raise ValueError("Native output exceeds finite safety rails")
    if np.float32(m["sample_peak"]) != np.max(abs(y)):
        raise ValueError("Native output peak mismatch")
    m.update(
        input_sha256=hashlib.sha256(raw).hexdigest(),
        output_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
    )
    (work / "metadata.json").write_text(json.dumps(m, indent=2, allow_nan=False) + "\n")
    return y, m
