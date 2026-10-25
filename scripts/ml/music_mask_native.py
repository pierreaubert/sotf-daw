"""Render native classical or QA musical gains through complete synthesis support."""

import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np


def parameters(low_latency=True, reduction=20.0, floor=-40.0):
    return {"low_latency": low_latency, "reduction_db": reduction, "floor_db": floor}


def render(
    signal,
    renderer,
    work,
    params,
    *,
    model=None,
    sha=None,
    enabled=False,
    rate=48000,
    callback=193,
    drain_capacity=257,
):
    x = np.asarray(signal, dtype=np.float32)
    if (
        x.ndim != 2
        or not len(x)
        or not 1 <= x.shape[1] <= 8
        or x.size > 8_000_000
        or not np.isfinite(x).all()
        or np.max(abs(x)) > 16
        or type(rate) is not int
        or not 44100 <= rate <= 192000
        or type(callback) is not int
        or not 1 <= callback <= 512
        or type(drain_capacity) is not int
        or not 1 <= drain_capacity <= 2048
        or type(enabled) is not bool
        or not isinstance(params, dict)
    ):
        raise ValueError("Unsupported bounded native musical-mask geometry")
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
        "drain_capacity_frames": drain_capacity,
        "parameters": params,
        "model": None
        if model is None
        else {"path": str(Path(model).resolve()), "sha256": sha},
        "enabled": enabled,
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
        raise ValueError("Native music render failed: " + p.stderr)
    m = json.loads(p.stdout)
    effective = m["effective_parameters"]
    for key, value in params.items():
        if type(value) in (int, float):
            value = float(np.float32(value))
        if effective.get(key) != value:
            raise ValueError("Native denoiser parameters differ from request")
    fft = 512 if effective["low_latency"] else 2048
    hop = fft // 2
    tail = 2 * fft - hop + (hop - len(x) % hop) % hop
    expected = {
        "status": "ok",
        "sample_rate": rate,
        "channels": x.shape[1],
        "input_frames": len(x),
        "output_frames": len(x) + tail,
        "drain_frames": tail,
        "drain_capacity_frames": drain_capacity,
        "callback_frames": callback,
        "enabled": enabled,
        "latency_samples": fft,
        "added_model_lookahead_samples": 0,
        "deployment_admitted": False,
    }
    if (
        any(m.get(k) != v for k, v in expected.items())
        or m["counts"]["frames"] != len(x) // hop
        or not 0 < m["drain_calls"] <= m["drain_call_bound"]
    ):
        raise ValueError("Native musical-mask timing/context mismatch")
    output = work / "output.f32le"
    if output.stat().st_size != m["output_frames"] * x.shape[1] * 4:
        raise ValueError("Native music PCM length mismatch")
    y = np.fromfile(output, dtype="<f4").reshape(-1, x.shape[1])
    if not np.isfinite(y).all() or np.max(abs(y)) > 128:
        raise ValueError("Native music output rail violation")
    m.update(
        input_sha256=hashlib.sha256(raw).hexdigest(),
        output_sha256=hashlib.sha256(output.read_bytes()).hexdigest(),
    )
    (work / "metadata.json").write_text(json.dumps(m, indent=2, allow_nan=False) + "\n")
    return y, m
