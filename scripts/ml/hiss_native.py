"""Explicit native Hiss capture/render boundary for optional learned proposals."""

import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np

PARAMS = {
    "enabled": True,
    "threshold_db": -30.0,
    "frequency_hz": 4000.0,
    "strength": 0.5,
    "spectral_mode": False,
    "use_captured_profile": False,
    "curve_low": 1.0,
    "curve_mid": 1.0,
    "curve_high": 1.0,
    "link_mode": 0,
    "transient_guard": False,
    "captured_profile": None,
}


def audio(signal):
    x = np.asarray(signal, dtype=np.float32)
    if x.ndim == 1:
        x = x[:, None]
    if (
        x.ndim != 2
        or not 1 <= x.shape[1] <= 8
        or not 1 <= x.size <= 4_000_000
        or not np.isfinite(x).all()
        or np.max(abs(x)) > 32
    ):
        raise ValueError("Use bounded finite native PCM")
    return x


def render(signal, renderer, work, params=None, capture=None, rate=48000, callback=193):
    """Remove declared delay only, retaining the full public spectral tail."""
    x = audio(signal)
    if (
        type(rate) is not int
        or not 44100 <= rate <= 192000
        or type(callback) is not int
        or not 1 <= callback <= 4096
    ):
        raise ValueError("Unsupported Hiss rate/callback geometry")
    if capture is not None:
        capture = audio(capture)
        if capture.shape != (rate, x.shape[1]):
            raise ValueError(
                "Explicit capture must be exactly one second with matching channels"
            )
    work = Path(work)
    work.mkdir(parents=True, exist_ok=False)
    raw = x.astype("<f4").tobytes()
    (work / "input.f32le").write_bytes(raw)
    request = {
        "input": str((work / "input.f32le").resolve()),
        "output": str((work / "output.f32le").resolve()),
        "capture_input": None,
        "sample_rate": rate,
        "channels": x.shape[1],
        "frames": len(x),
        "callback_frames": callback,
        "params": dict(PARAMS if params is None else params),
    }
    capture_hash = None
    if capture is not None:
        captured = capture.astype("<f4").tobytes()
        (work / "capture.f32le").write_bytes(captured)
        capture_hash = hashlib.sha256(captured).hexdigest()
        request["capture_input"] = str((work / "capture.f32le").resolve())
    path = work / "request.json"
    path.write_text(json.dumps(request, allow_nan=False) + "\n")
    result = subprocess.run(
        [str(Path(renderer).resolve()), str(path)],
        capture_output=True,
        text=True,
        check=False,
        timeout=120,
    )
    (work / "stderr.txt").write_text(result.stderr)
    if result.returncode:
        raise ValueError("Native Hiss render failed: " + result.stderr)
    meta = json.loads(result.stdout)
    latency, drained = meta["latency_samples"], meta["drained_frames"]
    if (
        type(latency) is not int
        or type(drained) is not int
        or not 0 <= latency <= drained <= 65536
    ):
        raise ValueError("Invalid native Hiss delay/tail")
    expected = {
        "status": "ok",
        "sample_rate": rate,
        "channels": x.shape[1],
        "input_frames": len(x),
        "output_frames": len(x) + drained,
        "callback_frames": callback,
        "capture_frames": rate if capture is not None else 0,
    }
    if any(meta.get(k) != v for k, v in expected.items()):
        raise ValueError("Native Hiss geometry mismatch")
    output = work / "output.f32le"
    if output.stat().st_size != (len(x) + drained) * x.shape[1] * 4:
        raise ValueError("Native Hiss output size mismatch")
    raw_output = output.read_bytes()
    y = np.frombuffer(raw_output, dtype="<f4").reshape(-1, x.shape[1])
    if not np.isfinite(y).all():
        raise ValueError("Non-finite native Hiss output")
    meta.update(
        input_sha256=hashlib.sha256(raw).hexdigest(),
        capture_sha256=capture_hash,
        output_sha256=hashlib.sha256(raw_output).hexdigest(),
        aligned_output_frames=len(y) - latency,
    )
    (work / "metadata.json").write_text(
        json.dumps(meta, indent=2, allow_nan=False) + "\n"
    )
    (work / "effective-preset.json").write_text(
        json.dumps(meta["effective_params"], indent=2, allow_nan=False) + "\n"
    )
    return y[latency:].copy(), meta
