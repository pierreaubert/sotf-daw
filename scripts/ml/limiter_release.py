"""Bounded release initializer and actual-native paired-waveform qualification."""

import hashlib
import json
import time
from pathlib import Path

import numpy as np
from limiter_native import PARAMS, render
from scipy.optimize import minimize_scalar

FORMAT = "sotf-limiter-release-initializer-v1"
FEATURES = 32
RMS_LIMIT = 0.002
MAX_LIMIT = 0.01
AMBIGUITY_SLACK = 0.001


def validate_pair(dry, target, rate, fixed):
    dry, target = np.asarray(dry, np.float32), np.asarray(target, np.float32)
    if dry.ndim == 1:
        dry = dry[:, None]
    if target.ndim == 1:
        target = target[:, None]
    if (
        type(rate) is not int
        or not 44100 <= rate <= 192000
        or dry.ndim != 2
        or dry.shape != target.shape
        or not 1 <= dry.shape[1] <= 8
        or not 2 * rate <= len(dry) <= 8 * rate
        or dry.size > 4_000_000
        or not np.isfinite(dry).all()
        or not np.isfinite(target).all()
        or np.max(np.abs(dry)) > 32
        or np.max(np.abs(target)) > 32
    ):
        raise ValueError(
            "Use finite aligned 2–8 second dry/target arrays at 44.1–192 kHz"
        )
    if not isinstance(fixed, dict) or set(fixed) != set(PARAMS) - {"release_ms"}:
        raise ValueError("Supply every fixed native control, excluding release_ms")
    for key, value in fixed.items():
        if key in {"threshold_db", "lookahead_ms"}:
            lo, hi = (-20, 0) if key == "threshold_db" else (0, 20)
            if (
                type(value) not in {int, float}
                or not np.isfinite(value)
                or not lo <= value <= hi
            ):
                raise ValueError("Invalid fixed native control: " + key)
        elif type(value) is not type(PARAMS[key]) or value != PARAMS[key]:
            raise ValueError("Unsupported release-model profile control: " + key)
    return dry, target


def descriptors(dry, target, rate, fixed):
    """Chronological window regression; no labels or hidden native state used."""
    dry, target = validate_pair(dry, target, rate, fixed)
    hop = max(1, round(rate * 0.005))
    n = len(dry) // hop
    x = dry[: n * hop].astype(np.float64).reshape(n, -1)
    y = target[: n * hop].astype(np.float64).reshape(n, -1)
    energy = np.mean(x * x, axis=1)
    gain = np.sum(x * y, axis=1) / np.maximum(np.sum(x * x, axis=1), 1e-20)
    valid = energy > 1e-8
    attenuation = np.clip(-20 * np.log10(np.clip(gain, 1e-4, 1)), 0, 80)
    attenuation[~valid] = 0
    values = np.quantile(
        attenuation[valid] if valid.any() else [0], np.linspace(0, 1, 7)
    ).tolist()
    values = [v / 40 for v in values]
    values.extend(
        [
            float(np.mean(valid)),
            float(np.mean((attenuation > 0.2) & valid)),
            float(np.log1p(np.sqrt(np.mean(x * x)))),
            float(np.log1p(np.sqrt(np.mean(y * y)))),
            float(np.log1p(np.max(np.abs(x)))),
            fixed["threshold_db"] / 20,
            fixed["lookahead_ms"] / 20,
            min(len(dry) / rate, 8) / 8,
        ]
    )
    estimates = []
    for lag in [1, 2, 4, 10]:
        before, after = attenuation[:-lag], attenuation[lag:]
        mask = (
            valid[:-lag]
            & valid[lag:]
            & (before > 0.3)
            & (after > 0.1)
            & (after < before - 1e-5)
        )
        decay = -np.log(np.clip(after / np.maximum(before, 1e-20), 1e-8, 1))
        tau = np.clip(1000 * lag * hop / rate / np.maximum(decay, 1e-12), 1, 10000)
        sample = np.log10(tau[mask]) if mask.any() else np.array([2.0])
        values.extend((np.quantile(sample, [0.1, 0.5, 0.9]) / 4).tolist())
        values.append(float(np.mean(mask)))
        estimates.extend(tau[mask].tolist())
    # A label-free analytic recovery estimate is also a strong classical seed.
    analytic = float(np.clip(np.median(estimates) if estimates else 100, 10, 1000))
    values.append(np.log10(analytic) / 3)
    features = np.asarray(values, np.float64)
    if features.shape != (FEATURES,) or not np.isfinite(features).all():
        raise ValueError("Invalid release feature vector")
    return features, analytic


class Model:
    def __init__(self, path, digest):
        with Path(path).open("rb") as source:
            raw = source.read(262145)
        if len(raw) > 262144 or hashlib.sha256(raw).hexdigest() != digest:
            raise ValueError("Limiter model size/hash mismatch")
        data = json.loads(raw)
        if (
            set(data) != {"format", "features", "layers"}
            or data["format"] != FORMAT
            or data["features"] != FEATURES
        ):
            raise ValueError("Unsupported limiter model contract")
        if not isinstance(data["layers"], list) or len(data["layers"]) != 2:
            raise ValueError("Invalid limiter model layer count")
        self.layers = []
        for layer, shape in zip(data["layers"], [(32, FEATURES), (1, 32)]):
            if not isinstance(layer, dict) or set(layer) != {"weight", "bias"}:
                raise ValueError("Invalid limiter layer fields")
            w, b = (
                np.asarray(layer["weight"], np.float64),
                np.asarray(layer["bias"], np.float64),
            )
            if (
                w.shape != shape
                or b.shape != (shape[0],)
                or not np.isfinite(w).all()
                or not np.isfinite(b).all()
                or np.max(np.abs(w)) > 100
                or np.max(np.abs(b)) > 100
            ):
                raise ValueError("Invalid limiter model arrays")
            self.layers.append((w, b))
        self.digest = digest

    def predict_features(self, features):
        x = np.asarray(features, np.float64)
        if x.shape != (FEATURES,) or not np.isfinite(x).all():
            raise ValueError("Invalid release features")
        w, b = self.layers[0]
        x = np.tanh(w @ x + b)
        w, b = self.layers[1]
        return float(10 ** (2 + np.tanh(w @ x + b).item()))


def fit(dry, target, rate, fixed, renderer, work, model=None, callback=193, budget=24):
    """Compare equal-budget neural/classical searches, then sample ambiguity."""
    dry, target = validate_pair(dry, target, rate, fixed)
    if (
        type(budget) is not int
        or not 16 <= budget <= 64
        or type(callback) is not int
        or not 1 <= callback <= 4096
    ):
        raise ValueError("Unsupported native fitting budget/callback")
    features, analytic = descriptors(dry, target, rate, fixed)
    started = time.monotonic()
    work = Path(work)
    work.mkdir(parents=True, exist_ok=False)
    scale = max(float(np.sqrt(np.mean(target.astype(np.float64) ** 2))), 1e-6)
    cache = {}
    calls = []

    def evaluate(log_release):
        log_release = float(np.clip(log_release, 1, 3))
        key = round(log_release, 10)
        if key not in cache:
            output, meta = render(
                dry,
                rate,
                renderer,
                work / f"render-{len(cache):04d}",
                callback,
                fixed | {"release_ms": 10**log_release},
            )
            delta = output.astype(np.float64) - target
            cache[key] = {
                "log_release": log_release,
                "release_ms": 10**log_release,
                "relative_rms": float(np.sqrt(np.mean(delta**2)) / scale),
                "max_error": float(np.max(np.abs(delta))),
                "native": meta,
            }
        calls.append(key)
        return cache[key]["relative_rms"] ** 2

    def search(seeds, name):
        start = len(calls)
        unique_start = len(cache)
        search_started = time.monotonic()
        for seed in seeds:
            evaluate(seed)
        best = min(calls[start:], key=lambda key: cache[key]["relative_rms"])
        center = cache[best]["log_release"]
        remaining = budget - (len(calls) - start)
        # Local refinement is only one proposal. Shared global probes follow.
        minimize_scalar(
            evaluate,
            bounds=(max(1, center - 0.3), min(3, center + 0.3)),
            method="bounded",
            options={"maxiter": remaining, "xatol": 1e-5},
        )
        chosen = min(calls[start:], key=lambda key: cache[key]["relative_rms"])
        return {
            "method": name,
            "attempted_native_evaluations": len(calls) - start,
            "actual_uncached_native_renders": len(cache) - unique_start,
            "seconds_with_shared_cache": time.monotonic() - search_started,
            **cache[chosen],
        }

    candidates = []
    if model is not None:
        proposal = model.predict_features(features)
        candidates.append(search([1, 3, np.log10(proposal)], "neural"))
    candidates.append(
        search(
            [*np.linspace(1, 3, 9), np.log10(analytic)], "classical-grid-and-analytic"
        )
    )
    # Mandatory shared global coverage is separate from each equal-budget search.
    # This is a finite diagnostic, not proof of global continuous identifiability.
    for value in np.linspace(1, 3, 33):
        evaluate(value)
    best = min(cache.values(), key=lambda c: c["relative_rms"])
    center = best["log_release"]
    margin = np.log10(1.1)
    for value in [center - margin, center + margin]:
        if 1 <= value <= 3:
            evaluate(value)
    ambiguous = [
        item["release_ms"]
        for item in cache.values()
        if abs(item["log_release"] - center) >= margin - 1e-8
        and item["relative_rms"] <= best["relative_rms"] + AMBIGUITY_SLACK
    ]
    reasons = []
    if best["relative_rms"] > RMS_LIMIT or best["max_error"] > MAX_LIMIT:
        reasons.append("native waveform mismatch")
    if ambiguous:
        reasons.append("release ambiguous on sampled native objective")
    reopened = None
    if not reasons:
        output, meta = render(
            dry,
            rate,
            renderer,
            work / "reopen",
            257 if callback != 257 else 17,
            fixed | {"release_ms": best["release_ms"]},
        )
        delta = output.astype(np.float64) - target
        reopened = {
            "relative_rms": float(np.sqrt(np.mean(delta**2)) / scale),
            "max_error": float(np.max(np.abs(delta))),
            "native": meta,
        }
        if reopened["relative_rms"] > RMS_LIMIT or reopened["max_error"] > MAX_LIMIT:
            reasons.append("native preset reopen waveform mismatch")
    accepted = not reasons
    return {
        "accepted": accepted,
        "parameters": fixed | {"release_ms": best["release_ms"]} if accepted else None,
        "reasons": reasons,
        "best": best,
        "reopened": reopened,
        "candidates": candidates,
        "analytic_seed_ms": analytic,
        "sampled_ambiguous_release_ms": sorted(ambiguous),
        "objective": sorted(cache.values(), key=lambda c: c["release_ms"]),
        "unique_native_renders": len(cache) + int(reopened is not None),
        "attempted_native_evaluations": len(calls),
        "seconds": time.monotonic() - started,
        "model_sha256": model.digest if model else None,
        "qualification": "Offline paired waveform under fixed native 1x hard fully linked single-release profile; finite ambiguity probes, no preference/hardware/continuous-time peak guarantee",
    }
