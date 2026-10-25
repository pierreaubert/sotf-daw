"""Static compressor response, learned initializer and bounded exact fitting."""

import hashlib
import json
from pathlib import Path

import numpy as np
from scipy.optimize import least_squares
from scipy.special import expit

GRID = np.linspace(-84, 0, 128)
LOWER = np.array([-60, 0, 0, -24.0])
UPPER = np.array([0, np.log(20), 20, 24.0])
FORMAT = "sotf-compressor-static-initializer-v1"


def response(parameters, levels=GRID):
    """Output dBFS; coordinates threshold, log(ratio), knee, makeup dB."""
    p = np.asarray(parameters, dtype=np.float64)
    difference = np.asarray(levels) - p[..., 0, None]
    width, slope = p[..., 2, None], 1 - np.exp(-p[..., 1, None])
    hard = np.maximum(difference, 0) * slope
    distance = np.maximum(difference + width / 2, 0)
    soft = np.where(
        difference > width / 2,
        difference * slope,
        distance**2 * slope / (2 * np.maximum(width, 0.1)),
    )
    reduction = np.where(width < 0.1, hard, soft)
    return np.asarray(levels) + p[..., 3, None] - reduction


def dataset(count, seed):
    rng = np.random.default_rng(seed)
    p = rng.uniform(
        [-54, np.log(1.25), 0.1, -12], [-10, np.log(16), 20, 12], (count, 4)
    )
    ids = [hashlib.sha256(row.astype("<f8").tobytes()).hexdigest() for row in p]
    return p, response(p), ids


class Model:
    def __init__(self, path, digest):
        with Path(path).open("rb") as source:
            raw = source.read(1048577)
        if len(raw) > 1048576 or hashlib.sha256(raw).hexdigest() != digest:
            raise ValueError("Compressor model size/hash mismatch")
        data = json.loads(raw)
        if (
            not isinstance(data, dict)
            or data.get("format") != FORMAT
            or data.get("grid_dbfs") != GRID.tolist()
        ):
            raise ValueError("Unsupported compressor initializer contract")
        self.layers = []
        shapes = [(64, 128), (64, 64), (4, 64)]
        if not isinstance(data.get("layers"), list) or len(data["layers"]) != 3:
            raise ValueError("Invalid compressor model layers")
        for layer, shape in zip(data["layers"], shapes):
            w, b = (
                np.asarray(layer["weight"], dtype=np.float64),
                np.asarray(layer["bias"], dtype=np.float64),
            )
            if (
                w.shape != shape
                or b.shape != (shape[0],)
                or not np.isfinite(w).all()
                or not np.isfinite(b).all()
                or np.max(np.abs(w)) > 100
                or np.max(np.abs(b)) > 100
            ):
                raise ValueError("Invalid compressor model arrays")
            self.layers.append((w, b))
        self.digest = digest

    def predict(self, output):
        values = np.asarray(output, dtype=np.float64)
        if values.shape != GRID.shape or not np.isfinite(values).all():
            raise ValueError("Invalid model response input")
        x = (values - GRID) / 84
        for index, (w, b) in enumerate(self.layers):
            x = w @ x + b
            x = x * expit(x) if index < 2 else np.tanh(x)
        return LOWER + (x + 1) * (UPPER - LOWER) / 2


def validate_target(data):
    x, y = (
        np.asarray(data["input_dbfs"], dtype=np.float64),
        np.asarray(data["output_dbfs"], dtype=np.float64),
    )
    if x.ndim != 1 or x.shape != y.shape or not 64 <= len(x) <= 128:
        raise ValueError("Supply 64 to 128 matching input/output level samples")
    if (
        not np.isfinite(x).all()
        or not np.isfinite(y).all()
        or np.any(np.diff(x) <= 0)
        or x[0] > -84
        or x[-1] < 0
        or x[0] < -120
        or x[-1] > 0
        or np.max(np.abs(y)) > 144
    ):
        raise ValueError(
            "Static target must cover -84..0 dBFS with finite bounded levels"
        )
    return x, y


def classical_seed(x, y):
    makeup = np.median((y - x)[:8])
    slope, intercept = np.polyfit(x[-8:], y[-8:], 1)
    slope = np.clip(slope, 0.05, 0.9999)
    threshold = (intercept - makeup) / (1 - slope)
    return np.clip([threshold, np.log(1 / slope), 6, makeup], LOWER, UPPER)


def fit_curve(x, y, model, budget=100):
    gain = y - x
    if np.max(np.abs(gain - np.median(gain))) <= 1e-8 and abs(np.median(gain)) <= 24:
        p = np.array([0, 0, 0, float(np.median(gain))])
        return {
            "accepted": True,
            "method": "canonical-constant-gain",
            "parameters": p.tolist(),
            "rms_db": float(np.sqrt(np.mean((response(p, x) - y) ** 2))),
            "max_db": float(np.max(np.abs(response(p, x) - y))),
            "candidates": [],
        }
    seed = classical_seed(x, y)
    starts = []
    if model is not None:
        starts.append(("neural", model.predict(np.interp(GRID, x, y))))
    for knee in [0, 6, 16]:
        p = seed.copy()
        p[2] = knee
        starts.append((f"segmented-knee-{knee}", p))
    starts.append(("fixed", np.array([-24, np.log(4), 6, 0])))
    rows = []
    for method, initial in starts:
        calls = 0

        def residual(p):
            nonlocal calls
            calls += 1
            return response(p, x) - y

        result = least_squares(
            residual,
            np.clip(initial, LOWER + 1e-8, UPPER - 1e-8),
            bounds=(LOWER, UPPER),
            max_nfev=budget,
            ftol=1e-10,
            xtol=1e-10,
            gtol=1e-10,
        )
        error = response(result.x, x) - y
        rms, maximum = float(np.sqrt(np.mean(error**2))), float(np.max(np.abs(error)))
        accepted = bool(
            result.success
            and np.isfinite(result.x).all()
            and rms <= 0.01
            and maximum <= 0.03
        )
        rows.append(
            {
                "method": method,
                "accepted": accepted,
                "parameters": result.x.tolist() if accepted else None,
                "rms_db": rms,
                "max_db": maximum,
                "residual_calls": calls,
                "solver_evaluations": int(result.nfev),
            }
        )
    winner = min(rows, key=lambda r: (not r["accepted"], r["rms_db"]))
    return dict(winner, candidates=rows)
