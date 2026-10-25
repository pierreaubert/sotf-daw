"""Per-session Gaussian-process proposals over four bounded native EQ controls."""

import copy

import numpy as np
from scipy.linalg import cho_factor, cho_solve, solve_triangular
from scipy.optimize import minimize
from scipy.special import ndtr
from scipy.stats import qmc

LOW = np.array([-60.0, 0.0, np.log(0.1), np.log(10.0)])
HIGH = np.array([0.0, np.log(20.0), np.log(100.0), np.log(1000.0)])


def encode(params):
    x = np.array(
        [
            params["threshold"],
            np.log(params["ratio"]),
            np.log(params["attack_ms"]),
            np.log(params["release_ms"]),
        ]
    )
    if not np.isfinite(x).all() or np.any(x < LOW) or np.any(x > HIGH):
        raise ValueError("Initial dynamics outside supported bounds")
    return (x - LOW) / (HIGH - LOW)


def controls(x):
    x = np.asarray(x, dtype=float)
    if x.shape != (4,) or not np.isfinite(x).all() or np.any(x < 0) or np.any(x > 1):
        raise ValueError("Normalized controls must lie in [0,1]^4")
    p = LOW + x * (HIGH - LOW)
    return {
        "threshold": float(np.float32(p[0])),
        "ratio": float(np.float32(np.exp(p[1]))),
        "attack_ms": float(np.float32(np.exp(p[2]))),
        "release_ms": float(np.float32(np.exp(p[3]))),
    }


def make_preset(template, x):
    result = copy.deepcopy(template)
    result.update(controls(x))
    for band in result["bands"]:
        band["band_threshold"] = result["threshold"]
        band["band_ratio"] = result["ratio"]
    return result


def kernel(a, b):
    r = (
        np.sqrt(np.maximum(np.sum((a[:, None, :] - b[None, :, :]) ** 2, axis=2), 0))
        / 0.25
    )
    z = np.sqrt(5) * r
    return (1 + z + z * z / 3) * np.exp(-z)


def posterior(x, y, query):
    """Fixed Matérn-5/2 prior, learned objective observations, normalized log loss."""
    y = np.log10(np.maximum(np.asarray(y), 1e-12))
    mean = float(y.mean())
    scale = max(float(y.std()), 0.1)
    system = kernel(x, x) + np.eye(len(x)) * 1e-6
    factor = cho_factor(system, lower=True, check_finite=True)
    cross = kernel(x, query)
    mu = mean + scale * (cross.T @ cho_solve(factor, (y - mean) / scale))
    whitened = solve_triangular(factor[0], cross, lower=True)
    sigma = scale * np.sqrt(np.maximum(1 - np.sum(whitened**2, axis=0), 0))
    return mu, sigma


def optimize(objective, initial, budget=160, method="surrogate", seed=20581026):
    """Equal native-call allowance: 32 global proposals, then bounded refinement."""
    if (
        type(budget) is not int
        or not 40 <= budget <= 256
        or method not in {"surrogate", "classical"}
    ):
        raise ValueError("Use 40..256 evaluations and a declared method")
    initial = np.asarray(initial, dtype=float)
    controls(initial)
    rng = np.random.default_rng(seed)
    pool = qmc.Sobol(4, scramble=True, seed=seed).random_base2(10)
    rows = []
    cache = {}

    def evaluate(x):
        x = np.clip(np.asarray(x, dtype=float), 0, 1)
        values = controls(x)
        key = tuple(values.values())
        if key in cache:
            return cache[key]
        if len(rows) >= budget:
            raise RuntimeError("native evaluation budget exhausted")
        value = float(objective(x))
        if not np.isfinite(value) or value < 0:
            raise ValueError("Objective must be finite nonnegative loss")
        rows.append({"x": x.tolist(), "controls": values, "loss": value})
        cache[key] = value
        return value

    global_count = min(32, budget // 3)
    evaluate(initial)
    for point in pool[:7]:
        evaluate(point)
    for index in range(8, global_count):
        if method == "classical":
            point = pool[index - 1]
        else:
            x = np.array([r["x"] for r in rows])
            y = np.array([r["loss"] for r in rows])
            best = x[int(np.argmin(y))]
            query = np.vstack(
                [pool[32:288], np.clip(best + rng.normal(size=(256, 4)) * 0.12, 0, 1)]
            )
            keep = [tuple(controls(q).values()) not in cache for q in query]
            query = query[keep]
            mu, sigma = posterior(x, y, query)
            improvement = np.log10(max(float(y.min()), 1e-12)) - mu
            z = improvement / np.maximum(sigma, 1e-12)
            expected = improvement * ndtr(z) + sigma * np.exp(-z * z / 2) / np.sqrt(
                2 * np.pi
            )
            point = query[int(np.argmax(expected))]
        evaluate(point)
    best = np.asarray(min(rows, key=lambda r: r["loss"])["x"])
    simplex = np.tile(best, (5, 1))
    for dimension in range(4):
        simplex[dimension + 1, dimension] += 0.08 if best[dimension] <= 0.92 else -0.08
    # SciPy's maxfev counts even repeated cached points, so this cannot exceed
    # the native allowance. Stop at convergence rather than waste calls.
    result = minimize(
        evaluate,
        best,
        method="Nelder-Mead",
        bounds=[(0, 1)] * 4,
        options={
            "maxfev": budget - len(rows),
            "maxiter": 1000,
            "initial_simplex": simplex,
            "xatol": 1e-5,
            "fatol": 1e-10,
        },
    )
    del result
    best = min(rows, key=lambda r: r["loss"])
    return {
        "method": method,
        "budget": budget,
        "evaluations": len(rows),
        "best": best,
        "trace": rows,
        "policy": {
            "global_proposals": global_count,
            "kernel": "Matern5/2",
            "length_scale": 0.25,
            "diagonal_jitter": 1e-6,
            "minimum_log_loss_scale": 0.1,
            "acquisition": "expected improvement over log10 loss",
            "seed": seed,
            "refinement": "bounded Nelder-Mead",
            "uncertainty": "Optimization heuristic; not calibrated parameter or population confidence",
        },
    }
