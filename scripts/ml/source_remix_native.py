"""Actual unchanged ITU5.1 downmix followed by a separate optional stereo remixer."""

import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np


def downmix_parameters():
    return {
        "input_channels": 6,
        "input_layout": "5.1",
        "center_gain_db": -3.0,
        "surround_gain_db": -3.0,
        "height_gain_db": -6.0,
        "lfe_gain_db": -20.0,
        "phase_coherence": False,
        "phase_blend_low_hz": 300.0,
        "phase_blend_high_hz": 2000.0,
        "itu_mode": True,
        "matrix_ltrt": False,
    }


def remix_parameters(method="learned", foreground=3.0, bed=-3.0, mix=1.0):
    return {
        "foreground_gain_db": foreground,
        "bed_gain_db": bed,
        "mix": mix,
        "method": method,
    }


def render(
    signal,
    renderer,
    work,
    params,
    *,
    model=None,
    sha=None,
    rate=48000,
    callback=193,
    drain=193,
):
    x = np.asarray(signal, dtype=np.float32)
    if (
        x.ndim != 2
        or x.shape[1] != 6
        or not 0 < len(x) <= 1_000_000
        or not np.isfinite(x).all()
        or np.max(abs(x)) > 16
        or type(rate) is not int
        or not 44100 <= rate <= 192000
        or type(callback) is not int
        or not 1 <= callback <= 256
        or type(drain) is not int
        or not 1 <= drain <= 256
    ):
        raise ValueError("Unsupported bounded ITU5.1 remix-render geometry")
    if set(params) != set(remix_parameters()) or params["method"] not in {
        "bypass",
        "analytic",
        "learned",
    }:
        raise ValueError("Use complete explicit remix parameters")
    for key, low, high in [
        ("foreground_gain_db", -6, 6),
        ("bed_gain_db", -6, 6),
        ("mix", 0, 1),
    ]:
        v = params[key]
        if (
            isinstance(v, bool)
            or not isinstance(v, (int, float))
            or not np.isfinite(v)
            or not low <= v <= high
        ):
            raise ValueError("Remix controls exceed bounded domain")
    work = Path(work)
    work.mkdir(parents=True, exist_ok=False)
    raw = x.astype("<f4").tobytes()
    (work / "input.f32le").write_bytes(raw)
    request = {
        "input": str((work / "input.f32le").resolve()),
        "output": str((work / "output.f32le").resolve()),
        "baseline_output": str((work / "baseline.f32le").resolve()),
        "gain_trace_output": str((work / "gains.f32le").resolve()),
        "sample_rate": rate,
        "frames": len(x),
        "callback_frames": callback,
        "drain_capacity_frames": drain,
        "downmix": downmix_parameters(),
        "remix": params,
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
        raise ValueError("Native explicit remix failed: " + p.stderr)
    m = json.loads(p.stdout)
    tail = 768 + (256 - len(x) % 256) % 256
    expected = {
        "status": "ok",
        "pipeline_mode": "ordinary_downmix_then_separate_source_remix",
        "sample_rate": rate,
        "input_channels": 6,
        "output_channels": 2,
        "input_frames": len(x),
        "output_frames": len(x) + tail,
        "drain_frames": tail,
        "callback_frames": callback,
        "drain_capacity_frames": drain,
        "latency_samples": 512,
        "downmix_latency_samples": 0,
        "added_model_lookahead_samples": 0,
        "deployment_admitted": False,
    }
    effective = dict(
        params,
        **{
            k: float(np.float32(params[k]))
            for k in ["foreground_gain_db", "bed_gain_db", "mix"]
        },
    )
    if (
        any(m.get(k) != v for k, v in expected.items())
        or m["remix_parameters"] != effective
        or m["counts"]["frames"] != len(x) // 256
    ):
        raise ValueError("Native source-remix context mismatch")
    arrays = {}
    for name, shape in [
        ("output", (m["output_frames"], 2)),
        ("baseline", (len(x), 2)),
        ("gains", (m["trace_frames"], 2, 257)),
    ]:
        path = work / (name + ".f32le")
        if path.stat().st_size != int(np.prod(shape)) * 4:
            raise ValueError("Native remix PCM/trace size mismatch")
        arrays[name] = np.fromfile(path, dtype="<f4").reshape(shape)
        if not np.isfinite(arrays[name]).all():
            raise ValueError("Nonfinite native remix output/trace")
        m[name + "_sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
    if np.max(abs(arrays["output"])) > 256:
        raise ValueError("Native remix software rail violation")
    m["input_sha256"] = hashlib.sha256(raw).hexdigest()
    (work / "metadata.json").write_text(json.dumps(m, indent=2, allow_nan=False) + "\n")
    return arrays, m


def reference(signal, window, gains, output_frames):
    """Independent complete conditional WOLA using recorded native gain decisions."""
    x = np.asarray(signal, float)
    window = np.asarray(window, float)
    y = np.zeros((output_frames, 2), float)
    for frame, weights in enumerate(gains):
        origin = frame * 256 - 256
        indices = origin + np.arange(512)
        block = np.zeros((512, 2))
        valid = (indices >= 0) & (indices < len(x))
        block[valid] = x[indices[valid]]
        spectrum = np.fft.rfft(block * window[:, None], axis=0)
        mid = (spectrum[:, 0] + spectrum[:, 1]) / 2
        side = (spectrum[:, 0] - spectrum[:, 1]) / 2
        transformed = np.column_stack(
            [mid * weights[0] + side * weights[1], mid * weights[0] - side * weights[1]]
        )
        block = np.fft.irfft(transformed, n=512, axis=0) * window[:, None]
        output_index = 512 + origin + np.arange(512)
        valid = (indices >= 0) & (output_index >= 0) & (output_index < output_frames)
        y[output_index[valid]] += block[valid]
    return y
