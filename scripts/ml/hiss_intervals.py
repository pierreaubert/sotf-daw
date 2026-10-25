"""Offline source-grouped noise-interval proposals; never starts native capture."""

import hashlib
import json
import math
from pathlib import Path

import numpy as np
from scipy.optimize import minimize
from scipy.special import expit

FORMAT = "sotf-hiss-labelled-intervals-v1"
MODEL_FORMAT = "sotf-hiss-interval-logistic-v1"
CORE_NAMES = [
    "log_hf_flatness",
    "hf_fraction",
    "dominant_fraction",
    "envelope_cv",
    "log_rms_p95_p05",
    "mean_flux",
    "p95_flux",
    "zero_crossing_fraction",
]
FEATURE_NAMES = [
    prefix + name for prefix in ["channel_min_", "channel_max_"] for name in CORE_NAMES
]
POLICY = {
    "ridge": 0.01,
    "bootstrap_models": 50,
    "bootstrap_seed": 20711026,
    "minimum_source_groups": 8,
    "minimum_class_source_groups": 6,
    "minimum_noise_lower_score": 0.95,
    "maximum_support_distance": 1.5,
    "minimum_cv_noise_coverage": 0.25,
    "maximum_cv_programme_proposals": 0,
    "feature_scale_floor": 0.05,
    "minimum_channel_rms": 1e-5,
    "maximum_model_bytes": 1048576,
}


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def identifier(value):
    if not isinstance(value, str) or not 1 <= len(value) <= 128:
        raise ValueError("Declare bounded nonempty source/setup identifiers")
    return value


def context(value):
    if not isinstance(value, dict) or set(value) != {
        "sample_rate",
        "channels",
        "frequency_hz",
        "capture_setup_id",
        "noise_definition",
    }:
        raise ValueError("Declare the exact rate/channel/cutoff/setup/target context")
    rate, channels, cutoff = (
        value["sample_rate"],
        value["channels"],
        value["frequency_hz"],
    )
    if (
        type(rate) is not int
        or not 44100 <= rate <= 192000
        or type(channels) is not int
        or not 1 <= channels <= 8
    ):
        raise ValueError("Unsupported feature geometry")
    if (
        isinstance(cutoff, bool)
        or not isinstance(cutoff, (int, float))
        or not math.isfinite(cutoff)
        or not 1000 <= cutoff <= min(16000, 0.45 * rate)
    ):
        raise ValueError("Invalid Hiss cutoff context")
    identifier(value["capture_setup_id"])
    if value["noise_definition"] != "unwanted_stationary_hiss":
        raise ValueError("This model is conditional on unwanted stationary hiss labels")
    return value


def features(signal, ctx):
    ctx = context(ctx)
    rate = ctx["sample_rate"]
    channels = ctx["channels"]
    x = np.asarray(signal, dtype=np.float32)
    if x.shape != (rate, channels) or not np.isfinite(x).all() or np.max(abs(x)) > 32:
        raise ValueError("Use exactly one finite second of physical-channel PCM")
    rms = np.sqrt(np.mean(x.astype(float) ** 2, axis=0))
    if np.any(rms < POLICY["minimum_channel_rms"]):
        raise ValueError(
            "Every channel needs noise excitation; silent/near-silent channels abstain"
        )
    x = x.astype(float) / rms
    n = round(0.02 * rate)
    hop = round(0.01 * rate)
    window = 0.5 - 0.5 * np.cos(2 * np.pi * np.arange(n) / n)
    frames = np.lib.stride_tricks.sliding_window_view(x, n, axis=0)[::hop]
    raw = abs(np.fft.rfft(frames * window, axis=-1)) ** 2
    weights = np.full(raw.shape[-1], 2.0)
    weights[0] = 1
    if n % 2 == 0:
        weights[-1] = 1
    power = raw * weights
    normalized = power / np.maximum(power.sum(axis=-1, keepdims=True), 1e-20)
    hz = np.fft.rfftfreq(n, 1 / rate)
    hf = hz >= ctx["frequency_hz"]
    floor = np.maximum(raw[:, :, hf], 1e-12)
    flat = np.mean(
        np.mean(np.log(floor), axis=-1) - np.log(np.mean(floor, axis=-1)), axis=0
    )
    fraction = np.mean(normalized[:, :, hf].sum(axis=-1), axis=0)
    dominant = np.mean(normalized.max(axis=-1), axis=0)
    envelope = np.sqrt(np.mean(frames * frames, axis=-1))
    cv = envelope.std(axis=0) / np.maximum(envelope.mean(axis=0), 1e-12)
    ratio = np.minimum(
        np.log(
            np.maximum(np.quantile(envelope, 0.95, axis=0), 1e-12)
            / np.maximum(np.quantile(envelope, 0.05, axis=0), 1e-12)
        ),
        20,
    )
    flux = np.sum(abs(np.diff(normalized, axis=0)), axis=-1)
    zcr = np.mean(np.signbit(x[1:]) != np.signbit(x[:-1]), axis=0)
    each = np.column_stack(
        [
            flat,
            fraction,
            dominant,
            cv,
            ratio,
            np.mean(flux, axis=0),
            np.quantile(flux, 0.95, axis=0),
            zcr,
        ]
    )
    result = np.concatenate([each.min(axis=0), each.max(axis=0)])
    if result.shape != (16,) or not np.isfinite(result).all():
        raise ValueError("Invalid interval features")
    return result


def classical(feature):
    f = np.asarray(feature)
    return bool(
        np.exp(f[0]) > 0.35
        and f[1] > 0.1
        and f[8 + 2] < 0.03
        and f[8 + 3] < 0.15
        and f[8 + 4] < math.log(2)
    )


def prepare(data):
    if (
        not isinstance(data, dict)
        or set(data) != {"format", "data_origin", "context", "protocol", "records"}
        or data["format"] != FORMAT
    ):
        raise ValueError("Use the explicit labelled interval session schema")
    if (
        data["data_origin"] not in {"labelled_recordings", "synthetic_validation"}
        or not isinstance(data["protocol"], dict)
        or set(data["protocol"]) != {"whole_source_grouping"}
        or data["protocol"]["whole_source_grouping"] is not True
    ):
        raise ValueError("Declare data origin and whole-source grouping")
    ctx = context(data["context"])
    records = data["records"]
    if not isinstance(records, list) or not 16 <= len(records) <= 256:
        raise ValueError("Supply16..256 labelled one-second intervals")
    if len(records) * ctx["sample_rate"] * ctx["channels"] > 32_000_000:
        raise ValueError("Labelled session exceeds32 million aggregate PCM samples")
    x = []
    labels = []
    groups = []
    ids = set()
    identities = {}
    size = ctx["sample_rate"] * ctx["channels"] * 4
    for record in records:
        if set(record) != {"id", "source_group", "label", "audio_file", "audio_sha256"}:
            raise ValueError("Unknown/missing interval fields")
        rid = identifier(record["id"])
        group = identifier(record["source_group"])
        if rid in ids or record["label"] not in {"noise_only", "programme"}:
            raise ValueError("Use unique interval IDs and explicit labels")
        ids.add(rid)
        with Path(record["audio_file"]).open("rb") as f:
            raw = f.read(size + 1)
        if len(raw) != size or digest(raw) != record["audio_sha256"]:
            raise ValueError("Interval PCM size/SHA256 mismatch")
        previous = identities.get(record["audio_sha256"])
        if previous is not None and previous != (group, record["label"]):
            raise ValueError(
                "Identical PCM cannot be independent sources or conflicting labels"
            )
        identities[record["audio_sha256"]] = (group, record["label"])
        x.append(
            features(np.frombuffer(raw, dtype="<f4").reshape(-1, ctx["channels"]), ctx)
        )
        labels.append(record["label"] == "noise_only")
        groups.append(group)
    x = np.asarray(x)
    labels = np.asarray(labels, dtype=float)
    groups = np.asarray(groups)
    if len(set(groups)) > 32:
        raise ValueError("At most32 whole-source groups")
    # Amplitude-normalized observations can coincide despite different bytes.
    for i in range(len(x)):
        for j in range(i):
            if labels[i] != labels[j] and np.max(abs(x[i] - x[j])) < 1e-6:
                raise ValueError(
                    "Conflicting labels for indistinguishable interval features"
                )
    return x, labels, groups


def solve(x, labels, weights):
    weights = weights / weights.sum()
    center = np.sum(x * weights[:, None], axis=0)
    scale = np.maximum(
        np.sqrt(np.sum((x - center) ** 2 * weights[:, None], axis=0)),
        POLICY["feature_scale_floor"],
    )
    design = np.column_stack([(x - center) / scale, np.ones(len(x))])
    penalty = np.full(17, POLICY["ridge"])
    penalty[-1] = 1e-6

    def terms(theta):
        z = design @ theta
        p = expit(z)
        loss = float(
            np.dot(weights, np.logaddexp(0, z) - labels * z)
            + np.dot(penalty * theta, theta) / 2
        )
        grad = design.T @ (weights * (p - labels)) + penalty * theta
        hessian = design.T @ ((weights * p * (1 - p))[:, None] * design) + np.diag(
            penalty
        )
        return loss, grad, hessian

    result = minimize(
        lambda v: terms(v)[0],
        np.zeros(17),
        method="trust-exact",
        jac=lambda v: terms(v)[1],
        hess=lambda v: terms(v)[2],
        options={"maxiter": 100, "gtol": 1e-9},
    )
    if not np.isfinite(result.x).all() or np.linalg.norm(terms(result.x)[1]) > 1e-7:
        raise ValueError("Noise-interval optimizer did not converge")
    return {
        "center": center.tolist(),
        "scale": scale.tolist(),
        "weights": result.x.tolist(),
    }


def bundle(x, labels, groups):
    names, index, counts = np.unique(groups, return_inverse=True, return_counts=True)
    weights = 1 / counts[index]
    base = solve(x, labels, weights)
    rng = np.random.default_rng(POLICY["bootstrap_seed"])
    models = []
    for _ in range(POLICY["bootstrap_models"]):
        multiplicity = np.bincount(
            rng.integers(len(names), size=len(names)), minlength=len(names)
        )
        w = weights * multiplicity[index]
        if len(set(labels[w > 0])) != 2:
            continue
        models.append(solve(x, labels, w))
    if len(models) < 0.95 * POLICY["bootstrap_models"]:
        raise ValueError("Too few class-complete source bootstraps")
    return {"main": base, "bootstrap": models, "noise_support": x[labels == 1].tolist()}


def score(model, feature):
    f = np.asarray(feature, dtype=float)

    def probability(m):
        z = (f - np.asarray(m["center"])) / m["scale"]
        return float(expit(np.dot(z, m["weights"][:-1]) + m["weights"][-1]))

    base = model["main"]
    distance = float(
        np.min(
            np.sqrt(
                np.mean(
                    ((np.asarray(model["noise_support"]) - f) / base["scale"]) ** 2,
                    axis=1,
                )
            )
        )
    )
    lower = float(np.quantile([probability(m) for m in model["bootstrap"]], 0.05))
    review = (
        lower >= POLICY["minimum_noise_lower_score"]
        and distance <= POLICY["maximum_support_distance"]
    )
    return {
        "review_candidate": bool(review),
        "noise_score": probability(base),
        "bootstrap_lower_score": lower,
        "nearest_noise_support_distance": distance,
        "automatic_capture": False,
    }


def fit(data):
    x, labels, groups = prepare(data)
    names = np.unique(groups)
    reasons = []
    if len(names) < POLICY["minimum_source_groups"] or any(
        len(set(groups[labels == label])) < POLICY["minimum_class_source_groups"]
        for label in [0, 1]
    ):
        reasons.append("insufficient_independent_labelled_sources")
    result = {
        "accepted": False,
        "model": None,
        "reasons": reasons,
        "source_groups": len(names),
        "intervals": len(labels),
        "data_origin": data["data_origin"],
        "automatic_capture": False,
    }
    if reasons:
        return result
    cv = []
    for group in names:
        held = groups == group
        m = bundle(x[~held], labels[~held], groups[~held])
        for i in np.flatnonzero(held):
            cv.append(
                {
                    "record_id": data["records"][int(i)]["id"],
                    "source_group": str(group),
                    "label": bool(labels[i]),
                    **score(m, x[i]),
                    "classical_review_candidate": classical(x[i]),
                }
            )
    false = sum(r["review_candidate"] and not r["label"] for r in cv)
    noise = sum(r["review_candidate"] and r["label"] for r in cv)
    coverage = noise / sum(labels)
    if false > POLICY["maximum_cv_programme_proposals"]:
        reasons.append("held_source_programme_review_candidates")
    if coverage < POLICY["minimum_cv_noise_coverage"]:
        reasons.append("insufficient_held_source_noise_coverage")
    result.update(
        cross_validation=cv,
        held_source_programme_proposals=int(false),
        held_source_noise_coverage=float(coverage),
    )
    if not reasons:
        result["accepted"] = True
        result["model"] = {
            "format": MODEL_FORMAT,
            "admitted": True,
            "automatic_capture": False,
            "context": dict(data["context"]),
            "feature_names": list(FEATURE_NAMES),
            "policy": dict(POLICY),
            "parameters": bundle(x, labels, groups),
            "training_data_origin": data["data_origin"],
            "training_identities": [
                {k: r[k] for k in ["source_group", "audio_sha256", "label"]}
                for r in data["records"]
            ],
            "qualification": "Conditional source-labelled review proposals only; scores/bootstrap bounds are not calibrated probabilities or safe-noise guarantees.",
        }
    return result


def load_model(path, expected_sha256):
    with Path(path).open("rb") as f:
        raw = f.read(POLICY["maximum_model_bytes"] + 1)
    if len(raw) > POLICY["maximum_model_bytes"] or digest(raw) != expected_sha256:
        raise ValueError("Noise-interval model size/SHA256 mismatch")
    return validate_model(json.loads(raw))


def validate_model(m):
    if (
        not isinstance(m, dict)
        or m.get("format") != MODEL_FORMAT
        or m.get("admitted") is not True
        or m.get("automatic_capture") is not False
        or m.get("feature_names") != FEATURE_NAMES
        or m.get("policy") != POLICY
    ):
        raise ValueError("Unsupported/unadmitted noise-interval model")
    context(m["context"])
    parameters = m["parameters"]
    if not 48 <= len(parameters["bootstrap"]) <= 50:
        raise ValueError("Invalid bootstrap ensemble size")
    for item in [parameters["main"], *parameters["bootstrap"]]:
        for key, size in [("center", 16), ("scale", 16), ("weights", 17)]:
            value = np.asarray(item[key], dtype=float)
            if value.shape != (size,) or not np.isfinite(value).all():
                raise ValueError("Invalid model numeric shape")
        if np.any(np.asarray(item["scale"]) < POLICY["feature_scale_floor"]):
            raise ValueError("Invalid model scale")
    support = np.asarray(parameters["noise_support"], dtype=float)
    if (
        support.ndim != 2
        or not 1 <= len(support) <= 256
        or support.shape[1] != 16
        or not np.isfinite(support).all()
    ):
        raise ValueError("Invalid model support")
    return m


def propose(signal, ctx, model=None):
    ctx = context(ctx)
    x = np.asarray(signal, dtype=np.float32)
    if (
        x.ndim != 2
        or x.shape[1] != ctx["channels"]
        or not 1 <= x.size <= 4_000_000
        or not np.isfinite(x).all()
        or np.max(abs(x)) > 32
    ):
        raise ValueError("Unsupported proposal PCM")
    if model is not None:
        try:
            validate_model(model)
        except (ValueError, KeyError, TypeError):
            model = None
    rows = []
    rate = ctx["sample_rate"]
    for start in range(0, len(x) - rate + 1, rate):
        row = {
            "start_frame": start,
            "end_frame": start + rate,
            "available_at_frame": start + rate,
            "review_candidate": False,
            "automatic_capture": False,
        }
        if model is None:
            row["reason"] = "model_unavailable"
        elif model["context"] != ctx:
            row["reason"] = "context_mismatch"
        else:
            try:
                f = features(x[start : start + rate], ctx)
            except ValueError:
                row["reason"] = "insufficient_interval_excitation"
            else:
                row.update(score(model["parameters"], f))
                row["classical_review_candidate"] = classical(f)
                row["reason"] = (
                    "review_only"
                    if row["review_candidate"]
                    else "uncertain_or_outside_labelled_support"
                )
        rows.append(row)
    return {
        "intervals": rows,
        "input_frames": len(x),
        "unused_final_frames": len(x) % rate,
        "automatic_capture": False,
        "profile_changed": False,
        "qualification": "Offline proposals only; completed1s windows, no EOF padding or future backdating. No native profile is created/replaced.",
    }
