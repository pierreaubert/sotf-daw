"""Actual native Dynamic EQ qualification for offline learned setting proposals."""

import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np


def preset(
    bands,
    threshold=-24.0,
    ratio=4.0,
    attack_ms=2.0,
    release_ms=50.0,
    knee=0.0,
    linked=True,
    mix=1.0,
    pairs=None,
):
    """Make explicit controls; per-band overrides default to these global values."""
    defaults = {
        "shape": "peak",
        "placement": "stereo",
        "shelf_slope": 1.0,
        "frequency": 1000.0,
        "q": 0.7,
        "gain": -6.0,
        "band_threshold": threshold,
        "band_ratio": ratio,
        "active": True,
        "solo": False,
    }
    return {
        "num_bands": len(bands),
        "threshold": threshold,
        "ratio": ratio,
        "attack_ms": attack_ms,
        "release_ms": release_ms,
        "knee": knee,
        "link_channels": linked,
        "mix": mix,
        "stereo_pairs": pairs,
        "bands": [dict(defaults, **band) for band in bands],
    }


def render(
    signal, renderer, work, params, rate=48000, callback=193, zeros=0, response_hz=()
):
    x = np.asarray(signal, dtype=np.float32)
    if x.ndim == 1:
        x = x[:, None]
    if (
        x.ndim != 2
        or not 1 <= x.shape[1] <= 8
        or not 1 <= x.size <= 4_000_000
        or not np.isfinite(x).all()
        or np.max(np.abs(x)) > 32
        or type(rate) is not int
        or not 44100 <= rate <= 192000
        or type(callback) is not int
        or not 1 <= callback <= 4096
        or type(zeros) is not int
        or not 0 <= zeros <= 65536
    ):
        raise ValueError("Use supported finite bounded Dynamic EQ geometry")
    frequencies = np.asarray(response_hz, dtype=float)
    if (
        frequencies.ndim != 1
        or len(frequencies) > 1024
        or not np.isfinite(frequencies).all()
        or np.any(frequencies < 0)
        or np.any(frequencies > rate / 2)
    ):
        raise ValueError("Response query must be finite DC..Nyquist points")
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
        "response_hz": frequencies.tolist(),
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
        raise ValueError("Native Dynamic EQ failed: " + result.stderr)
    metadata = json.loads(result.stdout)
    expected = {
        "status": "ok",
        "sample_rate": rate,
        "channels": x.shape[1],
        "input_frames": len(x),
        "output_frames": len(x) + zeros,
        "appended_zero_frames": zeros,
        "callback_frames": callback,
        "latency_samples": 0,
        "response_hz": frequencies.tolist(),
    }
    if any(metadata.get(k) != v for k, v in expected.items()):
        raise ValueError("Native Dynamic EQ geometry mismatch")
    response = np.asarray(metadata["full_target_band_response_db"], dtype=float)
    if (
        response.shape != (params["num_bands"], len(frequencies))
        or not np.isfinite(response).all()
    ):
        raise ValueError("Invalid native static target response")
    output = work / "output.f32le"
    if output.stat().st_size != (len(x) + zeros) * x.shape[1] * 4:
        raise ValueError("Native Dynamic EQ output byte mismatch")
    y = np.fromfile(output, dtype="<f4").reshape(-1, x.shape[1])
    if not np.isfinite(y).all():
        raise ValueError("Native Dynamic EQ output must be finite")
    metadata.update(
        input_sha256=hashlib.sha256(raw).hexdigest(),
        output_sha256=hashlib.sha256(output.read_bytes()).hexdigest(),
    )
    (work / "metadata.json").write_text(
        json.dumps(metadata, indent=2, allow_nan=False) + "\n"
    )
    return y, metadata
