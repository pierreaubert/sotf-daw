"""Independently reconstruct retained scene features, source fits, decisions and native gates."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from scipy.linalg import cho_factor, cho_solve
from scipy.signal import get_window, stft
from scipy.special import logsumexp


def independent_features(x, ctx):
    rate = ctx["sample_rate"]
    x = x.astype(float)
    if np.sqrt(np.mean(x * x)) < 1e-5:
        raise ValueError("unexcited")
    n, hop = round(rate / 50), round(rate / 100)
    window = get_window("hann", n, fftbins=True)
    hz, _, spectrum = stft(
        x,
        fs=rate,
        window=window,
        nperseg=n,
        noverlap=n - hop,
        boundary=None,
        padded=False,
        detrend=False,
        axis=0,
    )
    spectrum = spectrum.transpose(2, 1, 0)
    starts = np.arange(0, len(x) - n + 1, hop)
    if starts[-1] != len(x) - n:
        final = np.fft.rfft(x[-n:] * window[:, None], axis=0).T / window.sum()
        spectrum = np.concatenate([spectrum, final[None]])
        starts = np.r_[starts, len(x) - n]
    weights = np.full(len(hz), 2.0)
    weights[0] = 1
    if n % 2 == 0:
        weights[-1] = 1
    l, r = spectrum[:, 0], spectrum[:, 1]
    left, right = (l * np.conj(l)).real * weights, (r * np.conj(r)).real * weights
    cross = (l * np.conj(r)).real * weights
    all_mass = np.sum(left + right)
    values, energy = [], []
    for mask in [
        np.ones(len(hz), dtype=bool),
        hz < 250,
        (hz >= 250) & (hz < 4000),
        hz >= 4000,
    ]:
        a, b, c = left[:, mask].sum(), right[:, mask].sum(), cross[:, mask].sum()
        energy.append((a + b) / all_mass)
        if a + b <= all_mass * 1e-12:
            values.extend([0, 0, 0])
        else:
            values.extend(
                [
                    0.5 - c / (a + b),
                    float(np.clip(c / max(np.sqrt(a * b), 1e-20), -1, 1)),
                    (a - b) / (a + b),
                ]
            )
    power = np.mean(x * x, axis=1)
    cumulative = np.r_[0, np.cumsum(power)]
    envelope = np.sqrt(np.maximum((cumulative[starts + n] - cumulative[starts]) / n, 0))
    return np.r_[values, energy[1:], envelope.std() / envelope.mean()]


def independent_fit(x, labels, weights, classes):
    center = np.average(x, axis=0, weights=weights)
    scale = np.maximum(
        np.sqrt(np.average((x - center) ** 2, axis=0, weights=weights)), 0.05
    )
    z = (x - center) / scale
    means = np.array(
        [
            np.average(z[labels == c], axis=0, weights=weights[labels == c])
            for c in range(classes)
        ]
    )
    residual = (z - means[labels]) * np.sqrt(weights / weights.sum())[:, None]
    covariance = residual.T @ residual * 0.8 + np.eye(16) * 0.2
    return {
        "center": center,
        "scale": scale,
        "means": means,
        "precision": cho_solve(cho_factor(covariance, lower=True), np.eye(16)),
    }


def independent_bundle(x, labels, groups, classes):
    names, inverse, counts = np.unique(groups, return_inverse=True, return_counts=True)
    weights = 1 / counts[inverse]
    rng = np.random.default_rng(20871026)
    ensemble = []
    for _ in range(50):
        multiplicity = np.bincount(
            rng.integers(len(names), size=len(names)), minlength=len(names)
        )
        w = weights * multiplicity[inverse]
        selected = w > 0
        if len(set(labels[selected])) == classes:
            ensemble.append(
                independent_fit(x[selected], labels[selected], w[selected], classes)
            )
    return {
        "main": independent_fit(x, labels, weights, classes),
        "bootstrap": ensemble,
        "support": x,
        "labels": labels,
    }


def independent_score(bundle, feature):
    def probabilities(m):
        z = (feature - m["center"]) / m["scale"]
        # Linear discriminant form cancels the class-common quadratic term.
        logits = m["means"] @ m["precision"] @ z - 0.5 * np.diag(
            m["means"] @ m["precision"] @ m["means"].T
        )
        return np.exp(logits - logsumexp(logits))

    scores = probabilities(bundle["main"])
    label = int(np.argmax(scores))
    lower = float(
        np.quantile([probabilities(m)[label] for m in bundle["bootstrap"]], 0.05)
    )
    support = bundle["support"][bundle["labels"] == label]
    distance = np.min(
        np.linalg.norm((support - feature) / bundle["main"]["scale"], axis=1) / 4
    )
    return label, scores, lower, float(distance)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        raise ValueError("Refuse analysis overwrite")
    report = json.loads(args.report.read_text())
    modelraw = Path(report["model_file"]).read_bytes()
    if hashlib.sha256(modelraw).hexdigest() != report["model_sha256"]:
        raise ValueError("Model identity mismatch")
    model = json.loads(modelraw)
    data = json.loads(Path(report["training_session"]).read_text())
    fitted = json.loads(Path(report["fit_report"]).read_text())
    classes = model["class_names"]
    identities = set()

    def read(record):
        raw = Path(record["audio_file"]).read_bytes()
        identity = hashlib.sha256(raw).hexdigest()
        if identity != record["audio_sha256"]:
            raise ValueError("PCM identity mismatch")
        identities.add(identity)
        return np.frombuffer(raw, dtype="<f4").reshape(-1, 2)

    x = np.array(
        [independent_features(read(r), data["context"]) for r in data["records"]]
    )
    labels = np.array([classes.index(r["scene"]) for r in data["records"]])
    groups = np.array([r["source_group"] for r in data["records"]])
    train_hashes = set(identities)
    full = independent_bundle(x, labels, groups, len(classes))
    error = 0.0
    for a, b in zip(
        [full["main"], *full["bootstrap"]],
        [model["parameters"]["main"], *model["parameters"]["bootstrap"]],
        strict=True,
    ):
        for key in ["center", "scale", "means", "precision"]:
            error = max(error, float(np.max(abs(a[key] - np.asarray(b[key])))))
    feature_error = float(np.max(abs(x - np.asarray(model["parameters"]["support"]))))
    score_error, fits = 0.0, 1 + len(full["bootstrap"])
    decisions = 0
    baseline_decisions = 0
    by_id = {r["record_id"]: r for r in fitted["cross_validation"]}
    for group in np.unique(groups):
        selected = groups != group
        bundle = independent_bundle(
            x[selected], labels[selected], groups[selected], len(classes)
        )
        fits += 1 + len(bundle["bootstrap"])
        for i in np.flatnonzero(~selected):
            value = independent_score(bundle, x[i])
            expected = by_id[data["records"][int(i)]["id"]]
            centers = (
                bundle["main"]["means"][:, 0] * bundle["main"]["scale"][0]
                + bundle["main"]["center"][0]
            )
            assert int(np.argmin(abs(centers - x[i, 0]))) == expected["baseline_class"]
            baseline_decisions += 1
            assert value[0] == expected["class_index"]
            assert (value[2] >= 0.95 and value[3] <= 1.5) == expected["confident"]
            score_error = max(
                score_error,
                float(np.max(abs(value[1] - np.asarray(expected["scene_scores"])))),
                abs(value[2] - expected["bootstrap_lower_score"]),
                abs(value[3] - expected["support_distance"]),
            )
            decisions += 1
    native_error, accepted, classified, baseline_correct, known = 0.0, 0, 0, 0, 0
    eval_hashes = set()
    side_loss = None
    for row in report["cases"]:
        pcm = read(row)
        eval_hashes.add(row["audio_sha256"])
        result = row["proposal"]
        if row["id"] == "silent":
            assert not result["accepted"]
            continue
        feature = independent_features(pcm, row["context"])
        feature_error = max(
            feature_error, float(np.max(abs(feature - np.asarray(result["features"]))))
        )
        value = independent_score(full, feature)
        assert value[0] == result["class_index"]
        confidence = value[2] >= 0.95 and value[3] <= 1.5
        assert confidence == result["confident"]
        score_error = max(
            score_error,
            float(np.max(abs(value[1] - np.asarray(result["scene_scores"])))),
            abs(value[2] - result["bootstrap_lower_score"]),
            abs(value[3] - result["support_distance"]),
        )
        if row["label"] is not None:
            centers = (
                full["main"]["means"][:, 0] * full["main"]["scale"][0]
                + full["main"]["center"][0]
            )
            assert (
                classes[int(np.argmin(abs(centers - feature[0])))]
                == row["baseline_scene"]
            )
            baseline_decisions += 1
            known += 1
            classified += confidence and classes[value[0]] == row["label"]
            baseline_correct += row["baseline_scene"] == row["label"]
        valid = confidence
        if "native_metadata" in result:
            path = Path(row["audio_file"]).with_suffix("")
            outbytes = (path / "output.f32le").read_bytes()
            assert (
                hashlib.sha256(outbytes).hexdigest()
                == result["native_metadata"]["output_sha256"]
            )
            y = np.frombuffer(outbytes, dtype="<f4").reshape(-1, 2).astype(float)
            padded = np.pad(pcm.astype(float), ((0, 4096), (0, 0)))
            input_rms = np.linalg.norm(padded) / np.sqrt(padded.size)
            output_rms = np.linalg.norm(y) / np.sqrt(y.size)
            mono = padded.mean(axis=1)
            metrics = {
                "input_sample_peak": float(np.max(abs(pcm))),
                "output_sample_peak": float(np.max(abs(y))),
                "rms_gain_db": float(
                    20 * np.log10(max(output_rms, 1e-20) / max(input_rms, 1e-20))
                ),
                "mono_relative_rms_error": float(
                    np.linalg.norm(y.mean(axis=1) - mono)
                    / np.sqrt(len(mono))
                    / max(input_rms, 1e-6)
                ),
                "input_mono_energy_fraction": float(
                    np.mean(mono**2) / max(input_rms**2, 1e-20)
                ),
            }
            native_error = max(
                native_error,
                max(abs(metrics[k] - result["native_metrics"][k]) for k in metrics),
            )
            valid &= (
                max(metrics["input_sample_peak"], metrics["output_sample_peak"])
                <= 0.999
                and -6 <= metrics["rms_gain_db"] <= 6
                and metrics["mono_relative_rms_error"] <= 1e-6
                and metrics["input_mono_energy_fraction"] >= 0.01
            )
            if valid:
                reopened = (
                    path.with_name(path.name + "-reopen") / "output.f32le"
                ).read_bytes()
                assert reopened == outbytes and result["native_reopen_error"] == 0
            if row["id"] == "desired_wide_effect":
                side_loss = float(
                    20
                    * np.log10(
                        np.linalg.norm((y[:, 0] - y[:, 1]) / 2)
                        / np.linalg.norm((padded[:, 0] - padded[:, 1]) / 2)
                    )
                )
        assert valid == result["accepted"], row["id"]
        accepted += valid
    assert not train_hashes & eval_hashes
    same = [
        r
        for r in report["cases"]
        if r["id"] in {"desired_wide_effect", "unwanted_side_spill"}
    ]
    assert same[0]["audio_sha256"] == same[1]["audio_sha256"]
    assert (
        same[0]["proposal"]["suggested_params"]
        == same[1]["proposal"]["suggested_params"]
    )
    assert (
        feature_error <= 1e-10
        and error <= 1e-8
        and score_error <= 1e-8
        and native_error <= 1e-10
    )
    summary = {
        "training_records": len(x),
        "evaluation_records": len(report["cases"]),
        "unique_audio_identities": len(identities),
        "source_grouped_closed_form_fits": fits,
        "cv_decisions_reconstructed": decisions,
        "baseline_decisions_reconstructed": baseline_decisions,
        "maximum_feature_error": feature_error,
        "maximum_parameter_error": error,
        "maximum_score_support_error": score_error,
        "maximum_native_metric_error": native_error,
        "accepted_native_proposals": accepted,
        "known_label_conditions": known,
        "confident_correct_conditions": classified,
        "one_feature_baseline_correct": baseline_correct,
        "desired_wide_effect_side_gain_db": side_loss,
        "qualification": "Procedural functional evidence only. No baseline superiority or preferred-width/recorded-scene acceptance; identical PCM with differing intended uses gets identical settings.",
    }
    args.output.write_text(json.dumps(summary, indent=2, allow_nan=False) + "\n")
    print(json.dumps(summary))


if __name__ == "__main__":
    main()
