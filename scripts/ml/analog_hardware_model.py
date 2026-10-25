"""Offline causal neural hardware candidate and parallel-Hammerstein comparison."""

import hashlib
import json
import math
from pathlib import Path

import numpy as np
from analog_hardware_capture import validate, validate_context
from scipy.optimize import minimize

FORMAT = "sotf-analog-hardware-candidate-v1"
POLICY = {
    "minimum_train_groups": 8,
    "minimum_validation_groups": 2,
    "minimum_test_groups": 2,
    "maximum_nmse_db": -25.0,
    "minimum_hammerstein_advantage_db": 1.0,
    "maximum_peak": 0.999,
    "neural_ridge": 1e-6,
    "hammerstein_ridge": 1e-5,
    "seed": 20261101,
}


def identity(value):
    return hashlib.sha256(
        json.dumps(value, sort_keys=True, allow_nan=False).encode()
    ).hexdigest()


def windows(signal, taps):
    x = np.asarray(signal, dtype=float)
    if x.ndim != 1 or not len(x) or not np.isfinite(x).all():
        raise ValueError("Expected finite nonempty mono PCM")
    return np.lib.stride_tricks.sliding_window_view(np.pad(x, (taps - 1, 0)), taps)[
        :, ::-1
    ]


def unpack(theta, taps, hidden):
    offset = taps + taps * hidden
    return (
        theta[:taps],
        theta[taps:offset].reshape(taps, hidden),
        theta[offset : offset + hidden],
        theta[offset + hidden : offset + 2 * hidden],
        theta[-1],
    )


def objective(theta, x, y, weights, taps, hidden, energy, ridge):
    linear, w, b, v, d = unpack(theta, taps, hidden)
    h = np.tanh(x @ w + b)
    residual = x @ linear + h @ v + d - y
    delta = 2 * weights * residual / energy
    back = delta[:, None] * v * (1 - h * h)
    gradient = np.r_[
        x.T @ delta, (x.T @ back).ravel(), back.sum(axis=0), h.T @ delta, delta.sum()
    ]
    return float(
        weights @ (residual * residual) / energy + ridge * np.dot(theta, theta) / 2
    ), gradient + ridge * theta


def training_design(data, loaded, taps, hidden, iterations):
    records = [r for r in data["captures"] if r["split"] == "train"]
    groups = {r["source_group"] for r in records}
    if len(groups) < POLICY["minimum_train_groups"]:
        raise ValueError("Need at least8 whole-source training groups")
    # Bound both sampled feature storage and approximate forward multiply budget.
    budget = min(65536, 2_000_000_000 // (taps * hidden * iterations))
    quota = min(8192, budget // len(records))
    if quota < 32:
        raise ValueError("Architecture exceeds bounded training feature budget")
    counts = {g: sum(r["source_group"] == g for r in records) for g in groups}
    xs, ys, ws = [], [], []
    peak = 0.0
    for r in records:
        dry, wet = loaded[r["id"]]
        if len(dry) != r["frames"] or len(wet) != len(dry):
            raise ValueError("Training pair geometry differs from capture manifest")
        indices = np.linspace(
            taps - 1, len(dry) - 1, min(quota, len(dry) - taps + 1), dtype=int
        )
        xs.append(windows(dry, taps)[indices])
        ys.append(np.asarray(wet, dtype=float)[indices])
        ws.append(
            np.full(
                len(indices),
                1 / (len(groups) * counts[r["source_group"]] * len(indices)),
            )
        )
        peak = max(peak, float(np.max(abs(dry))))
    x, y, weights = np.concatenate(xs), np.concatenate(ys), np.concatenate(ws)
    scale = max(float(np.sqrt(weights @ (x[:, 0] ** 2))), 1e-4)
    return x / scale, y, weights, scale, peak


def polynomial_features(x):
    return np.column_stack([*(x**degree for degree in range(1, 6)), np.ones(len(x))])


def fit(data, loaded, taps=8, hidden=12, iterations=300):
    validate(data)
    if (
        type(taps) is not int
        or not 1 <= taps <= 64
        or type(hidden) is not int
        or not 2 <= hidden <= 32
        or type(iterations) is not int
        or not 50 <= iterations <= 1000
    ):
        raise ValueError("Use1..64 causal taps,2..32 hidden units,50..1000 iterations")
    x, y, weights, scale, peak = training_design(data, loaded, taps, hidden, iterations)
    energy = max(float(weights @ (y * y)), 1e-10)
    rng = np.random.default_rng(POLICY["seed"])
    linear = np.linalg.solve(
        x.T @ (weights[:, None] * x) + 1e-6 * np.eye(taps), x.T @ (weights * y)
    )
    theta = np.r_[
        linear,
        rng.normal(0, 0.5 / math.sqrt(taps), taps * hidden),
        rng.normal(0, 0.1, hidden),
        np.zeros(hidden),
        0.0,
    ]
    result = minimize(
        lambda p: objective(
            p, x, y, weights, taps, hidden, energy, POLICY["neural_ridge"]
        ),
        theta,
        jac=True,
        method="L-BFGS-B",
        bounds=[(-16.0, 16.0)] * len(theta),
        options={
            "maxiter": iterations,
            "maxfun": iterations * 3,
            "gtol": 1e-7,
            "ftol": 1e-12,
            "maxls": 40,
        },
    )
    if not np.isfinite(result.x).all():
        raise ValueError("Non-finite learned candidate")
    linear, w, b, v, d = unpack(result.x, taps, hidden)
    features = polynomial_features(x)
    dimension = features.shape[1]
    classical = np.linalg.lstsq(
        np.vstack(
            [
                features * np.sqrt(weights[:, None]),
                np.sqrt(POLICY["hammerstein_ridge"]) * np.eye(dimension),
            ]
        ),
        np.r_[y * np.sqrt(weights), np.zeros(dimension)],
        rcond=1e-10,
    )[0]
    bound = float(peak / scale * np.sum(abs(linear)) + np.sum(abs(v)) + abs(d))
    model = {
        "format": FORMAT,
        "context": data["context"],
        "deployment_admitted": False,
        "architecture": {
            "taps": taps,
            "hidden": hidden,
            "activation": "tanh",
            "history_order": "current_then_past",
        },
        "input_scale": scale,
        "input_peak_limit": peak,
        "linear": linear.tolist(),
        "hidden_weights": w.tolist(),
        "hidden_bias": b.tolist(),
        "output_weights": v.tolist(),
        "output_bias": float(d),
        "global_output_bound": bound,
        "hammerstein": {"degree": 5, "weights": classical.tolist()},
        "latency_samples": 0,
        "history_frames": taps - 1,
        "policy": POLICY.copy(),
        "training_captures": [
            {k: r[k] for k in ["id", "source_group", "dry_sha256", "wet_sha256"]}
            for r in data["captures"]
            if r["split"] == "train"
        ],
    }
    validate_model(model)
    diagnostics = {
        "iterations": int(result.nit),
        "optimizer_success": bool(result.success),
        "optimizer_message": str(result.message),
        "training_samples": len(y),
        "training_objective": float(result.fun),
        "gradient_max_abs": float(np.max(abs(result.jac))),
        "maximum_iterations": iterations,
    }
    return model, diagnostics


def validate_model(model):
    if not isinstance(model, dict):
        raise TypeError("Expected a hardware candidate object")
    if model.get("format") != FORMAT or model.get("deployment_admitted") is not False:
        raise ValueError(
            "This format is an offline candidate, not a deployment admission"
        )
    validate_context(model["context"])
    if model["policy"] != POLICY:
        raise ValueError("Candidate belongs to a different frozen comparison policy")
    a = model["architecture"]
    taps, hidden = a["taps"], a["hidden"]
    if (
        type(taps) is not int
        or not 1 <= taps <= 64
        or type(hidden) is not int
        or not 2 <= hidden <= 32
        or a["activation"] != "tanh"
        or a["history_order"] != "current_then_past"
    ):
        raise ValueError("Unsupported finite-memory neural architecture")
    arrays = [
        ("linear", (taps,)),
        ("hidden_weights", (taps, hidden)),
        ("hidden_bias", (hidden,)),
        ("output_weights", (hidden,)),
    ]
    for name, shape in arrays:
        x = np.asarray(model[name], dtype=float)
        if x.shape != shape or not np.isfinite(x).all() or np.max(abs(x)) > 16:
            raise ValueError("Malformed or unbounded model parameters")
    for name in [
        "input_scale",
        "input_peak_limit",
        "global_output_bound",
        "output_bias",
    ]:
        if (
            isinstance(model[name], bool)
            or not isinstance(model[name], (int, float))
            or not math.isfinite(model[name])
        ):
            raise ValueError("Invalid finite model scalar")
    if (
        not 1e-4 <= model["input_scale"] <= 1
        or not 0 < model["input_peak_limit"] <= 0.999
    ):
        raise ValueError("Invalid learned operating domain")
    bound = (
        model["input_peak_limit"]
        / model["input_scale"]
        * sum(abs(v) for v in model["linear"])
        + sum(abs(v) for v in model["output_weights"])
        + abs(model["output_bias"])
    )
    if abs(bound - model["global_output_bound"]) > 1e-10 or bound > 128:
        raise ValueError("Invalid global output bound")
    classical = model["hammerstein"]
    coefficients = np.asarray(classical["weights"], dtype=float)
    if (
        classical["degree"] != 5
        or coefficients.shape != (5 * taps + 1,)
        or not np.isfinite(coefficients).all()
        or np.max(abs(coefficients)) > 1e6
    ):
        raise ValueError("Invalid Hammerstein comparison")
    if model["latency_samples"] != 0 or model["history_frames"] != taps - 1:
        raise ValueError("Invalid causal frame contract")


def load_model(path, expected_sha256, context):
    with Path(path).open("rb") as source:
        raw = source.read(1024 * 1024 + 1)
    if len(raw) > 1024 * 1024 or hashlib.sha256(raw).hexdigest() != expected_sha256:
        raise ValueError("Candidate size or SHA256 differs from explicit identity")
    model = json.loads(raw)
    validate_model(model)
    if model["context"] != context:
        raise ValueError("Candidate context mismatch")
    return model


def predict(model, signal, context, baseline=False):
    validate_model(model)
    if context != model["context"]:
        raise ValueError(
            "Candidate belongs to a different hardware/calibration/rate context"
        )
    x = np.asarray(signal, dtype=float)
    if (
        x.ndim != 1
        or not len(x)
        or len(x) > 4_000_000
        or not np.isfinite(x).all()
        or np.max(abs(x)) > model["input_peak_limit"] + 1e-7
    ):
        raise ValueError("Signal outside candidate trained peak domain")
    a = model["architecture"]
    history = windows(x, a["taps"])
    result = np.empty(len(x))
    w, b, v = (
        np.array(model["hidden_weights"]),
        np.array(model["hidden_bias"]),
        np.array(model["output_weights"]),
    )
    for start in range(0, len(x), 4096):
        u = history[start : start + 4096] / model["input_scale"]
        if baseline:
            y = polynomial_features(u) @ np.array(model["hammerstein"]["weights"])
        else:
            y = (
                u @ np.array(model["linear"])
                + np.tanh(u @ w + b) @ v
                + model["output_bias"]
            )
        result[start : start + len(y)] = y
    if not np.isfinite(result).all():
        raise ValueError("Non-finite candidate prediction")
    return result


def evaluate(model, data, loaded, split):
    if split not in {"validation", "test"}:
        raise ValueError("Use explicit held-source validation or test split")
    records = [r for r in data["captures"] if r["split"] == split]
    groups = {r["source_group"] for r in records}
    reasons, rows = [], []
    if len(groups) < POLICY["minimum_" + split + "_groups"]:
        reasons.append("insufficient_held_source_groups")
    for r in records:
        dry, wet = loaded[r["id"]]
        target = np.asarray(wet, dtype=float)
        if (
            target.ndim != 1
            or len(target) != r["frames"]
            or not np.isfinite(target).all()
            or np.max(abs(target)) > 0.999
            or np.mean(target * target) < 1e-10
        ):
            raise ValueError("Invalid held target geometry, headroom or excitation")
        try:
            neural = predict(model, dry, data["context"])
            baseline = predict(model, dry, data["context"], baseline=True)
        except ValueError as error:
            rows.append(
                {
                    "id": r["id"],
                    "source_group": r["source_group"],
                    "supported": False,
                    "reason": str(error),
                }
            )
            reasons.append("held_source_outside_candidate_domain")
            continue
        # Do not remove startup, fit a delay/gain, center DC, or alter recorded levels.
        energy = max(float(np.mean(target * target)), 1e-20)
        scores = [
            10 * math.log10(max(float(np.mean((p - target) ** 2)) / energy, 1e-30))
            for p in [neural, baseline]
        ]
        peak = float(np.max(abs(neural)))
        row = {
            "id": r["id"],
            "source_group": r["source_group"],
            "supported": True,
            "neural_nmse_db": scores[0],
            "hammerstein_nmse_db": scores[1],
            "advantage_db": scores[1] - scores[0],
            "sample_peak": peak,
            "dc_error": float(np.mean(neural - target)),
        }
        if (
            scores[0] > POLICY["maximum_nmse_db"]
            or row["advantage_db"] < POLICY["minimum_hammerstein_advantage_db"]
            or peak > POLICY["maximum_peak"]
        ):
            reasons.append("held_source_quality_or_advantage_gate")
        rows.append(row)
    return {
        "split": split,
        "source_groups": len(groups),
        "captures": rows,
        "passed": not reasons,
        "reasons": sorted(set(reasons)),
    }
