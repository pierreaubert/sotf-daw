"""Native prepared-XTC export and independent double-precision WOLA reference."""

import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np


def render(
    signal, renderer, work, *, proposal=None, sha=None, model_sha=None, callback=113
):
    x = np.asarray(signal, dtype="<f4")
    if (
        x.ndim != 2
        or x.shape[1] != 2
        or not 0 < len(x) <= 1_000_000
        or not np.isfinite(x).all()
        or np.max(abs(x)) > 64
        or type(callback) is not int
        or not 1 <= callback <= 1024
    ):
        raise ValueError("Invalid bounded native XTC geometry")
    root = Path(work).resolve()
    root.mkdir(parents=True, exist_ok=False)
    request = {
        "input": str(root / "input.f32le"),
        "output": str(root / "output.f32le"),
        "metadata": str(root / "metadata.json"),
        "frames": len(x),
        "callback_frames": callback,
        "proposal": None
        if proposal is None
        else {
            "path": str(Path(proposal).resolve()),
            "sha256": sha,
            "model_sha256": model_sha,
        },
    }
    Path(request["input"]).write_bytes(x.tobytes())
    path = root / "request.json"
    path.write_text(json.dumps(request, indent=2) + "\n")
    p = subprocess.run(
        [str(Path(renderer).resolve()), str(path)],
        capture_output=True,
        text=True,
        timeout=120,
        check=False,
    )
    (root / "stderr.txt").write_text(p.stderr)
    if p.returncode:
        raise RuntimeError(p.stderr)
    metadata = json.loads(Path(request["metadata"]).read_text())
    if (
        metadata["source_frames"] != len(x)
        or metadata["sample_rate"] != 44100
        or metadata["fft_size"] != 2048
        or metadata["hop_size"] != 512
        or metadata["latency_samples"] != 2048
        or metadata["model_lookahead_samples"] != 0
        or not len(x) <= metadata["output_frames"] <= len(x) + 65536
    ):
        raise RuntimeError("Native XTC clock/geometry mismatch")
    y = np.fromfile(request["output"], dtype="<f4").reshape(-1, 2)
    if len(y) != metadata["output_frames"] or not np.isfinite(y).all():
        raise RuntimeError("Invalid native XTC output")
    metadata["files"] = {
        key: {
            "path": request[key],
            "sha256": hashlib.sha256(Path(request[key]).read_bytes()).hexdigest(),
        }
        for key in ["input", "output", "metadata"]
    }
    (root / "evidence.json").write_text(json.dumps(metadata, indent=2) + "\n")
    return y, metadata


def coefficients(metadata):
    raw = np.asarray(metadata["filters_real_imag"], float)
    if raw.shape != (1025, 2, 2, 2) or not np.isfinite(raw).all():
        raise ValueError("Expected finite stereo FFT2048 filter matrices")
    return raw[..., 0] + 1j * raw[..., 1]


def wola_reference(signal, metadata):
    x = np.asarray(signal, dtype="<f4").astype(float)
    window = 0.5 - 0.5 * np.cos(2 * np.pi * np.arange(2048) / 2048)
    if (
        np.max(abs(window - np.asarray(metadata["analysis_window"]))) > 3e-7
        or abs(metadata["output_scale"] - 1 / (1.5 * 2048)) > 2e-11
    ):
        raise ValueError("Native window or COLA convention changed")
    matrices = coefficients(metadata)
    result = np.zeros((metadata["output_frames"], 2))
    for start in range(-1536, len(x), 512):
        block = np.zeros((2048, 2))
        lo, hi = max(start, 0), min(start + 2048, len(x))
        block[lo - start : hi - start] = x[lo:hi]
        spectrum = np.fft.rfft(block * window[:, None], axis=0)
        transformed = np.einsum("fse,fe->fs", matrices, spectrum)
        audio = np.fft.irfft(transformed, n=2048, axis=0) * window[:, None] / 1.5
        # Native startup discards the finalized negative-programme-time hops.
        offset = start + 2048
        begin, end = max(2048 - offset, 0), min(2048, len(result) - offset)
        if end > begin:
            result[offset + begin : offset + end] += audio[begin:end]
    return result


def projected_transfer(audio, bin_index, cue_amplitude, source_frames):
    """Fit carrier gain and DC after settling; preserve complex drive ratios."""
    y = np.asarray(audio, float)
    source_indices = np.arange(8820, source_frames - 4410)
    phase = 2 * np.pi * bin_index * source_indices / 2048
    design = np.column_stack([np.cos(phase), np.sin(phase), np.ones(len(phase))])
    observations = y[source_indices + 2048]
    fitted = np.linalg.lstsq(design, observations, rcond=None)[0]
    transfer = (fitted[0] - 1j * fitted[1]) / cue_amplitude
    residual = observations - design @ fitted
    return transfer, float(np.sqrt(np.mean(residual**2)))
