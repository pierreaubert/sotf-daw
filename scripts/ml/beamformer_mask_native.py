"""Bounded actual native MVDR renderer for optional spatial-mask qualification."""

import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np


def parameters(microphones=4, spacing=5.0, angle=0.0):
    return {
        "num_mics": microphones,
        "mic_spacing_cm": spacing,
        "steer_angle_deg": angle,
        "beamformer_type": "MVDR",
    }


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
    zeros=2048,
):
    x = np.asarray(signal, dtype=np.float32)
    if (
        set(params) != set(parameters())
        or type(params["num_mics"]) is not int
        or not 2 <= params["num_mics"] <= 8
        or params["beamformer_type"] != "MVDR"
    ):
        raise ValueError("Supply complete canonical native MVDR parameters")
    for key, low, high in [("mic_spacing_cm", 1, 50), ("steer_angle_deg", -180, 180)]:
        v = params[key]
        if (
            isinstance(v, bool)
            or not isinstance(v, (int, float))
            or not np.isfinite(v)
            or not low <= v <= high
        ):
            raise ValueError("Invalid native array geometry")
    if (
        x.ndim != 2
        or x.shape[1] != params["num_mics"]
        or not len(x)
        or not np.isfinite(x).all()
        or np.max(abs(x)) > 16
        or type(rate) is not int
        or not 16000 <= rate <= 96000
        or type(callback) is not int
        or not 1 <= callback <= 4096
        or type(zeros) is not int
        or not 0 <= zeros <= 65536
        or (len(x) + zeros) * x.shape[1] > 8_000_000
        or type(enabled) is not bool
    ):
        raise ValueError("Unsupported bounded native mask-render geometry")
    work = Path(work)
    work.mkdir(parents=True, exist_ok=False)
    raw = x.astype("<f4").tobytes()
    (work / "input.f32le").write_bytes(raw)
    request = {
        "input": str((work / "input.f32le").resolve()),
        "output": str((work / "output.f32le").resolve()),
        "sample_rate": rate,
        "frames": len(x),
        "callback_frames": callback,
        "appended_zero_frames": zeros,
        "parameters": params,
        "enabled": enabled,
        "model": None
        if model is None
        else {"path": str(Path(model).resolve()), "sha256": sha},
    }
    path = work / "request.json"
    path.write_text(json.dumps(request, allow_nan=False) + "\n")
    p = subprocess.run(
        [str(Path(renderer).resolve()), str(path)],
        capture_output=True,
        text=True,
        check=False,
        timeout=120,
    )
    (work / "stderr.txt").write_text(p.stderr)
    if p.returncode:
        raise ValueError("Native MVDR mask render failed: " + p.stderr)
    m = json.loads(p.stdout)
    expected = {
        "status": "ok",
        "sample_rate": rate,
        "input_channels": x.shape[1],
        "output_channels": 1,
        "input_frames": len(x),
        "output_frames": len(x) + zeros,
        "callback_frames": callback,
        "appended_zero_frames": zeros,
        "enabled": enabled,
        "latency_samples": 512,
        "added_model_lookahead_samples": 0,
        "deployment_admitted": False,
    }
    effective = dict(
        params,
        **{
            k: float(np.float32(params[k]))
            for k in ["mic_spacing_cm", "steer_angle_deg"]
        },
    )
    if (
        any(m.get(k) != v for k, v in expected.items())
        or m["effective_parameters"] != effective
    ):
        raise ValueError("Native MVDR context/parameter mismatch")
    output = work / "output.f32le"
    if output.stat().st_size != (len(x) + zeros) * 4:
        raise ValueError("Native MVDR PCM length mismatch")
    y = np.fromfile(output, dtype="<f4")
    if not np.isfinite(y).all() or np.max(abs(y)) > 128:
        raise ValueError("Native MVDR output rail violation")
    m.update(
        input_sha256=hashlib.sha256(raw).hexdigest(),
        output_sha256=hashlib.sha256(output.read_bytes()).hexdigest(),
    )
    (work / "metadata.json").write_text(json.dumps(m, indent=2, allow_nan=False) + "\n")
    return y, m
