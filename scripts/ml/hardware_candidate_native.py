"""Explicit native QA qualification of unadmitted hardware neural candidates."""

import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np
from analog_color_native import controls


def render(
    signal,
    renderer,
    work,
    candidate,
    sha256,
    context,
    *,
    fallback=None,
    rate=48000,
    callback=193,
    zeros=0,
    enabled=True,
    disabled_ranges=(),
    reset_frames=(),
):
    x = np.asarray(signal, dtype=np.float32)
    if (
        x.ndim != 1
        or not len(x)
        or type(rate) is not int
        or not 44100 <= rate <= 192000
        or type(callback) is not int
        or not 1 <= callback <= 4096
        or type(zeros) is not int
        or not 0 <= zeros <= 65536
        or len(x) + zeros > 4_000_000
        or type(enabled) is not bool
    ):
        raise ValueError("Unsupported bounded mono qualification request")
    fallback = controls() if fallback is None else fallback
    work = Path(work)
    work.mkdir(parents=True, exist_ok=False)
    raw = x.astype("<f4").tobytes()
    (work / "input.f32le").write_bytes(raw)
    request = {
        "input": str((work / "input.f32le").resolve()),
        "output": str((work / "output.f32le").resolve()),
        "candidate": None
        if candidate is None
        else {"path": str(Path(candidate).resolve()), "sha256": sha256},
        "expected_context": context,
        "sample_rate": rate,
        "frames": len(x),
        "callback_frames": callback,
        "appended_zero_frames": zeros,
        "enabled": enabled,
        "disabled_ranges": list(disabled_ranges),
        "reset_frames": list(reset_frames),
        "fallback": fallback,
    }
    source = work / "request.json"
    source.write_text(json.dumps(request, allow_nan=False) + "\n")
    process = subprocess.run(
        [str(Path(renderer).resolve()), str(source)],
        check=False,
        capture_output=True,
        text=True,
        timeout=120,
    )
    (work / "stderr.txt").write_text(process.stderr)
    if process.returncode:
        raise ValueError("Native qualification failed: " + process.stderr)
    m = json.loads(process.stdout)
    expected = {
        "status": "ok",
        "sample_rate": rate,
        "channels": 1,
        "input_frames": len(x),
        "output_frames": len(x) + zeros,
        "callback_frames": callback,
        "appended_zero_frames": zeros,
        "latency_samples": 0,
        "hardware_model_admitted": False,
    }
    if any(m.get(k) != v for k, v in expected.items()):
        raise ValueError("Native qualification geometry mismatch")
    if set(m["effective_fallback"]) != set(fallback) or any(
        np.float32(m["effective_fallback"][k]) != np.float32(v)
        for k, v in fallback.items()
    ):
        raise ValueError("Native classical fallback controls mismatch")
    path = work / "output.f32le"
    if path.stat().st_size != (len(x) + zeros) * 4:
        raise ValueError("Native qualification byte count mismatch")
    y = np.fromfile(path, dtype="<f4")
    if (
        not np.isfinite(y).all()
        or np.max(abs(y)) > 128
        or np.float32(m["sample_peak"]) != np.max(abs(y))
    ):
        raise ValueError("Native qualification violates finite magnitude contract")
    if sum(m["counts"].values()) != len(y):
        raise ValueError("Native frame counters do not cover output")
    m.update(
        input_sha256=hashlib.sha256(raw).hexdigest(),
        output_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
    )
    (work / "metadata.json").write_text(json.dumps(m, indent=2, allow_nan=False) + "\n")
    return y, m
