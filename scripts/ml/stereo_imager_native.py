"""Actual native Stereo Imager renders for offline learned-setting qualification."""

import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np


def preset(**changes):
    result = {
        "width": 1.0,
        "low_mid_freq": 250.0,
        "mid_high_freq": 4000.0,
        "low_width": 1.0,
        "mid_width": 1.0,
        "high_width": 1.0,
        "mono_bass": False,
        "mix": 1.0,
    }
    if set(changes) - set(result):
        raise ValueError("Unknown native Stereo Imager controls")
    return dict(result, **changes)


def render(signal, renderer, work, params, rate=48000, callback=193, zeros=0):
    x = np.asarray(signal, dtype=np.float32)
    if (
        x.ndim != 2
        or x.shape[1] != 2
        or not 1 <= x.size <= 4_000_000
        or not np.isfinite(x).all()
        or np.max(abs(x)) > 32
        or type(rate) is not int
        or not 44100 <= rate <= 192000
        or type(callback) is not int
        or not 1 <= callback <= 4096
        or type(zeros) is not int
        or not 0 <= zeros <= 65536
    ):
        raise ValueError("Use supported finite bounded stereo geometry")
    work = Path(work)
    work.mkdir(parents=True, exist_ok=False)
    raw = x.astype("<f4").tobytes()
    (work / "input.f32le").write_bytes(raw)
    request = {
        "input": str((work / "input.f32le").resolve()),
        "output": str((work / "output.f32le").resolve()),
        "sample_rate": rate,
        "channels": 2,
        "frames": len(x),
        "callback_frames": callback,
        "appended_zero_frames": zeros,
        "params": params,
    }
    source = work / "request.json"
    source.write_text(json.dumps(request, allow_nan=False) + "\n")
    result = subprocess.run(
        [str(Path(renderer).resolve()), str(source)],
        capture_output=True,
        text=True,
        check=False,
        timeout=120,
    )
    (work / "stderr.txt").write_text(result.stderr)
    if result.returncode:
        raise ValueError("Native Stereo Imager failed: " + result.stderr)
    metadata = json.loads(result.stdout)
    expected = {
        "status": "ok",
        "sample_rate": rate,
        "channels": 2,
        "input_frames": len(x),
        "output_frames": len(x) + zeros,
        "appended_zero_frames": zeros,
        "callback_frames": callback,
        "latency_samples": 0,
    }
    if any(metadata.get(k) != value for k, value in expected.items()):
        raise ValueError("Native Stereo Imager geometry mismatch")
    effective = metadata["effective_params"]
    if set(effective) != set(params) or any(
        effective[k] != v
        if isinstance(v, bool)
        else np.float32(effective[k]) != np.float32(v)
        for k, v in params.items()
    ):
        raise ValueError("Native controls do not match requested controls")
    path = work / "output.f32le"
    if path.stat().st_size != (len(x) + zeros) * 2 * 4:
        raise ValueError("Native Stereo Imager output byte mismatch")
    y = np.fromfile(path, dtype="<f4").reshape(-1, 2)
    if not np.isfinite(y).all():
        raise ValueError("Native Stereo Imager output must be finite")
    metadata.update(
        input_sha256=hashlib.sha256(raw).hexdigest(),
        output_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
    )
    (work / "metadata.json").write_text(
        json.dumps(metadata, indent=2, allow_nan=False) + "\n"
    )
    return y, metadata
