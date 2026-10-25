"""Offline spectral-preference regression against explicitly supplied SPL levels.

The RBF prior is conditioned to zero at the declared reference. Uncertainty is
conditional on this prior; programme bootstrap and holdout errors add abstention
checks, not calibrated human-confidence guarantees.
"""

import numpy as np
from scipy.linalg import cho_factor, cho_solve

FORMAT = "sotf-loudness-preferences-v1"
POLICY = {
    "lengthscale_db": 10.0,
    "prior_std_db": 3.0,
    "noise_floor_db": 0.25,
    "minimum_programmes": 8,
    "minimum_levels": 3,
    "minimum_groups_per_level": 4,
    "bootstrap_samples": 200,
    "bootstrap_seed": 20261026,
    "maximum_lopo_rmse_db": 1.0,
    "maximum_within_cell_spread_db": 1.5,
    "maximum_interval_halfwidth_db": 1.5,
    "maximum_abs_gain_db": 6.0,
    "maximum_dense_abs_response_db": 12.0,
}


def number(value, lower, upper, name):
    if (
        isinstance(value, bool)
        or not isinstance(value, (int, float))
        or not np.isfinite(value)
        or not lower <= value <= upper
    ):
        raise ValueError("Invalid " + name)
    return float(value)


def identifier(value):
    if not isinstance(value, str) or not 1 <= len(value) <= 128:
        raise ValueError(
            "Identifiers must be nonempty strings of at most 128 characters"
        )
    return value


def validate(data):
    if (
        not isinstance(data, dict)
        or data.get("format") != FORMAT
        or data.get("data_origin")
        not in ("listener_observations", "synthetic_validation")
    ):
        raise ValueError("Declare the preference-session format and data origin")
    context, calibration, protocol = (
        data["context"],
        data["calibration"],
        data["protocol"],
    )
    for key in ["listener_id", "playback_chain_id", "position_id", "session_id"]:
        identifier(context[key])
    if (
        context["presentation"] not in ("headphones", "stereo_speakers")
        or type(context["sample_rate"]) is not int
        or not 44100 <= context["sample_rate"] <= 192000
    ):
        raise ValueError("Unsupported listening context")
    if (
        calibration.get("level_basis")
        != "identity_reference_signal_at_listening_position"
        or calibration.get("weighting") != "Z"
        or calibration.get("confirmed") is not True
    ):
        raise ValueError("Explicit identity-reference SPL measurement is required")
    identifier(calibration["measurement_id"])
    identifier(calibration["method"])
    reference = number(calibration["reference_level_db_spl"], 60, 100, "reference SPL")
    if (
        protocol.get("blinded") is not True
        or protocol.get("randomized_order") is not True
        or protocol.get("reference_identity_anchor") is not True
        or protocol.get("headroom_policy") != "native_peak_normalized"
    ):
        raise ValueError(
            "Declare controlled auditions, identity anchor and native normalization policy"
        )
    identifier(protocol["external_level_matching_method"])
    number(protocol["maximum_level_mismatch_db"], 0, 0.25, "audition level mismatch")
    shape = data["filter_shape"]
    bounds = {
        "low_freq": (20, 500),
        "mid_freq": (500, 8000),
        "mid_q": (0.1, 5),
        "high_freq": (2000, 20000),
    }
    if not isinstance(shape, dict) or set(shape) != set(bounds):
        raise ValueError("Supply the exact fixed auditioned filter shape")
    for key, (lower, upper) in bounds.items():
        number(shape[key], lower, upper, key)
    trials = data["observations"]
    if not isinstance(trials, list) or not 1 <= len(trials) <= 512:
        raise ValueError("Supply one to 512 observations")
    seen, grouped = set(), {}
    for row in trials:
        trial_id, programme = identifier(row["id"]), identifier(row["programme_id"])
        if trial_id in seen:
            raise ValueError("Repeated observation identifier")
        seen.add(trial_id)
        level = number(row["level_db_spl"], 40, min(reference, 90), "observation SPL")
        gains = row["preferred_gains_db"]
        if not isinstance(gains, list) or len(gains) != 3:
            raise ValueError("Preferred gains must be [low, mid, high]")
        gains = [number(g, -6, 6, "preferred gain") for g in gains]
        if level == reference and max(abs(g) for g in gains) > 0.25:
            raise ValueError(
                "Reference observations contradict the declared identity anchor"
            )
        noise = number(
            row["uncertainty_db"], 0.05, 2, "reported observation uncertainty"
        )
        grouped.setdefault((programme, level), []).append((gains, noise))
    if (
        len(grouped) > 128
        or len({key[0] for key in grouped}) > 32
        or len({key[1] for key in grouped}) > 8
    ):
        raise ValueError("At most128 programme-level cells,32 programmes and8 levels")
    cells = []
    for (programme, level), values in sorted(grouped.items()):
        gains = np.asarray([v[0] for v in values])
        spread = float(np.max(np.std(gains, axis=0)))
        noise = max(POLICY["noise_floor_db"], max(v[1] for v in values), spread)
        cells.append(
            {
                "programme": programme,
                "level": level,
                "gains": gains.mean(axis=0).tolist(),
                "noise": noise,
                "spread": spread,
            }
        )
    return reference, cells


def kernel(x, y, reference):
    x, y = np.asarray(x, dtype=np.float64), np.asarray(y, dtype=np.float64)
    scale, variance = POLICY["lengthscale_db"], POLICY["prior_std_db"] ** 2
    base = variance * np.exp(-0.5 * ((x[:, None] - y[None, :]) / scale) ** 2)
    a = np.exp(-0.5 * ((x - reference) / scale) ** 2)
    b = np.exp(-0.5 * ((y - reference) / scale) ** 2)
    result = base - variance * a[:, None] * b[None, :]
    result[x == reference, :] = 0
    result[:, y == reference] = 0
    return result


def regress(x, y, noise, weights, query, reference):
    keep = weights > 0
    x, y, noise, weights = x[keep], y[keep], noise[keep], weights[keep]
    covariance = kernel(x, x, reference) + np.diag(noise**2 / weights)
    factor = cho_factor(covariance, lower=True, check_finite=True)
    cross = kernel(np.asarray([query]), x, reference)[0]
    alpha = cho_solve(factor, y)
    mean = cross @ alpha
    variance = max(
        0.0,
        float(
            kernel(np.asarray([query]), np.asarray([query]), reference)[0, 0]
            - cross @ cho_solve(factor, cross)
        ),
    )
    return mean, variance, alpha


def fit(data, query):
    reference, cells = validate(data)
    query = number(query, 40, 90, "requested SPL")
    x = np.asarray([c["level"] for c in cells])
    y = np.asarray([c["gains"] for c in cells])
    noise = np.asarray([c["noise"] for c in cells])
    groups, index, counts = np.unique(
        [c["programme"] for c in cells], return_inverse=True, return_counts=True
    )
    weights = 1 / counts[index]
    reasons = []
    if (
        len(groups) < POLICY["minimum_programmes"]
        or len(np.unique(x)) < POLICY["minimum_levels"]
    ):
        reasons.append("insufficient_independent_programmes_or_levels")
    if any(
        np.sum(x == level) < POLICY["minimum_groups_per_level"]
        for level in np.unique(x)
    ):
        reasons.append("insufficient_programmes_at_a_level")
    if not float(x.min()) <= query <= reference:
        reasons.append("requested_level_outside_observed_reference_span")
    if max(c["spread"] for c in cells) > POLICY["maximum_within_cell_spread_db"]:
        reasons.append("inconsistent_repeated_preferences")
    report = {
        "accepted": False,
        "parameters": None,
        "requested_level_db_spl": query,
        "reference_level_db_spl": reference,
        "reasons": reasons,
        "policy": POLICY.copy(),
        "data_origin": data["data_origin"],
        "programmes": len(groups),
        "cells": cells,
        "qualification": "Personal spectral preference versus supplied SPL; conditional GP and programme sensitivity are not calibrated listener confidence or an equal-loudness model",
    }
    if reasons:
        return report
    mean, variance, alpha = regress(x, y, noise, weights, query, reference)
    residuals = np.zeros_like(y)
    for group in range(len(groups)):
        training_weights = weights * (index != group)
        for row in np.flatnonzero(index == group):
            predicted, _, _ = regress(x, y, noise, training_weights, x[row], reference)
            residuals[row] = predicted - y[row]
    rmse = np.sqrt(np.sum(weights[:, None] * residuals**2, axis=0) / weights.sum())
    rng = np.random.default_rng(POLICY["bootstrap_seed"])
    bootstrap = []
    for _ in range(POLICY["bootstrap_samples"]):
        draws = np.bincount(
            rng.integers(len(groups), size=len(groups)), minlength=len(groups)
        )
        bootstrap.append(
            regress(x, y, noise, weights * draws[index], query, reference)[0]
        )
    bootstrap = np.asarray(bootstrap)
    low, high = np.quantile(bootstrap, [0.025, 0.975], axis=0)
    halfwidth = np.maximum(
        1.96 * np.sqrt(variance + rmse**2),
        np.maximum(np.abs(mean - low), np.abs(high - mean)),
    )
    if query == reference:
        mean, halfwidth = np.zeros(3), np.zeros(3)
    if np.max(rmse) > POLICY["maximum_lopo_rmse_db"]:
        reasons.append("held_programme_preferences_inconsistent")
    if np.max(halfwidth) > POLICY["maximum_interval_halfwidth_db"]:
        reasons.append("uncertainty_too_large")
    if np.max(np.abs(mean) + halfwidth) > POLICY["maximum_abs_gain_db"]:
        reasons.append("uncertainty_interval_exceeds_gain_bounds")
    report.update(
        predicted_gains_db=mean.tolist(),
        interval_low_db=(mean - halfwidth).tolist(),
        interval_high_db=(mean + halfwidth).tolist(),
        latent_variance_db2=variance,
        lopo_rmse_db=rmse.tolist(),
        lopo_residuals_db=residuals.tolist(),
        bootstrap_gains_db=bootstrap.tolist(),
        model={
            "format": "sotf-anchored-rbf-preference-v1",
            "alpha": alpha.tolist(),
            "anchor": "base RBF GP conditioned on f(reference)=0; zero uncertainty at reference is imposed, not inferred",
        },
    )
    if not reasons:
        shape = data["filter_shape"]
        report["parameters"] = dict(
            shape,
            low_gain=float(mean[0]),
            mid_gain=float(mean[1]),
            high_gain=float(mean[2]),
            mid_enabled=True,
            mode=0,
            channel_params=[],
            auto_gain_enabled=False,
            auto_gain_position="disabled",
            headroom_normalized=True,
            auto_calibrated=False,
            playback_level_db=query,
            reference_level_db=reference,
            playback_volume_db=0.0,
        )
        report["accepted"] = True
    return report
