#!/usr/bin/env python3
"""Independent feature, grouped BFGS/bootstrap and review-only decision audit."""

import argparse
import hashlib
import json
import math
from pathlib import Path

import numpy as np
from scipy.fft import rfft
from scipy.optimize import minimize
from scipy.special import expit


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def extract(raw, ctx):
    x = np.frombuffer(raw, dtype="<f4").reshape(-1, ctx["channels"]).astype(float)
    rate = ctx["sample_rate"]
    n = round(rate * 0.02)
    hop = round(rate * 0.01)
    hz = np.arange(n // 2 + 1) * rate / n
    hf = hz >= ctx["frequency_hz"]
    window = np.array([0.5 - 0.5 * math.cos(2 * math.pi * i / n) for i in range(n)])
    weight = np.full(n // 2 + 1, 2.0)
    weight[0] = 1
    if n % 2 == 0:
        weight[-1] = 1
    channels = []
    for ch in range(x.shape[1]):
        values = x[:, ch] / math.sqrt(
            math.fsum(float(v) ** 2 for v in x[:, ch]) / len(x)
        )
        frames = np.array(
            [values[start : start + n] for start in range(0, len(values) - n + 1, hop)]
        )
        raw_power = abs(rfft(frames * window, axis=1)) ** 2
        power = raw_power * weight
        mass = power / np.maximum(power.sum(axis=1)[:, None], 1e-20)
        band = np.maximum(raw_power[:, hf], 1e-12)
        flat = float(np.mean(np.log(band).mean(axis=1) - np.log(band.mean(axis=1))))
        env = np.sqrt(np.mean(frames * frames, axis=1))
        flux = np.abs(np.diff(mass, axis=0)).sum(axis=1)
        channels.append(
            [
                flat,
                float(np.mean(mass[:, hf].sum(axis=1))),
                float(np.mean(mass.max(axis=1))),
                float(env.std() / max(env.mean(), 1e-12)),
                min(
                    math.log(
                        max(float(np.quantile(env, 0.95)), 1e-12)
                        / max(float(np.quantile(env, 0.05)), 1e-12)
                    ),
                    20,
                ),
                float(flux.mean()),
                float(np.quantile(flux, 0.95)),
                float(
                    np.count_nonzero(np.signbit(values[1:]) != np.signbit(values[:-1]))
                    / (len(values) - 1)
                ),
            ]
        )
    channels = np.array(channels)
    return np.r_[channels.min(axis=0), channels.max(axis=0)]


def solve(x, y, weights):
    weights = weights / weights.sum()
    center = np.sum(x * weights[:, None], axis=0)
    scale = np.maximum(
        np.sqrt(np.sum((x - center) ** 2 * weights[:, None], axis=0)), 0.05
    )
    a = np.column_stack([(x - center) / scale, np.ones(len(x))])
    ridge = np.r_[np.full(16, 0.01), 1e-6]

    def loss(theta):
        z = a @ theta
        return float(
            weights @ (np.logaddexp(0, z) - y * z) + np.dot(ridge * theta, theta) / 2
        )

    def gradient(theta):
        return a.T @ (weights * (expit(a @ theta) - y)) + ridge * theta

    result = minimize(
        loss,
        np.zeros(17),
        jac=gradient,
        method="BFGS",
        options={"gtol": 1e-10, "maxiter": 500},
    )
    assert np.linalg.norm(gradient(result.x)) < 1e-7
    return {"center": center, "scale": scale, "weights": result.x}


def bundle(x, y, g):
    names, index, count = np.unique(g, return_inverse=True, return_counts=True)
    weights = 1 / count[index]
    base = solve(x, y, weights)
    models = []
    rng = np.random.default_rng(20711026)
    for _ in range(50):
        multiplicity = np.bincount(
            rng.integers(len(names), size=len(names)), minlength=len(names)
        )
        w = weights * multiplicity[index]
        if len(set(y[w > 0])) == 2:
            models.append(solve(x, y, w))
    return {"main": base, "bootstrap": models, "noise_support": x[y == 1]}


def score(model, feature):
    def value(m):
        theta = m["weights"]
        z = float(np.dot((feature - m["center"]) / m["scale"], theta[:-1]) + theta[-1])
        return 1 / (1 + math.exp(-z))

    scores = sorted(value(m) for m in model["bootstrap"])
    position = 0.05 * (len(scores) - 1)
    lo = math.floor(position)
    hi = math.ceil(position)
    lower = scores[lo] + (scores[hi] - scores[lo]) * (position - lo)
    distance = float(
        np.min(
            np.sqrt(
                np.mean(
                    (
                        (np.asarray(model["noise_support"]) - feature)
                        / model["main"]["scale"]
                    )
                    ** 2,
                    axis=1,
                )
            )
        )
    )
    return value(model["main"]), lower, distance, lower >= 0.95 and distance <= 1.5


def baseline(f):
    return bool(
        math.exp(f[0]) > 0.35
        and f[1] > 0.1
        and f[10] < 0.03
        and f[11] < 0.15
        and f[12] < math.log(2)
    )


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--evaluation", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True)
    args = p.parse_args()
    evaluation = json.loads(args.evaluation.read_text())
    data = json.loads(Path(evaluation["training_session"]).read_text())
    report = json.loads(Path(evaluation["fit_report"]).read_text())
    raw_model = Path(evaluation["model"]).read_bytes()
    assert sha(raw_model) == evaluation["model_sha256"]
    stored = json.loads(raw_model)
    ctx = data["context"]
    x = []
    y = []
    groups = []
    cache = {}
    feature_error = 0.0
    coefficient_error = 0.0
    score_error = 0.0
    fits = 0
    for r in data["records"]:
        raw = Path(r["audio_file"]).read_bytes()
        assert sha(raw) == r["audio_sha256"]
        f = extract(raw, ctx)
        cache[r["audio_sha256"]] = f
        x.append(f)
        y.append(r["label"] == "noise_only")
        groups.append(r["source_group"])
    x = np.array(x)
    y = np.array(y, dtype=float)
    groups = np.array(groups)
    independent = bundle(x, y, groups)
    fits += 1 + len(independent["bootstrap"])
    for expected, actual in zip(
        [independent["main"], *independent["bootstrap"]],
        [stored["parameters"]["main"], *stored["parameters"]["bootstrap"]],
        strict=True,
    ):
        for key in ["center", "scale", "weights"]:
            coefficient_error = max(
                coefficient_error, float(np.max(abs(expected[key] - actual[key])))
            )
    indexed = {r["record_id"]: r for r in report["cross_validation"]}
    for group in np.unique(groups):
        held = groups == group
        model = bundle(x[~held], y[~held], groups[~held])
        fits += 1 + len(model["bootstrap"])
        for i in np.flatnonzero(held):
            expected = score(model, x[i])
            actual = indexed[data["records"][int(i)]["id"]]
            score_error = max(
                score_error,
                abs(expected[0] - actual["noise_score"]),
                abs(expected[1] - actual["bootstrap_lower_score"]),
                abs(expected[2] - actual["nearest_noise_support_distance"]),
            )
            assert (
                expected[3] == actual["review_candidate"]
                and baseline(x[i]) == actual["classical_review_candidate"]
            )
    for r in evaluation["cases"]:
        raw = Path(r["input_file"]).read_bytes()
        assert sha(raw) == r["input_sha256"]
        f = extract(raw, ctx)
        cache[r["input_sha256"]] = f
        feature_error = max(feature_error, float(np.max(abs(f - r["features"]))))
        expected = score(independent, f)
        actual = r["proposal"]
        score_error = max(
            score_error,
            abs(expected[0] - actual["noise_score"]),
            abs(expected[1] - actual["bootstrap_lower_score"]),
            abs(expected[2] - actual["nearest_noise_support_distance"]),
        )
        assert (
            expected[3] == actual["review_candidate"]
            and baseline(f) == r["classical_review_candidate"]
            and actual["automatic_capture"] is False
        )
    counter = [
        r for r in evaluation["cases"] if r["case"].startswith("counterfactual-")
    ]
    assert (
        counter[0]["input_sha256"] == counter[1]["input_sha256"]
        and counter[0]["proposal"] == counter[1]["proposal"]
        and counter[0]["label"] != counter[1]["label"]
    )
    native = evaluation["explicit_native_diagnostic"]
    folder = Path(native["metadata"]).parent
    meta = json.loads(Path(native["metadata"]).read_text())
    raw = (folder / "input.f32le").read_bytes()
    out = (folder / "output.f32le").read_bytes()
    capture = (folder / "capture.f32le").read_bytes()
    assert (
        sha(raw) == meta["input_sha256"]
        and sha(out) == meta["output_sha256"]
        and sha(capture) == meta["capture_sha256"] == counter[0]["input_sha256"]
    )
    source = np.frombuffer(raw, dtype="<f4")
    output = np.frombuffer(out, dtype="<f4")[meta["latency_samples"] :]
    start, end = native["region_frames"]
    before = math.fsum(float(v) ** 2 for v in source[start:end])
    after = math.fsum(float(v) ** 2 for v in output[start:end])
    attenuation = 10 * math.log10(before / after)
    assert abs(attenuation - native["full_band_attenuation_db"]) < 1e-10
    cli = Path(evaluation["successful_proposal_cli"])
    proposal = json.loads(cli.read_text())
    assert (
        proposal["automatic_capture"] is False
        and proposal["profile_changed"] is False
        and list(cli.parent.iterdir()) == [cli]
    )
    assert feature_error < 1e-9 and coefficient_error < 2e-6 and score_error < 2e-6
    result = {
        "training_records": len(x),
        "evaluation_records": len(evaluation["cases"]),
        "unique_audio_identities": len(cache),
        "independent_bfgs_fits": fits,
        "max_feature_error": feature_error,
        "max_model_parameter_error": coefficient_error,
        "max_score_or_support_error": score_error,
        "held_source_decisions_verified": len(indexed),
        "counterfactual_identical_score_verified": True,
        "counterfactual_review_candidate": counter[0]["proposal"]["review_candidate"],
        "explicit_native_desired_noise_effect_attenuation_db": attenuation,
        "proposal_cli_writes_only_review_json": True,
        "scope": "Source-labelled synthetic review model and one recorded development clip; no automatic capture, semantic safety or population confidence guarantee.",
    }
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
