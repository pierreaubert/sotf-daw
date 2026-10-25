"""Bounded categorical/continuous native search with an offline learned GP."""

import copy

import numpy as np
from analog_color_native import NAMES
from analog_eq_native import RANGES, validate
from scipy.linalg import cho_factor, cho_solve, solve_triangular
from scipy.optimize import minimize
from scipy.special import ndtr
from scipy.stats import qmc


def float32_interval(low, high):
    a, b = np.float32(low), np.float32(high)
    if float(a) < low:
        a = np.nextafter(a, np.float32(np.inf))
    if float(b) > high:
        b = np.nextafter(b, np.float32(-np.inf))
    return float(a), float(b)


def space(spec):
    validate(spec["template"])
    models = spec["models"]
    if (
        not isinstance(models, list)
        or not 1 <= len(models) <= 6
        or len(set(models)) != len(models)
        or any(m not in NAMES for m in models)
        or spec["template"]["analog_model"] not in models
    ):
        raise ValueError("Declare unique native models including the template model")
    free = spec["free"]
    if not isinstance(free, dict) or not 1 <= len(free) <= 14:
        raise ValueError("Declare1..14 bounded numeric controls")
    for key, bound in free.items():
        if (
            key not in RANGES
            or not isinstance(bound, dict)
            or set(bound) != {"minimum", "maximum", "scale"}
        ):
            raise ValueError("Use exact numeric native search bounds")
        low, high = bound["minimum"], bound["maximum"]
        if (
            any(
                isinstance(v, bool)
                or not isinstance(v, (int, float))
                or not np.isfinite(v)
                for v in [low, high]
            )
            or not RANGES[key][0] <= low < high <= RANGES[key][1]
            or bound["scale"] not in {"linear", "log"}
            or (bound["scale"] == "log" and low <= 0)
            or not low <= spec["template"][key] <= high
        ):
            raise ValueError(
                "Search bounds/template must respect native registry ranges"
            )
        if (
            key.startswith("analog_")
            and float32_interval(low, high)[0] >= float32_interval(low, high)[1]
        ):
            raise ValueError(
                "Free analog interval needs two representable native values"
            )
    return list(free)


def decode(spec, x, model):
    keys = list(spec["free"])
    x = np.asarray(x, dtype=float)
    if (
        x.shape != (len(keys),)
        or not np.isfinite(x).all()
        or np.any((x < 0) | (x > 1))
        or model not in spec["models"]
    ):
        raise ValueError("Normalized proposal outside declared search space")
    result = copy.deepcopy(spec["template"])
    result["analog_model"] = model
    for key, value in zip(keys, x, strict=True):
        b = spec["free"][key]
        low, high = b["minimum"], b["maximum"]
        result[key] = (
            float(np.exp(np.log(low) + value * np.log(high / low)))
            if b["scale"] == "log"
            else float(low + value * (high - low))
        )
        result[key] = float(np.clip(result[key], low, high))
    for key in ["analog_drive", "analog_color", "analog_character", "analog_trim"]:
        result[key] = float(np.float32(result[key]))
        if key in spec["free"]:
            bound = spec["free"][key]
            low, high = float32_interval(bound["minimum"], bound["maximum"])
            result[key] = float(np.clip(result[key], low, high))
    validate(result)
    return result


def encode(spec):
    keys = space(spec)
    result = []
    for key in keys:
        b = spec["free"][key]
        low, high = b["minimum"], b["maximum"]
        value = spec["template"][key]
        result.append(
            np.log(value / low) / np.log(high / low)
            if b["scale"] == "log"
            else (value - low) / (high - low)
        )
    return np.array(result)


def kernel(a, b, models_a, models_b):
    distance = np.linalg.norm(a[:, None, :] - b[None, :, :], axis=2) / 0.25
    z = np.sqrt(5) * distance
    return (
        (1 + z + z * z / 3)
        * np.exp(-z)
        * (np.asarray(models_a)[:, None] == np.asarray(models_b)[None, :])
    )


def posterior(rows, query, models):
    x = np.array([r["x"] for r in rows])
    categories = [r["model"] for r in rows]
    y = np.log10(np.maximum([r["loss"] for r in rows], 1e-12))
    mean = float(y.mean())
    scale = max(float(y.std()), 0.1)
    factor = cho_factor(
        kernel(x, x, categories, categories) + 1e-6 * np.eye(len(x)), lower=True
    )
    cross = kernel(x, query, categories, models)
    mu = mean + scale * cross.T @ cho_solve(factor, (y - mean) / scale)
    projected = solve_triangular(factor[0], cross, lower=True)
    sigma = scale * np.sqrt(np.maximum(1 - np.sum(projected**2, axis=0), 0))
    return mu, sigma


def optimize(objective, spec, budget=128, method="surrogate", seed=20261109):
    initial = encode(spec)
    models = spec["models"]
    dimension = len(initial)
    if (
        type(budget) is not int
        or not 40 <= budget <= 256
        or method not in {"surrogate", "classical"}
    ):
        raise ValueError("Use40..256 native objectives and a declared search method")
    pool = qmc.Sobol(dimension, scramble=True, seed=seed).random_base2(10)
    rng = np.random.default_rng(seed)
    rows = []
    cache = {}

    def evaluate(x, model):
        x = np.clip(np.asarray(x, dtype=float), 0, 1)
        parameters = decode(spec, x, model)
        key = tuple(parameters.items())
        if key in cache:
            return cache[key]
        if len(rows) >= budget:
            raise RuntimeError("native objective budget exhausted")
        loss = float(objective(parameters))
        if not np.isfinite(loss) or loss < 0:
            raise ValueError("Native objective must be finite nonnegative loss")
        rows.append(
            {"x": x.tolist(), "model": model, "parameters": parameters, "loss": loss}
        )
        cache[key] = loss
        return loss

    for model in models:
        evaluate(initial, model)
    global_count = min(48, max(len(models) + 8, budget // 3))
    for index in range(len(rows), global_count):
        if method == "classical" or len(rows) < max(8, len(models) * 2):
            point = pool[index]
            model = models[index % len(models)]
        else:
            best = min(rows, key=lambda r: r["loss"])
            queries = np.vstack(
                [
                    pool[64:192],
                    np.clip(
                        np.array(best["x"]) + rng.normal(0, 0.12, (128, dimension)),
                        0,
                        1,
                    ),
                ]
            )
            points = np.tile(queries, (len(models), 1))
            categories = np.repeat(models, len(queries)).tolist()
            keep = [
                tuple(decode(spec, p, m).items()) not in cache
                for p, m in zip(points, categories, strict=True)
            ]
            points = points[keep]
            categories = [m for m, k in zip(categories, keep, strict=True) if k]
            if not len(points):
                break
            mu, sigma = posterior(rows, points, categories)
            improvement = np.log10(max(best["loss"], 1e-12)) - mu
            z = improvement / np.maximum(sigma, 1e-12)
            acquisition = improvement * ndtr(z) + sigma * np.exp(-z * z / 2) / np.sqrt(
                2 * np.pi
            )
            chosen = int(np.argmax(acquisition))
            point = points[chosen]
            model = categories[chosen]
        evaluate(point, model)
    best = min(rows, key=lambda r: r["loss"])
    start = np.array(best["x"])
    category = best["model"]
    simplex = np.tile(start, (dimension + 1, 1))
    for d in range(dimension):
        simplex[d + 1, d] += 0.08 if start[d] <= 0.92 else -0.08
    result = minimize(
        lambda x: evaluate(x, category),
        start,
        method="Nelder-Mead",
        bounds=[(0, 1)] * dimension,
        options={
            "maxfev": budget - len(rows),
            "maxiter": 1000,
            "initial_simplex": simplex,
            "xatol": 1e-6,
            "fatol": 1e-12,
        },
    )
    return {
        "method": method,
        "budget": budget,
        "evaluations": len(rows),
        "best": min(rows, key=lambda r: r["loss"]),
        "trace": rows,
        "refinement_result": {
            "success": bool(result.success),
            "message": str(result.message),
            "evaluations": int(result.nfev),
        },
        "policy": {
            "seed": seed,
            "global_proposals": global_count,
            "kernel": "categorical-independent Matern5/2",
            "length_scale": 0.25,
            "diagonal_jitter": 1e-6,
            "acquisition": "expected improvement in normalized log10 loss",
            "refinement": "bounded Nelder-Mead within best observed model",
            "uncertainty": "Search heuristic, not calibrated parameter/population confidence",
        },
    }
