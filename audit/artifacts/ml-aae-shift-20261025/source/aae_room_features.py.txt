"""Versioned, gain-invariant finite-window AAE response descriptors.

These are energy-decay statistics, not a reconstruction of a complex sound
field. Analysis bands are constant-Q RBJ filters, not standardized IEC bands.
"""

from __future__ import annotations

import hashlib
import json
import math
import subprocess
from pathlib import Path
from typing import Self

import numpy as np
from scipy import signal

RATE = 16000
SECONDS = 8.0
TIMES = np.geomspace(0.001, 6.0, 32)
TIME_FRAMES = np.rint(TIMES * RATE).astype(np.int64)
CENTERS = (125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0)
FLOOR_DB = -40.0
DIRECT_MASK_FRAMES = 32  # Explicit 2 ms exclusion after the supplied time origin.
LAYOUTS = ("5.0", "5.1", "7.1", "5.1.2", "5.1.4", "7.1.2", "7.1.4", "9.1.4", "9.1.6")
PRESETS = ("small", "medium", "large", "cathedral")
NAMES = (
    "room_size",
    "rt60",
    "bass_ratio",
    "treble_ratio",
    "pre_delay_ms",
    "er_fraction",
)
LOW = np.array([0.2, 0.3, 0.8, 0.2, 0.0, 0.0])
HIGH = np.array([3.0, 6.0, 2.0, 1.0, 100.0, 1.0])
FEATURE_SPEC = {
    "version": 1,
    "sample_rate": RATE,
    "observation_seconds": SECONDS,
    "time_frames": TIME_FRAMES.tolist(),
    "centers_hz": list(CENTERS),
    "band_q": math.sqrt(2.0),
    "floor_db": FLOOR_DB,
    "direct_mask_frames": DIRECT_MASK_FRAMES,
    "curve_order": ["wide", *[str(x) for x in CENTERS]],
    "channel_combination": "sum of per-channel squared filtered pressure",
    "edc": "reverse cumulative energy over the observed window, normalized per band",
    "resampling": "scipy resample_poly Kaiser beta=8.6, constant zero boundary",
    "time_origin": "caller supplied; no peak alignment or geometry inference",
}

CONTROL_SPEC = {
    "names": list(NAMES),
    "low": LOW.tolist(),
    "high": HIGH.tolist(),
    "log_coordinates": [0, 1, 2, 3],
    "layouts": list(LAYOUTS),
    "presets": list(PRESETS),
}


def resample_audio(audio: np.ndarray, rate: int) -> np.ndarray:
    """Apply antialias filtering before changing the sampling grid."""
    x = np.asarray(audio, dtype=np.float64)
    if x.ndim == 1:
        x = x[:, None]
    if x.ndim != 2 or not len(x) or not x.shape[1] or not np.isfinite(x).all():
        raise ValueError("IR must contain finite samples in nonempty channels")
    if not isinstance(rate, (int, np.integer)) or not 16000 <= rate <= 192000:
        raise ValueError("analysis requires an integer source rate from 16 to 192 kHz")
    if rate == RATE:
        return x.copy()
    common = math.gcd(int(rate), RATE)
    return signal.resample_poly(
        x,
        RATE // common,
        int(rate) // common,
        axis=0,
        window=("kaiser", 8.6),
        padtype="constant",
    )


def band_sos(center: float) -> np.ndarray:
    omega = 2.0 * np.pi * center / RATE
    alpha = np.sin(omega) / (2.0 * math.sqrt(2.0))
    a0 = 1.0 + alpha
    return np.array(
        [
            [
                alpha / a0,
                0.0,
                -alpha / a0,
                1.0,
                -2.0 * np.cos(omega) / a0,
                (1.0 - alpha) / a0,
            ]
        ]
    )


def describe(audio: np.ndarray, rate: int, *, direct_sample: int = 0) -> dict:
    """Describe the captured response; missing future samples remain unobserved."""
    source = np.asarray(audio, dtype=np.float64)
    if source.ndim == 1:
        source = source[:, None]
    if (
        not isinstance(direct_sample, int)
        or direct_sample < 0
        or direct_sample >= len(source)
    ):
        raise ValueError("direct_sample must index the caller's captured response")
    x = resample_audio(source[direct_sample:], rate)
    frames = int(SECONDS * RATE)
    observed_frames = min(len(x), frames)
    if observed_frames < RATE // 20:
        raise ValueError("at least 50 ms of response must follow the time origin")
    x = x[:observed_frames]
    peak = float(np.max(np.abs(x)))
    if peak <= 0.0:
        raise ValueError("response contains no energy")
    x = x / peak  # Descriptors intentionally discard overall measurement gain.
    x[:DIRECT_MASK_FRAMES] = 0.0
    if not np.any(x):
        raise ValueError("no wet response remains after the explicit direct exclusion")
    # Filtering and integration use only captured data. Fixed-length output is
    # accompanied by a mask; extrapolated/padded samples never enter a fit loss.
    valid_times = TIME_FRAMES <= int(observed_frames * 0.75)
    curves, fits, energies = [], [], []
    for center in (None, *CENTERS):
        filtered = x if center is None else signal.sosfilt(band_sos(center), x, axis=0)
        energy = np.sum(filtered * filtered, axis=1)
        total = float(np.sum(energy))
        if not np.isfinite(total) or total <= 1e-250:
            raise ValueError("analysis band contains insufficient finite energy")
        edc = np.cumsum(energy[::-1])[::-1] / total
        db = 10.0 * np.log10(np.maximum(edc, 1e-250))
        indices = np.minimum(TIME_FRAMES, observed_frames - 1)
        curves.append(np.clip(db[indices], FLOOR_DB, 0.0))
        region = (db <= -5.0) & (db >= -25.0)
        positions = np.flatnonzero(region)
        fit = None
        if len(positions) >= 16 and positions[-1] - positions[0] >= RATE * 0.02:
            times = positions / RATE
            slope, intercept = np.polyfit(times, db[positions], 1)
            residual = db[positions] - (slope * times + intercept)
            variance = np.sum((db[positions] - np.mean(db[positions])) ** 2)
            r2 = float(1.0 - np.sum(residual * residual) / max(variance, 1e-30))
            if slope < 0.0:
                fit = {
                    "rt60_seconds": float(-60.0 / slope),
                    "r_squared": r2,
                    "points": len(positions),
                    "method": "observed -5..-25 dB extrapolation",
                }
        fits.append(fit)
        energies.append(total)
    tail = float(np.sum(x[max(0, observed_frames * 9 // 10) :] ** 2) / np.sum(x * x))
    return {
        "features": np.asarray(curves).reshape(-1),
        "mask": np.tile(valid_times, 1 + len(CENTERS)),
        "t20_fits": fits,
        "observed_frames": observed_frames,
        "observed_seconds": observed_frames / RATE,
        "observed_last_tenth_energy_fraction": tail,
        "original_peak": peak,
        "normalized_band_energies": energies,
        "direct_sample": direct_sample,
        "native_or_measured_finite_support": "unknown; no inference from record duration",
    }


def encode(unit: np.ndarray, layout: str, preset: str) -> np.ndarray:
    u = np.asarray(unit, dtype=np.float64)
    if (
        u.shape != (6,)
        or not np.isfinite(u).all()
        or np.any(u < 0.0)
        or np.any(u > 1.0)
    ):
        raise ValueError("six normalized AAE controls must lie in [0,1]")
    return np.r_[
        u,
        np.eye(len(LAYOUTS))[LAYOUTS.index(layout)],
        np.eye(len(PRESETS))[PRESETS.index(preset)],
    ].astype(np.float32)


def parameters(unit: np.ndarray, layout: str, preset: str) -> dict:
    encode(unit, layout, preset)
    values = LOW + np.asarray(unit) * (HIGH - LOW)
    values[:4] = np.exp(
        np.log(LOW[:4]) + np.asarray(unit)[:4] * np.log(HIGH[:4] / LOW[:4])
    )
    values = values.astype(np.float32)
    return {
        "speaker_config": layout,
        "room_preset": preset,
        **{
            name: float(value)
            for name, value in zip(NAMES[:5], values[:5], strict=True)
        },
        "er_level": float(values[5]),
        "late_level": float(np.float32(1.0 - values[5])),
        "dry_level": 0.0,
        "lfe_level": 0.0,
        "mod_depth": 0.0,
        "er_mod_depth": 0.0,
        "content_aware": False,
        "auto_gain_enabled": False,
        "input_diffusion": 0.7,
        "envelopment": 0.7,
        "height_amount": 0.5,
    }


def actual_unit(params: dict) -> np.ndarray:
    values = np.array(
        [params[name] for name in NAMES[:5]] + [params["er_level"]], dtype=np.float32
    ).astype(np.float64)
    unit = (values - LOW) / (HIGH - LOW)
    unit[:4] = np.log(values[:4] / LOW[:4]) / np.log(HIGH[:4] / LOW[:4])
    return np.clip(unit, 0.0, 1.0)


class Renderer:
    """One persistent native process; each request constructs pristine DSP state."""

    def __init__(self, executable: Path, root: Path):
        self.executable = Path(executable).resolve()
        self.root = Path(root).resolve()
        self.root.mkdir(parents=True, exist_ok=True)
        self.process = subprocess.Popen(
            [str(self.executable), str(self.root)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        self.counter = 0
        self.binary_sha256 = hashlib.sha256(self.executable.read_bytes()).hexdigest()

    def render(
        self,
        unit: np.ndarray,
        layout: str,
        preset: str,
        *,
        rate: int = RATE,
        seconds: float = SECONDS,
        partition: int = 1024,
        excitation: float = 0.001,
    ) -> tuple[np.ndarray, dict]:
        self.counter += 1
        identity = f"ir_{self.process.pid}_{self.counter:08d}"
        request = {
            "id": identity,
            "params": parameters(unit, layout, preset),
            "sample_rate": rate,
            "frames": round(rate * seconds),
            "partition": partition,
            "excitation": excitation,
        }
        assert self.process.stdin is not None and self.process.stdout is not None
        self.process.stdin.write(json.dumps(request, allow_nan=False) + "\n")
        self.process.stdin.flush()
        line = self.process.stdout.readline()
        if not line:
            raise RuntimeError(
                f"native renderer ended unexpectedly: {self.process.poll()}"
            )
        metadata = json.loads(line)
        if metadata.get("status") != "ok" or metadata.get("id") != identity:
            raise RuntimeError(f"native renderer rejected request: {metadata}")
        path = Path(metadata["file"])
        if path.parent != self.root or path.name != identity + ".f32le":
            raise RuntimeError("native renderer returned an unexpected output path")
        raw = path.read_bytes()
        if hashlib.sha256(raw).hexdigest() != metadata["sha256"]:
            raise RuntimeError("native render hash mismatch")
        if len(raw) != metadata["frames"] * metadata["channels"] * 4:
            raise RuntimeError("native render geometry mismatch")
        audio = (
            np.frombuffer(raw, dtype="<f4")
            .reshape(metadata["frames"], metadata["channels"])
            .copy()
        )
        path.unlink()  # Retain hashes/parameters, not gigabytes of reproducible synthetic audio.
        return audio, metadata

    def close(self) -> None:
        if self.process.stdin is not None:
            self.process.stdin.close()
        self.process.wait(timeout=30)
        if self.process.returncode != 0:
            raise RuntimeError(f"native renderer exited with {self.process.returncode}")

    def __enter__(self) -> Self:
        return self

    def __exit__(self, *_: object) -> None:
        self.close()
