"""Per-session supervised linear routing with source-group validation."""

import hashlib

import numpy as np

LAMBDAS = (0.0, 1e-6, 1e-4, 0.01, 0.1)
MIN_SINGULAR = 0.02
MAX_CONDITION = 100.0
MAX_RMS = 0.005
MAX_GROUP_RMS = 0.01
MAX_ERROR = 0.03
MAX_INTERVAL_WIDTH = 0.03
BOOTSTRAPS = 200
BOOTSTRAP_SEED = 20491026


def validate(data):
    if not isinstance(data, dict) or set(data) != {
        "format",
        "data_origin",
        "sample_rate",
        "input_labels",
        "output_labels",
        "input_channel_map",
        "output_channel_map",
        "groups",
    }:
        raise ValueError("Supply the explicit grouped routing example format")
    if data["format"] != "sotf-matrix-routing-examples-v1" or data[
        "data_origin"
    ] not in {"paired_examples", "synthetic_validation"}:
        raise ValueError("Unsupported routing format or data origin")
    if (
        type(data["sample_rate"]) is not int
        or not 44100 <= data["sample_rate"] <= 192000
    ):
        raise ValueError("Unsupported sample rate")
    dimensions = []
    for name in ["input", "output"]:
        labels, mapping = data[name + "_labels"], data[name + "_channel_map"]
        if (
            not isinstance(labels, list)
            or not 1 <= len(labels) <= 16
            or any(
                not isinstance(label, str) or not 1 <= len(label) <= 64
                for label in labels
            )
            or len(set(labels)) != len(labels)
        ):
            raise ValueError("Supply unique explicit logical channel labels")
        if (
            not isinstance(mapping, list)
            or len(mapping) != len(labels)
            or any(type(index) is not int or not 0 <= index < 128 for index in mapping)
            or len(set(mapping)) != len(mapping)
        ):
            raise ValueError(
                "Supply distinct physical channel indices0..127 in logical order"
            )
        dimensions.append(len(labels))
    groups = data["groups"]
    if not isinstance(groups, list) or not 8 <= len(groups) <= 32:
        raise ValueError("Supply8..32 groups, from at least8 independent sources")
    by_source, fingerprints = {}, {}
    for group in groups:
        if (
            not isinstance(group, dict)
            or set(group) != {"source_id", "input", "desired_output"}
            or not isinstance(group["source_id"], str)
            or not 1 <= len(group["source_id"]) <= 128
        ):
            raise ValueError("Every example group requires source_id and paired arrays")
        arrays = []
        for key, width in zip(["input", "desired_output"], dimensions):
            values = group[key]
            if (
                not isinstance(values, list)
                or not 32 <= len(values) <= 256
                or any(
                    not isinstance(row, list)
                    or len(row) != width
                    or any(type(v) not in {int, float} for v in row)
                    for row in values
                )
            ):
                raise ValueError("Each group needs32..256 matching finite PCM rows")
            array = np.asarray(values, np.float64)
            if not np.isfinite(array).all() or np.max(np.abs(array)) > 1:
                raise ValueError("Paired PCM rows must be finite within +/-1")
            arrays.append(array)
        if len(arrays[0]) != len(arrays[1]):
            raise ValueError("Paired array frame counts differ")
        fingerprint = hashlib.sha256(
            b"".join(a.astype("<f8").tobytes() for a in arrays)
        ).hexdigest()
        if (
            fingerprint in fingerprints
            and fingerprints[fingerprint] != group["source_id"]
        ):
            raise ValueError(
                "Identical example groups cannot claim independent source IDs"
            )
        fingerprints[fingerprint] = group["source_id"]
        by_source.setdefault(group["source_id"], []).append(arrays)
    if not 8 <= len(by_source) <= 32:
        raise ValueError("At least8 distinct underlying sources are required")
    result = []
    for source, pairs in sorted(by_source.items()):
        x, y = (np.concatenate([pair[i] for pair in pairs]) for i in [0, 1])
        if len(x) > 2048:
            raise ValueError("Aggregated source exceeds2048 rows")
        result.append((source, x, y))
    return result


def path(groups):
    x = np.concatenate([g[1] / np.sqrt(len(g[1]) * len(groups)) for g in groups])
    y = np.concatenate([g[2] / np.sqrt(len(g[2]) * len(groups)) for g in groups])
    u, singular, vt = np.linalg.svd(x, full_matrices=False)
    condition = float(singular[0] / max(singular[-1], 1e-20))
    if singular[-1] < MIN_SINGULAR or condition > MAX_CONDITION:
        raise ValueError(
            "unregularized input design lacks identifiable independent excitation"
        )
    projected = u.T @ y
    matrices = [
        (vt.T @ ((singular / (singular**2 + penalty))[:, None] * projected)).T
        for penalty in LAMBDAS
    ]
    return matrices, {"singular_values": singular.tolist(), "condition": condition}


def errors(matrix, group):
    delta = group[1] @ matrix.T - group[2]
    return {
        "mse": float(np.mean(delta**2)),
        "rms": float(np.sqrt(np.mean(delta**2))),
        "max_error": float(np.max(np.abs(delta))),
    }


def select(groups):
    scores = [[] for _ in LAMBDAS]
    worst_condition = 0
    for index, heldout in enumerate(groups):
        matrices, support = path(groups[:index] + groups[index + 1 :])
        worst_condition = max(worst_condition, support["condition"])
        for score, matrix in zip(scores, matrices):
            score.append(errors(matrix, heldout)["mse"])
    mean = [float(np.mean(s)) for s in scores]
    # Include OLS and choose the smaller penalty on exact ties.
    selected = min(range(len(LAMBDAS)), key=lambda i: (mean[i], LAMBDAS[i]))
    return selected, mean, worst_condition


def fit(data):
    groups = validate(data)
    result = {
        "accepted": False,
        "parameters": None,
        "reasons": [],
        "source_ids": [g[0] for g in groups],
        "uncertainty_scope": "Source bootstrap plus nested-holdout coefficient variation; conditional on these examples and fixed assumptions. Heuristic abstention, not calibrated confidence or proof of source independence.",
    }
    try:
        full, support = path(groups)
        selected, cv, worst_condition = select(groups)
        nested = []
        for index, heldout in enumerate(groups):
            train = groups[:index] + groups[index + 1 :]
            chosen, _, inner_condition = select(train)
            matrices, fold_support = path(train)
            worst_condition = max(
                worst_condition, inner_condition, fold_support["condition"]
            )
            nested.append(
                {
                    "source_id": heldout[0],
                    "lambda": LAMBDAS[chosen],
                    "matrix": matrices[chosen].tolist(),
                    "errors": errors(matrices[chosen], heldout),
                    "ols_errors": errors(matrices[0], heldout),
                    "identity_errors": errors(
                        np.eye(matrices[0].shape[0], matrices[0].shape[1]), heldout
                    ),
                }
            )
    except ValueError as error:
        result["reasons"].append(str(error))
        return result
    raw = full[selected]
    matrix = raw.copy()
    # Reviewable sparse export; the final changed matrix is revalidated below.
    matrix[np.abs(matrix) < 1e-7] = 0
    matrix = matrix.astype(np.float32).astype(np.float64)
    rng = np.random.default_rng(BOOTSTRAP_SEED)
    bootstrap, deficient = [], 0
    for _ in range(BOOTSTRAPS):
        sample = [groups[i] for i in rng.integers(0, len(groups), len(groups))]
        try:
            matrices, _ = path(sample)
            bootstrap.append(matrices[selected])
        except ValueError:
            deficient += 1
    if bootstrap:
        lower, upper = np.quantile(np.array(bootstrap), [0.025, 0.975], axis=0)
        fold_matrices = np.array([f["matrix"] for f in nested])
        lower = np.minimum(lower, fold_matrices.min(axis=0))
        upper = np.maximum(upper, fold_matrices.max(axis=0))
    else:
        lower, upper = np.full_like(matrix, -2), np.full_like(matrix, 2)
    rms = float(np.sqrt(np.mean([f["errors"]["mse"] for f in nested])))
    full_errors = [errors(matrix, group) for group in groups]
    row_l1 = np.sum(np.abs(matrix), axis=1)
    interval_row_bound = np.sum(np.maximum(np.abs(lower), np.abs(upper)), axis=1)
    if (
        rms > MAX_RMS
        or max(f["errors"]["rms"] for f in nested) > MAX_GROUP_RMS
        or max(f["errors"]["max_error"] for f in nested) > MAX_ERROR
    ):
        result["reasons"].append("nested heldout reference mismatch")
    if (
        max(e["rms"] for e in full_errors) > MAX_GROUP_RMS
        or max(e["max_error"] for e in full_errors) > MAX_ERROR
    ):
        result["reasons"].append("exported matrix reference mismatch")
    if deficient / BOOTSTRAPS > 0.1 or np.max(upper - lower) > MAX_INTERVAL_WIDTH:
        result["reasons"].append("routing coefficient evidence is unstable")
    if np.max(row_l1) > 1 + 1e-9 or np.max(interval_row_bound) > 1 + 1e-9:
        result["reasons"].append("point or interval row-L1 exceeds coherent unit bound")
    result.update(
        selected_lambda=LAMBDAS[selected],
        regularization_cv_mse=cv,
        support=support,
        worst_fold_condition=worst_condition,
        nested_holdouts=nested,
        nested_rms=rms,
        matrix=matrix.tolist(),
        interval_lower=lower.tolist(),
        interval_upper=upper.tolist(),
        max_interval_width=float(np.max(upper - lower)),
        bootstrap_deficient=deficient,
        bootstrap_total=BOOTSTRAPS,
        bootstrap_seed=BOOTSTRAP_SEED,
        row_l1=row_l1.tolist(),
        interval_row_l1_bound=interval_row_bound.tolist(),
        export_max_abs_change=float(np.max(np.abs(matrix - raw))),
        full_errors=full_errors,
    )
    if not result["reasons"]:
        result["accepted"] = True
        result["parameters"] = {
            "input_channel_map": data["input_channel_map"],
            "output_channel_map": data["output_channel_map"],
            "matrix": matrix.ravel().tolist(),
        }
        result["routing_table"] = [
            {
                "input": data["input_labels"][i],
                "physical_input": data["input_channel_map"][i],
                "output": data["output_labels"][o],
                "physical_output": data["output_channel_map"][o],
                "gain_linear": float(matrix[o, i]),
            }
            for o, i in zip(*np.nonzero(matrix))
        ]
    return result
