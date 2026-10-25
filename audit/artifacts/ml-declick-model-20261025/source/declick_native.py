"""Bounded offline access to actual native Declick repair groups."""

import json
import subprocess
from pathlib import Path

import numpy as np

PARAMS = {
    "enabled": True,
    "sensitivity": 5.0,
    "link_channels": True,
    "mode": 0,
    "bands": 0,
    "crossover_hz": 4000.0,
    "frequency_skew": 0.0,
    "repair_width": 0,
    "audition_residual": False,
}


def render(signal, sample_rate, renderer, work, callback_frames=193, params=None):
    signal = np.asarray(signal, dtype=np.float32)
    if signal.ndim == 1:
        signal = signal[:, None]
    if (
        signal.ndim != 2
        or not 1 <= signal.shape[1] <= 8
        or not 1 <= signal.size <= 4_000_000
        or not np.isfinite(signal).all()
        or np.max(np.abs(signal)) > 32
    ):
        raise ValueError("Unsupported bounded native input")
    work = Path(work)
    work.mkdir(parents=True, exist_ok=False)
    signal.astype("<f4").tofile(work / "input.f32le")
    request = {
        "input": str((work / "input.f32le").resolve()),
        "output": str((work / "output.f32le").resolve()),
        "sample_rate": sample_rate,
        "channels": signal.shape[1],
        "frames": len(signal),
        "callback_frames": callback_frames,
        "params": PARAMS if params is None else params,
    }
    path = work / "request.json"
    path.write_text(json.dumps(request, allow_nan=False) + "\n")
    result = subprocess.run(
        [str(Path(renderer).resolve()), str(path)],
        capture_output=True,
        text=True,
        check=False,
        timeout=120,
    )
    if result.returncode != 0:
        raise ValueError("Native Declick render failed: " + result.stderr)
    metadata = json.loads(result.stdout)
    (work / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    latency = metadata["latency_samples"]
    raw = np.fromfile(work / "output.f32le", dtype="<f4")
    if (
        metadata["status"] != "ok"
        or not 0 <= latency <= 16
        or raw.size != (len(signal) + latency) * signal.shape[1]
        or not np.isfinite(raw).all()
    ):
        raise ValueError("Invalid native Declick alignment/output")
    raw = raw.reshape(-1, signal.shape[1])
    return raw[latency:].copy(), metadata


def groups(dry, repaired):
    dry, repaired = np.asarray(dry), np.asarray(repaired)
    if dry.shape != repaired.shape or dry.ndim != 2:
        raise ValueError("Use matching frame/channel arrays")
    changed = np.any(dry != repaired, axis=1)
    edges = np.diff(np.r_[False, changed, False].astype(np.int8))
    return list(
        zip(np.flatnonzero(edges == 1).tolist(), np.flatnonzero(edges == -1).tolist())
    )


def features(dry, repaired, start, stop, rate):
    """Two-sided scale-normalized context; clean references are never consulted."""
    if not 0 <= start < stop <= len(dry) or stop - start > 32:
        raise ValueError("Unsupported native repair group")
    output = [(stop - start) / 32, np.log(rate / 48000)]
    for milliseconds in [2, 10, 40]:
        radius = max(1, round(rate * milliseconds / 1000))
        if start < radius or stop + radius > len(dry):
            raise ValueError("Insufficient real two-sided context")
        before, after = dry[start - radius : start], dry[stop : stop + radius]
        scale = max(
            1e-6, float(np.sqrt(np.mean(np.r_[before, after].astype(np.float64) ** 2)))
        )
        delta = dry[start:stop].astype(np.float64) - repaired[start:stop]
        for window in [before, after, delta]:
            energy = float(np.sqrt(np.mean(window.astype(np.float64) ** 2)))
            output.append(float(np.clip(np.log1p(energy / scale), 0, 8)))
        surrounding = dry[start - radius : stop + radius].astype(np.float64)
        difference = np.diff(surrounding, axis=0)
        output.append(
            float(np.clip(np.log1p(np.sqrt(np.mean(difference**2)) / scale), 0, 8))
        )
    context = dry[start - 8 : stop + 8].astype(np.float64)
    fixed = repaired[start - 8 : stop + 8].astype(np.float64)
    floor = 1e-12
    for order in [1, 2]:
        a = np.mean(np.diff(context, n=order, axis=0) ** 2)
        b = np.mean(np.diff(fixed, n=order, axis=0) ** 2)
        output.append(float(np.clip(np.log((a + floor) / (b + floor)), -8, 8)))
    return np.asarray(output, dtype=np.float32)
