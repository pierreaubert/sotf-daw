"""Shared regularized categorical preference ranking with grouped abstention."""

import math

import numpy as np
from scipy.optimize import minimize
from scipy.special import expit

POLICY = {
    "ridge": 0.01,
    "bootstrap_samples": 200,
    "bootstrap_seed": 20261026,
    "minimum_programmes": 8,
    "minimum_trials": 32,
    "minimum_bootstrap_winner_fraction": 0.9,
    "minimum_bootstrap_margin_p05": 0.1,
    "minimum_lopo_log_loss_improvement": 0.01,
    "maximum_abs_side_bias": 0.5,
}


def objective(theta, design, labels, weights):
    logits = design @ theta
    probability = expit(logits)
    loss = (
        np.sum(weights * (np.logaddexp(0, logits) - labels * logits))
        + POLICY["ridge"] * np.dot(theta, theta) / 2
    )
    gradient = design.T @ (weights * (probability - labels)) + POLICY["ridge"] * theta
    hessian = design.T @ (
        (weights * probability * (1 - probability))[:, None] * design
    ) + POLICY["ridge"] * np.eye(len(theta))
    return loss, gradient, hessian


def learn(design, labels, weights):
    weights = weights / np.sum(weights)
    solved = minimize(
        lambda theta: objective(theta, design, labels, weights)[0],
        np.zeros(design.shape[1]),
        method="trust-exact",
        jac=lambda theta: objective(theta, design, labels, weights)[1],
        hess=lambda theta: objective(theta, design, labels, weights)[2],
        options={"gtol": 1e-8, "maxiter": 100},
    )
    if (
        not np.isfinite(solved.x).all()
        or np.linalg.norm(objective(solved.x, design, labels, weights)[1]) > 1e-7
    ):
        raise ValueError("Preference optimizer did not converge")
    return solved.x


def connected(design, weights):
    edges = design[weights > 0, :-1]
    reached = {0}
    while True:
        added = set(reached)
        for row in edges:
            pair = set(np.flatnonzero(row))
            if pair & reached:
                added.update(pair)
        if added == reached:
            return len(reached) == design.shape[1] - 1
        reached = added


def fit_preferences(data, validator, model_format):
    ids, design, labels, groups, orientations = validator(data)
    group_ids, group_index, group_counts = np.unique(
        groups, return_inverse=True, return_counts=True
    )
    weights = 1 / group_counts[group_index]
    reasons = []
    if (
        len(group_ids) < POLICY["minimum_programmes"]
        or len(labels) < POLICY["minimum_trials"]
    ):
        reasons.append("insufficient_independent_programmes_or_trials")
    if not connected(design, weights):
        reasons.append("disconnected_comparison_graph")
    if any(len(sides) != 2 for sides in orientations.values()):
        reasons.append("pairs_not_auditioned_in_both_orders")
    report = {
        "data_origin": data["data_origin"],
        "accepted": False,
        "parameters": None,
        "selected_id": None,
        "fallback_id": data["baseline_id"],
        "policy": POLICY.copy(),
        "reasons": reasons,
        "programmes": len(group_ids),
        "trials": len(labels),
        "model": None,
        "qualification": "Conditional preference over auditioned settings; programme bootstrap stability is not calibrated population confidence; protocol metadata is self-reported",
    }
    if reasons:
        return report
    theta = learn(design, labels, weights)
    winner = int(np.argmax(theta[:-1]))
    rng = np.random.default_rng(POLICY["bootstrap_seed"])
    samples, connected_samples = [], 0
    for _ in range(POLICY["bootstrap_samples"]):
        counts = np.bincount(
            rng.integers(len(group_ids), size=len(group_ids)), minlength=len(group_ids)
        )
        bootstrap_weights = weights * counts[group_index]
        connected_samples += int(connected(design, bootstrap_weights))
        samples.append(learn(design, labels, bootstrap_weights)[:-1])
    samples = np.asarray(samples)
    margins = samples[:, winner] - np.max(np.delete(samples, winner, axis=1), axis=1)
    winner_fraction = float(np.mean(margins > 0))
    margin_p05 = float(np.quantile(margins, 0.05))
    lopo_losses, lopo_winners = [], []
    for index in range(len(group_ids)):
        held = group_index == index
        if not connected(design, weights * ~held):
            reasons.append("disconnected_held_programme_comparison_graph")
        model = learn(design, labels, weights * ~held)
        logits = design[held] @ model
        lopo_losses.append(
            float(np.mean(np.logaddexp(0, logits) - labels[held] * logits))
        )
        lopo_winners.append(ids[int(np.argmax(model[:-1]))])
    lopo_loss = float(np.mean(lopo_losses))
    if (
        winner_fraction < POLICY["minimum_bootstrap_winner_fraction"]
        or margin_p05 <= POLICY["minimum_bootstrap_margin_p05"]
        or connected_samples
        < POLICY["minimum_bootstrap_winner_fraction"] * len(samples)
    ):
        reasons.append("unstable_programme_bootstrap")
    if math.log(2) - lopo_loss < POLICY["minimum_lopo_log_loss_improvement"]:
        reasons.append("no_held_programme_prediction_advantage")
    if abs(theta[-1]) > POLICY["maximum_abs_side_bias"]:
        reasons.append("presentation_side_bias")
    report.update(
        model={
            "format": model_format,
            "candidate_ids": ids,
            "utilities": theta[:-1].tolist(),
            "presentation_side_bias": float(theta[-1]),
        },
        winner_bootstrap_fraction=winner_fraction,
        winner_margin_p05=margin_p05,
        connected_bootstrap_samples=connected_samples,
        lopo_log_loss=lopo_loss,
        lopo_uniform_log_loss=math.log(2),
        lopo_winners=lopo_winners,
        bootstrap_utilities=samples.tolist(),
    )
    if not reasons:
        report.update(
            accepted=True,
            selected_id=ids[winner],
            parameters=data["candidates"][winner]["parameters"],
        )
    return report
