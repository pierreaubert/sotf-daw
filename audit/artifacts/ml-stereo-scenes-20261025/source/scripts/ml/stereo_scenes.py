"""Source-labelled offline scene classification; native settings remain explicit."""

import hashlib
import json
import math
import subprocess
from pathlib import Path

import numpy as np
from scipy.special import softmax

SESSION_FORMAT = "sotf-stereo-scene-labels-v1"
MODEL_FORMAT = "sotf-stereo-scene-rda-v1"
FEATURE_NAMES = [
    band + "_" + name
    for band in ["full", "low", "mid", "high"]
    for name in ["side_fraction", "correlation", "balance"]
] + [
    "low_energy_fraction",
    "mid_energy_fraction",
    "high_energy_fraction",
    "envelope_cv",
]
POLICY = {
    "covariance_shrinkage": 0.2,
    "feature_scale_floor": 0.05,
    "bootstrap_models": 50,
    "bootstrap_seed": 20871026,
    "minimum_source_groups": 8,
    "minimum_class_source_groups": 6,
    "minimum_lower_score": 0.95,
    "maximum_support_distance": 1.5,
    "minimum_cv_coverage": 0.25,
    "maximum_cv_confident_errors": 0,
    "minimum_rms": 1e-5,
    "maximum_sample_peak": 0.999,
    "maximum_rms_gain_db": 6.0,
    "minimum_rms_gain_db": -6.0,
    "minimum_mono_energy_fraction": 0.01,
    "maximum_mono_relative_rms_error": 1e-6,
    "maximum_model_bytes": 1048576,
}


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def identifier(value):
    if not isinstance(value, str) or not 1 <= len(value) <= 128:
        raise ValueError("Use bounded nonempty source/scene identifiers")
    return value


def sha(value):
    if (
        not isinstance(value, str)
        or len(value) != 64
        or any(c not in "0123456789abcdef" for c in value)
    ):
        raise ValueError("Declare explicit lowercase SHA256")
    return value


def context(value):
    if not isinstance(value, dict) or set(value) != {
        "sample_rate",
        "channels",
        "scene_definition_id",
    }:
        raise ValueError("Declare exact stereo rate and scene-definition context")
    if (
        type(value["sample_rate"]) is not int
        or not 44100 <= value["sample_rate"] <= 192000
        or type(value["channels"]) is not int
        or value["channels"] != 2
    ):
        raise ValueError("Unsupported scene geometry")
    identifier(value["scene_definition_id"])
    return value


def native_params(p):
    limits = {
        "width": (0, 2),
        "low_mid_freq": (80, 1000),
        "mid_high_freq": (1000, 10000),
        "low_width": (0, 2),
        "mid_width": (0, 2),
        "high_width": (0, 2),
        "mix": (0, 1),
    }
    if (
        not isinstance(p, dict)
        or set(p) != {*limits, "mono_bass"}
        or type(p["mono_bass"]) is not bool
    ):
        raise ValueError("Supply the complete native eight-control policy")
    for key, (lo, hi) in limits.items():
        if (
            isinstance(p[key], bool)
            or not isinstance(p[key], (int, float))
            or not math.isfinite(p[key])
            or not lo <= p[key] <= hi
        ):
            raise ValueError("Policy control outside host registry range")
    if p["low_mid_freq"] >= p["mid_high_freq"]:
        raise ValueError("Policy crossovers must be strictly ordered")
    return p


def features(signal, ctx):
    ctx = context(ctx)
    rate = ctx["sample_rate"]
    x = np.asarray(signal, dtype=np.float32)
    if (
        x.ndim != 2
        or x.shape[1] != 2
        or not rate <= len(x) <= 10 * rate
        or x.size > 4_000_000
        or not np.isfinite(x).all()
        or np.max(abs(x)) > 32
    ):
        raise ValueError(
            "Use1..10s of finite stereo PCM within+/-32 and4million samples"
        )
    x = x.astype(float)
    rms = np.sqrt(np.mean(x * x))
    if rms < POLICY["minimum_rms"]:
        raise ValueError("Insufficient scene excitation")
    x /= rms
    n, hop = round(0.02 * rate), round(0.01 * rate)
    starts = list(range(0, len(x) - n + 1, hop))
    if starts[-1] != len(x) - n:
        starts.append(len(x) - n)  # Cover final real PCM, with no synthetic padding.
    frames = np.lib.stride_tricks.sliding_window_view(x, n, axis=0)[starts]
    window = 0.5 - 0.5 * np.cos(2 * np.pi * np.arange(n) / n)
    spectrum = np.fft.rfft(frames * window, axis=-1)
    weights = np.full(spectrum.shape[-1], 2.0)
    weights[0] = 1
    if n % 2 == 0:
        weights[-1] = 1
    hz = np.fft.rfftfreq(n, 1 / rate)
    l, r = spectrum[:, 0], spectrum[:, 1]
    lp, rp = abs(l) ** 2 * weights, abs(r) ** 2 * weights
    sp = abs((l - r) / 2) ** 2 * weights
    cross = (l * np.conj(r)).real * weights
    total = float(np.sum(lp + rp))
    result, fractions = [], []
    for mask in [
        np.ones(len(hz), dtype=bool),
        hz < 250,
        (hz >= 250) & (hz < 4000),
        hz >= 4000,
    ]:
        el, er = float(lp[:, mask].sum()), float(rp[:, mask].sum())
        mass = el + er
        fractions.append(mass / total)
        if mass <= total * 1e-12:
            result.extend([0.0, 0.0, 0.0])
        else:
            result.extend(
                [
                    float(sp[:, mask].sum()) / (mass / 2),
                    float(
                        np.clip(
                            cross[:, mask].sum() / max(math.sqrt(el * er), 1e-20), -1, 1
                        )
                    ),
                    (el - er) / mass,
                ]
            )
    envelope = np.sqrt(np.mean(frames**2, axis=(1, 2)))
    result.extend([*fractions[1:], float(envelope.std() / max(envelope.mean(), 1e-12))])
    result = np.asarray(result)
    if result.shape != (16,) or not np.isfinite(result).all():
        raise ValueError("Invalid scene features")
    return result


def solve(x, labels, weights, classes):
    weights = weights / weights.sum()
    center = np.sum(x * weights[:, None], axis=0)
    scale = np.maximum(
        np.sqrt(np.sum((x - center) ** 2 * weights[:, None], axis=0)),
        POLICY["feature_scale_floor"],
    )
    z = (x - center) / scale
    means = np.array(
        [
            np.average(z[labels == c], axis=0, weights=weights[labels == c])
            for c in range(classes)
        ]
    )
    residual = z - means[labels]
    covariance = (residual.T * weights) @ residual
    a = POLICY["covariance_shrinkage"]
    covariance = (1 - a) * covariance + a * np.eye(16)
    precision = np.linalg.solve(covariance, np.eye(16))
    return {
        "center": center.tolist(),
        "scale": scale.tolist(),
        "means": means.tolist(),
        "precision": precision.tolist(),
    }


def bundle(x, labels, groups, classes):
    names, inverse, counts = np.unique(groups, return_inverse=True, return_counts=True)
    weights = 1 / counts[inverse]
    rng = np.random.default_rng(POLICY["bootstrap_seed"])
    bootstrap = []
    for _ in range(POLICY["bootstrap_models"]):
        multiplicity = np.bincount(
            rng.integers(len(names), size=len(names)), minlength=len(names)
        )
        w = weights * multiplicity[inverse]
        if len(set(labels[w > 0])) != classes:
            continue
        selected = w > 0
        bootstrap.append(solve(x[selected], labels[selected], w[selected], classes))
    if len(bootstrap) < 48:
        raise ValueError("Insufficient class-complete source bootstraps")
    return {
        "main": solve(x, labels, weights, classes),
        "bootstrap": bootstrap,
        "support": x.tolist(),
        "support_labels": labels.tolist(),
    }


def probabilities(model, feature):
    delta = (feature - np.asarray(model["center"])) / model["scale"] - np.asarray(
        model["means"]
    )
    logits = -0.5 * np.einsum(
        "ci,ij,cj->c", delta, np.asarray(model["precision"]), delta
    )
    if not np.isfinite(logits).all():
        raise ValueError("Invalid scene inference arithmetic")
    return softmax(logits)


def score(parameters, feature):
    main = parameters["main"]
    scores = probabilities(main, feature)
    label = int(np.argmax(scores))
    lower = float(
        np.quantile(
            [probabilities(m, feature)[label] for m in parameters["bootstrap"]], 0.05
        )
    )
    support = np.asarray(parameters["support"])[
        np.asarray(parameters["support_labels"]) == label
    ]
    distance = float(
        np.min(np.sqrt(np.mean(((support - feature) / main["scale"]) ** 2, axis=1)))
    )
    return {
        "class_index": label,
        "scene_scores": scores.tolist(),
        "bootstrap_lower_score": lower,
        "support_distance": distance,
        "confident": lower >= POLICY["minimum_lower_score"]
        and distance <= POLICY["maximum_support_distance"],
    }


def prepare(data):
    keys = {
        "format",
        "context",
        "data_origin",
        "renderer_sha256",
        "settings_by_scene",
        "protocol",
        "records",
    }
    if (
        not isinstance(data, dict)
        or set(data) != keys
        or data["format"] != SESSION_FORMAT
    ):
        raise ValueError("Use the explicit source-labelled scene session schema")
    ctx = context(data["context"])
    sha(data["renderer_sha256"])
    if (
        data["data_origin"]
        not in {
            "labelled_recordings",
            "synthetic_validation",
        }
        or not isinstance(data["protocol"], dict)
        or set(data["protocol"]) != {"whole_source_grouping"}
        or data["protocol"]["whole_source_grouping"] is not True
    ):
        raise ValueError("Declare source grouping and data origin")
    table = data["settings_by_scene"]
    if not isinstance(table, dict) or not 2 <= len(table) <= 4:
        raise ValueError("Declare2..4 scene names and user-chosen native settings")
    names = sorted(table)
    for name in names:
        identifier(name)
        native_params(table[name])
    records = data["records"]
    if not isinstance(records, list) or not 16 <= len(records) <= 256:
        raise ValueError("Supply16..256 labelled source records")
    seen, identities, total = set(), {}, 0
    matrix, labels, groups = [], [], []
    for record in records:
        if not isinstance(record, dict) or set(record) != {
            "id",
            "source_group",
            "scene",
            "frames",
            "audio_file",
            "audio_sha256",
        }:
            raise ValueError("Unknown/missing labelled record fields")
        rid, group = identifier(record["id"]), identifier(record["source_group"])
        if (
            rid in seen
            or record["scene"] not in names
            or type(record["frames"]) is not int
            or not ctx["sample_rate"] <= record["frames"] <= ctx["sample_rate"] * 10
        ):
            raise ValueError("Invalid scene, geometry or duplicate record")
        seen.add(rid)
        total += record["frames"] * 2
        if total > 32_000_000:
            raise ValueError("Session exceeds32million aggregate PCM samples")
        size = record["frames"] * 2 * 4
        with Path(record["audio_file"]).open("rb") as f:
            raw = f.read(size + 1)
        if len(raw) != size or digest(raw) != sha(record["audio_sha256"]):
            raise ValueError("Labelled PCM size/SHA256 mismatch")
        identity = identities.get(record["audio_sha256"])
        if identity is not None and identity != (group, record["scene"]):
            raise ValueError(
                "Identical audio cannot be independent sources or conflicting labels"
            )
        identities[record["audio_sha256"]] = (group, record["scene"])
        matrix.append(features(np.frombuffer(raw, dtype="<f4").reshape(-1, 2), ctx))
        labels.append(names.index(record["scene"]))
        groups.append(group)
    x, labels, groups = np.asarray(matrix), np.asarray(labels), np.asarray(groups)
    if len(set(groups)) > 32:
        raise ValueError("At most32 independent source groups")
    for i in range(len(x)):
        for j in range(i):
            if labels[i] != labels[j] and np.max(abs(x[i] - x[j])) < 1e-6:
                raise ValueError(
                    "Conflicting labels for indistinguishable scene features"
                )
    return x, labels, groups, names


def fit(data):
    x, labels, groups, names = prepare(data)
    result = {"accepted": False, "model": None, "reasons": [], "automatic_apply": False}
    if len(set(groups)) < POLICY["minimum_source_groups"] or any(
        len(set(groups[labels == c])) < POLICY["minimum_class_source_groups"]
        for c in range(len(names))
    ):
        result["reasons"].append("insufficient_independent_scene_sources")
        return result
    cv = []
    for group in np.unique(groups):
        train = groups != group
        parameters = bundle(x[train], labels[train], groups[train], len(names))
        # Same source folds and source weighting, one-feature nearest-centroid baseline.
        main = parameters["main"]
        centers = np.asarray(main["means"])[:, 0] * main["scale"][0] + main["center"][0]
        for i in np.flatnonzero(~train):
            value = score(parameters, x[i])
            cv.append(
                {
                    "record_id": data["records"][int(i)]["id"],
                    "source_group": str(group),
                    "true_class": int(labels[i]),
                    "baseline_class": int(np.argmin(abs(centers - x[i, 0]))),
                    **value,
                }
            )
    wrong = sum(r["confident"] and r["class_index"] != r["true_class"] for r in cv)
    coverage = sum(r["confident"] for r in cv) / len(cv)
    class_source_coverage = []
    for label in range(len(names)):
        by_source = []
        for group in sorted(
            {r["source_group"] for r in cv if r["true_class"] == label}
        ):
            selected = [
                r for r in cv if r["true_class"] == label and r["source_group"] == group
            ]
            by_source.append(sum(r["confident"] for r in selected) / len(selected))
        class_source_coverage.append(float(np.mean(by_source)))
    result.update(
        cross_validation=cv,
        confident_errors=wrong,
        coverage=coverage,
        class_source_coverage=class_source_coverage,
        baseline_accuracy=sum(r["baseline_class"] == r["true_class"] for r in cv)
        / len(cv),
    )
    if wrong > POLICY["maximum_cv_confident_errors"]:
        result["reasons"].append("held_source_confident_class_errors")
    if (
        coverage < POLICY["minimum_cv_coverage"]
        or min(class_source_coverage) < POLICY["minimum_cv_coverage"]
    ):
        result["reasons"].append("insufficient_held_source_coverage")
    if not result["reasons"]:
        result["accepted"] = True
        result["model"] = {
            "format": MODEL_FORMAT,
            "admitted": True,
            "context": dict(data["context"]),
            "class_names": names,
            "settings_by_scene": json.loads(json.dumps(data["settings_by_scene"])),
            "renderer_sha256": data["renderer_sha256"],
            "feature_names": list(FEATURE_NAMES),
            "policy": dict(POLICY),
            "parameters": bundle(x, labels, groups, len(names)),
            "training_data_origin": data["data_origin"],
            "training_identities": [
                {k: r[k] for k in ["source_group", "scene", "audio_sha256"]}
                for r in data["records"]
            ],
            "automatic_apply": False,
            "qualification": "Conditional source-labelled scene classification only. User supplies scene-to-settings policy. Bootstrap scores are not calibrated probabilities or perceptual preference confidence; native audio qualification occurs separately before export.",
        }
    return result


def validate_model(m):
    required = {
        "format",
        "admitted",
        "context",
        "class_names",
        "settings_by_scene",
        "renderer_sha256",
        "feature_names",
        "policy",
        "parameters",
        "training_data_origin",
        "training_identities",
        "automatic_apply",
        "qualification",
    }
    if (
        not isinstance(m, dict)
        or set(m) != required
        or m.get("format") != MODEL_FORMAT
        or m.get("admitted") is not True
        or m.get("automatic_apply") is not False
        or m.get("feature_names") != FEATURE_NAMES
        or m.get("policy") != POLICY
    ):
        raise ValueError("Unsupported/unadmitted scene model")
    context(m["context"])
    sha(m["renderer_sha256"])
    names = m["class_names"]
    if (
        not isinstance(names, list)
        or not 2 <= len(names) <= 4
        or any(not isinstance(n, str) for n in names)
        or names != sorted(set(names))
        or set(m["settings_by_scene"]) != set(names)
    ):
        raise ValueError("Invalid scene classes/settings")
    for name in names:
        identifier(name)
        native_params(m["settings_by_scene"][name])
    p = m["parameters"]
    if not isinstance(p, dict) or set(p) != {
        "main",
        "bootstrap",
        "support",
        "support_labels",
    }:
        raise ValueError("Invalid scene parameter schema")
    if not isinstance(p["bootstrap"], list) or not 48 <= len(p["bootstrap"]) <= 50:
        raise ValueError("Invalid scene ensemble")
    for model in [p["main"], *p["bootstrap"]]:
        if not isinstance(model, dict) or set(model) != {
            "center",
            "scale",
            "means",
            "precision",
        }:
            raise ValueError("Invalid scene classifier schema")
        for key, shape in [
            ("center", (16,)),
            ("scale", (16,)),
            ("means", (len(names), 16)),
            ("precision", (16, 16)),
        ]:
            values = np.asarray(model[key], dtype=float)
            typed = np.asarray(model[key], dtype=object)
            if any(type(v) not in (float, int) for v in typed.flat) or (
                values.shape != shape
                or not np.isfinite(values).all()
                or np.max(abs(values)) > 1e6
            ):
                raise ValueError("Invalid scene parameter shape/value")
        if np.min(model["scale"]) < POLICY["feature_scale_floor"]:
            raise ValueError("Invalid feature scale")
        precision = np.asarray(model["precision"])
        if (
            not np.allclose(precision, precision.T, atol=1e-10, rtol=0)
            or np.linalg.eigvalsh(precision).min() <= 1e-8
        ):
            raise ValueError("Scene precision must be symmetric positive definite")
    support = np.asarray(p["support"], dtype=float)
    labels = p["support_labels"]
    if (
        support.ndim != 2
        or not 16 <= len(support) <= 256
        or support.shape[1] != 16
        or not np.isfinite(support).all()
        or np.max(abs(support)) > 1e6
        or not isinstance(labels, list)
        or len(labels) != len(support)
        or any(type(v) is not int or not 0 <= v < len(names) for v in labels)
        or set(labels) != set(range(len(names)))
    ):
        raise ValueError("Invalid labelled scene support")
    identities = m["training_identities"]
    if (
        m["training_data_origin"] not in {"labelled_recordings", "synthetic_validation"}
        or not isinstance(identities, list)
        or len(identities) != len(support)
    ):
        raise ValueError("Declare matching training provenance")
    sources, by_class, known = set(), [set() for _ in names], {}
    for index, record in enumerate(identities):
        if (
            not isinstance(record, dict)
            or set(record) != {"source_group", "scene", "audio_sha256"}
            or record["scene"] not in names
            or names.index(record["scene"]) != labels[index]
        ):
            raise ValueError("Training provenance/label mismatch")
        group = identifier(record["source_group"])
        identity = sha(record["audio_sha256"])
        if identity in known and known[identity] != (group, record["scene"]):
            raise ValueError("Conflicting training identities")
        known[identity] = (group, record["scene"])
        sources.add(group)
        by_class[labels[index]].add(group)
    if (
        not POLICY["minimum_source_groups"] <= len(sources) <= 32
        or min(map(len, by_class)) < POLICY["minimum_class_source_groups"]
    ):
        raise ValueError("Insufficient declared model sources")
    return m


def load_model(path, expected_sha256):
    sha(expected_sha256)
    with Path(path).open("rb") as f:
        raw = f.read(POLICY["maximum_model_bytes"] + 1)
    if len(raw) > POLICY["maximum_model_bytes"] or digest(raw) != expected_sha256:
        raise ValueError("Scene model size/SHA256 mismatch")
    return validate_model(json.loads(raw))


def file_digest(path):
    result = hashlib.sha256()
    with Path(path).open("rb") as f:
        while block := f.read(1048576):
            result.update(block)
    return result.hexdigest()


def propose(signal, ctx, model, renderer, work):
    from stereo_imager_native import preset, render

    ctx = context(ctx)
    result = {
        "accepted": False,
        "reasons": [],
        "suggested_params": None,
        "classical_fallback": preset(),
        "automatic_apply": False,
        "qualification": "Offline scene suggestion only; user supplies class-to-settings policy. Scores/bootstrap are not calibrated probabilities or preference evidence. No realtime settings change or normalization.",
    }
    if model is None:
        result["reasons"].append("model_unavailable")
        return result
    try:
        validate_model(model)
    except (ValueError, KeyError, TypeError, np.linalg.LinAlgError):
        result["reasons"].append("invalid_model")
        return result
    if model["context"] != ctx:
        result["reasons"].append("context_mismatch")
        return result

    def renderer_matches():
        try:
            return file_digest(renderer) == model["renderer_sha256"]
        except (OSError, TypeError):
            return False

    if not renderer_matches():
        result["reasons"].append("native_renderer_identity_mismatch")
        return result
    try:
        f = features(signal, ctx)
        value = score(model["parameters"], f)
    except (ValueError, FloatingPointError):
        result["reasons"].append("insufficient_or_invalid_scene_features")
        return result
    result.update(value, features=f.tolist())
    scene = model["class_names"][value["class_index"]]
    result["scene"] = scene
    if not value["confident"]:
        result["reasons"].append("uncertain_or_outside_labelled_support")
        return result
    params = model["settings_by_scene"][scene]
    x = np.asarray(signal, dtype=np.float32)
    try:
        y, metadata = render(
            x, renderer, work, params, rate=ctx["sample_rate"], zeros=4096
        )
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        result["reasons"].append("native_render_failed")
        result["native_error"] = str(error)
        return result
    if not renderer_matches():
        result["reasons"].append("native_renderer_changed")
        return result
    padded = np.pad(x.astype(float), ((0, 4096), (0, 0)))
    input_rms = float(np.sqrt(np.mean(padded**2)))
    output_rms = float(np.sqrt(np.mean(y.astype(float) ** 2)))
    gain_db = 20 * math.log10(max(output_rms, 1e-20) / max(input_rms, 1e-20))
    mono = np.mean(padded, axis=1)
    mono_relative = float(
        np.sqrt(np.mean((y.astype(float).mean(axis=1) - mono) ** 2))
        / max(input_rms, 1e-6)
    )
    mono_fraction = float(np.mean(mono**2) / max(input_rms**2, 1e-20))
    metrics = {
        "input_sample_peak": float(np.max(abs(x))),
        "output_sample_peak": float(np.max(abs(y))),
        "rms_gain_db": gain_db,
        "mono_relative_rms_error": mono_relative,
        "input_mono_energy_fraction": mono_fraction,
    }
    result.update(native_metadata=metadata, native_metrics=metrics)
    if (
        metrics["output_sample_peak"] > POLICY["maximum_sample_peak"]
        or metrics["input_sample_peak"] > POLICY["maximum_sample_peak"]
    ):
        result["reasons"].append("insufficient_sample_headroom")
    if not POLICY["minimum_rms_gain_db"] <= gain_db <= POLICY["maximum_rms_gain_db"]:
        result["reasons"].append("excessive_rms_level_change")
    if mono_fraction < POLICY["minimum_mono_energy_fraction"]:
        result["reasons"].append("insufficient_original_mono_energy")
    if mono_relative > POLICY["maximum_mono_relative_rms_error"]:
        result["reasons"].append("native_mono_fold_mismatch")
    if not result["reasons"]:
        # Verify the exact JSON controls reconstruct a fresh native processor
        # across another callback size, including the same explicit observation zeros.
        try:
            reopened, reopened_metadata = render(
                x,
                renderer,
                Path(work).with_name(Path(work).name + "-reopen"),
                json.loads(json.dumps(metadata["effective_params"])),
                rate=ctx["sample_rate"],
                callback=4096,
                zeros=4096,
            )
        except (ValueError, OSError, subprocess.SubprocessError) as error:
            result["reasons"].append("native_preset_reconstruction_failed")
            result["native_error"] = str(error)
            return result
        error = float(np.max(abs(reopened.astype(float) - y.astype(float))))
        result.update(
            native_reopen_error=error,
            native_reopen_sha256=reopened_metadata["output_sha256"],
        )
        if error > 1e-7 or not renderer_matches():
            result["reasons"].append("native_preset_reconstruction_mismatch")
        else:
            result["accepted"] = True
            result["suggested_params"] = metadata["effective_params"]
    return result
