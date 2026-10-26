"""Shared bounded FFT1024/H256 native render protocol and independent WOLA."""

import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np


def render_fixed_wola(
    signal,
    renderer,
    work,
    *,
    params,
    model=None,
    sha=None,
    enabled=True,
    rate=48000,
    callback=193,
    drain=193,
):
    x = np.asarray(signal, dtype="<f4")
    if x.ndim == 1:
        x = x[:, None]
    if (
        x.ndim != 2
        or not 1 <= x.shape[1] <= 2
        or not 0 < len(x) <= 1_000_000
        or not np.isfinite(x).all()
        or np.max(abs(x)) > 16
        or type(rate) is not int
        or not 44100 <= rate <= 192000
        or type(callback) is not int
        or not 1 <= callback <= 256
        or type(drain) is not int
        or not 1 <= drain <= 256
        or type(enabled) is not bool
    ):
        raise ValueError("Invalid bounded native render geometry")
    work = Path(work).resolve()
    work.mkdir(parents=True, exist_ok=False)
    request = {
        "input": str(work / "input.f32le"),
        "output": str(work / "output.f32le"),
        "gain_trace_output": str(work / "gains.f32le"),
        "sample_rate": rate,
        "channels": x.shape[1],
        "frames": len(x),
        "callback_frames": callback,
        "drain_capacity_frames": drain,
        "parameters": params,
        "enabled": enabled,
        "model": None
        if model is None
        else {"path": str(Path(model).resolve()), "sha256": sha},
    }
    Path(request["input"]).write_bytes(x.tobytes())
    path = work / "request.json"
    path.write_text(json.dumps(request, indent=2, allow_nan=False) + "\n")
    result = subprocess.run(
        [str(renderer), str(path)],
        text=True,
        capture_output=True,
        timeout=120,
        check=False,
    )
    (work / "stderr.txt").write_text(result.stderr)
    if result.returncode:
        raise RuntimeError(result.stderr)
    metadata = json.loads(result.stdout)

    def native_values(value):
        if type(value) is float:
            return float(np.float32(value))
        if isinstance(value, dict):
            return {k: native_values(v) for k, v in value.items()}
        if isinstance(value, list):
            return [native_values(v) for v in value]
        return value

    if metadata.get("parameters") != native_values(params):
        raise RuntimeError("Native parameter echo differs from requested fixed profile")
    tail = 1792 + (256 - len(x) % 256) % 256
    expected = {
        "source_frames": len(x),
        "output_frames": len(x) + tail,
        "channels": x.shape[1],
        "sample_rate": rate,
        "latency_samples": 1024,
        "added_model_lookahead_samples": 0,
        "fft_size": 1024,
        "hop_size": 256,
        "drain_frames": tail,
        "enabled": enabled,
        "deployment_admitted": False,
    }
    if any(metadata.get(k) != v for k, v in expected.items()):
        raise RuntimeError("Native renderer metadata mismatch")
    if enabled:
        count = metadata["source_counters"]
        if (
            count["source_hops"] != len(x) // 256
            or count["selected_bins"] + count["fallback_bins"]
            != count["source_hops"] * 513
        ):
            raise RuntimeError("Native source counters mismatch")
    output = np.fromfile(request["output"], dtype="<f4").reshape(-1, x.shape[1])
    gains = np.fromfile(request["gain_trace_output"], dtype="<f4").reshape(
        -1, x.shape[1], 513
    )
    if (
        len(output) != expected["output_frames"]
        or len(gains) != metadata["trace_hops"]
        or len(gains) != (len(x) + tail) // 256
        or not np.isfinite(output).all()
        or not np.isfinite(gains).all()
        or np.any((gains < 0) | (gains > 1.000001))
    ):
        raise RuntimeError("Invalid native output/trace domain")
    metadata["files"] = {
        key: {
            "path": request[key],
            "sha256": hashlib.sha256(Path(request[key]).read_bytes()).hexdigest(),
        }
        for key in ["input", "output", "gain_trace_output"]
    }
    (work / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    return output, gains, metadata


def reference(signal, window, gains, frames):
    """Replay mixture-conditional gains; never reclassify an isolated source."""
    x = np.asarray(signal, float)
    if x.ndim == 1:
        x = x[:, None]
    window, gains = np.asarray(window, float), np.asarray(gains, float)
    if (
        window.shape != (1024,)
        or gains.ndim != 3
        or gains.shape[1:] != (x.shape[1], 513)
    ):
        raise ValueError("Invalid conditional WOLA geometry")
    out = np.zeros((frames, x.shape[1]))
    for hop, table in enumerate(gains):
        origin = hop * 256 - 768
        begin, end = max(0, origin), min(len(x), origin + 1024)
        frame = np.zeros((1024, x.shape[1]))
        if end > begin:
            frame[begin - origin : end - origin] = x[begin:end]
        spectrum = np.fft.rfft(frame * window[:, None], axis=0)
        reconstructed = (
            np.fft.irfft(spectrum * table.T, n=1024, axis=0) * window[:, None] / 1.5
        )
        start, stop = max(0, -origin), min(1024, frames - origin - 1024)
        if stop > start:
            out[origin + 1024 + start : origin + 1024 + stop] += reconstructed[
                start:stop
            ]
    return out
