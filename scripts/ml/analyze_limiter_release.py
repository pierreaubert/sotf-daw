#!/usr/bin/env python3
"""Recompute release-fit evidence from raw native arrays and independent Torch inference."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
import torch
from limiter_release import Model, descriptors


def independent_features(dry, reference, rate, fixed):
    width = round(rate * 0.005)
    gains, energy = [], []
    for start in range(0, len(dry) - width + 1, width):
        x = dry[start : start + width].astype(np.float64).ravel()
        y = reference[start : start + width].astype(np.float64).ravel()
        energy.append(np.dot(x, x) / len(x))
        gains.append(np.dot(x, y) / max(np.dot(x, x), 1e-20))
    energy, gains = np.array(energy), np.array(gains)
    valid = energy > 1e-8
    attenuation = np.minimum(
        80, np.maximum(0, -20 * np.log10(np.minimum(1, np.maximum(1e-4, gains))))
    )
    attenuation[~valid] = 0
    count = len(attenuation) * width
    x, y = dry[:count].astype(np.float64), reference[:count].astype(np.float64)
    features = (
        np.quantile(attenuation[valid] if valid.any() else [0], np.arange(7) / 6) / 40
    ).tolist()
    features += [
        float(valid.mean()),
        float(((attenuation > 0.2) & valid).mean()),
        float(np.log1p(np.linalg.norm(x) / np.sqrt(x.size))),
        float(np.log1p(np.linalg.norm(y) / np.sqrt(y.size))),
        float(np.log1p(np.max(np.abs(x)))),
        fixed["threshold_db"] / 20,
        fixed["lookahead_ms"] / 20,
        min(len(dry) / rate, 8) / 8,
    ]
    recovery = []
    for lag in [1, 2, 4, 10]:
        observed = []
        for index in range(len(attenuation) - lag):
            a, b = attenuation[index], attenuation[index + lag]
            if (
                valid[index]
                and valid[index + lag]
                and a > 0.3
                and b > 0.1
                and b < a - 1e-5
            ):
                decay = -np.log(np.clip(b / max(a, 1e-20), 1e-8, 1))
                observed.append(
                    float(
                        np.clip(1000 * lag * width / rate / max(decay, 1e-12), 1, 10000)
                    )
                )
        features += (
            np.quantile(np.log10(observed) if observed else [2.0], [0.1, 0.5, 0.9]) / 4
        ).tolist()
        features.append(len(observed) / (len(attenuation) - lag))
        recovery.extend(observed)
    analytic = np.clip(np.median(recovery) if recovery else 100, 10, 1000)
    features.append(np.log10(analytic) / 3)
    return np.array(features)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evaluation", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        raise ValueError("Use a new analysis path")
    report = json.loads(args.evaluation.read_text())
    model = Model(args.model, report["model_sha256"])
    weights = json.loads(args.model.read_text())
    max_features, max_prediction, max_metric = 0.0, 0.0, 0.0
    native_renders, total_samples = 0, 0
    rows, method_metrics = [], {"neural": [], "classical-grid-and-analytic": []}
    for row in report["rows"]:
        root = Path(row["work"])
        request = json.loads((root / "target.json").read_text())
        channels, rate = request["channels"], request["sample_rate"]
        for key, name in [("dry", "dry"), ("reference", "reference")]:
            if (
                hashlib.sha256((root / f"{name}.f32le").read_bytes()).hexdigest()
                != row[key + "_sha256"]
            ):
                raise ValueError("Evaluation source audio changed")
        dry = np.fromfile(root / "dry.f32le", dtype="<f4").reshape(-1, channels)
        target = np.fromfile(root / "reference.f32le", dtype="<f4").reshape(
            -1, channels
        )
        reference_features = independent_features(
            dry, target, rate, request["fixed_params"]
        )
        features = descriptors(dry, target, rate, request["fixed_params"])[0]
        max_features = max(
            max_features, float(np.max(np.abs(features - reference_features)))
        )
        tensor = torch.tensor(reference_features, dtype=torch.float64)
        for layer in weights["layers"]:
            tensor = torch.tanh(
                torch.tensor(layer["weight"], dtype=torch.float64) @ tensor
                + torch.tensor(layer["bias"], dtype=torch.float64)
            )
        independent_prediction = 10 ** (2 + tensor.item())
        max_prediction = max(
            max_prediction,
            abs(independent_prediction - model.predict_features(features)),
        )
        scale = max(
            float(np.linalg.norm(target.astype(np.float64)) / np.sqrt(target.size)),
            1e-6,
        )
        result = row["result"]
        all_renders = list((root / "fit").glob("render-*"))
        if (root / "fit/reopen").exists():
            all_renders.append(root / "fit/reopen")
        for folder in all_renders:
            req = json.loads((folder / "request.json").read_text())
            meta = json.loads((folder / "metadata.json").read_text())
            raw = np.fromfile(folder / "output.f32le", dtype="<f4")
            if (
                raw.size != meta["output_frames"] * channels
                or not np.isfinite(raw).all()
            ):
                raise ValueError("Invalid raw native output")
            if (
                np.max(np.abs(raw.astype(np.float64)))
                > 10 ** (req["params"]["threshold_db"] / 20) + 2e-6
            ):
                raise ValueError("Independent sample ceiling exceeded")
            aligned = raw.reshape(-1, channels)[
                meta["latency_samples"] : meta["latency_samples"] + len(dry)
            ]
            delta = aligned.astype(np.float64) - target
            rms = float(np.linalg.norm(delta) / np.sqrt(delta.size) / scale)
            maximum = float(np.abs(delta).max())
            measured = (
                result["reopened"]
                if folder.name == "reopen"
                else min(
                    result["objective"],
                    key=lambda item: abs(
                        item["release_ms"] - req["params"]["release_ms"]
                    ),
                )
            )
            max_metric = max(
                max_metric,
                abs(measured["relative_rms"] - rms),
                abs(measured["max_error"] - maximum),
            )
            native_renders += 1
            total_samples += raw.size
        if len(all_renders) != result["unique_native_renders"]:
            raise ValueError("Native render count mismatch")
        positive = row["kind"] != "negative-control"
        for item in result["candidates"]:
            if positive:
                method_metrics[item["method"]].append(
                    {
                        "relative_rms": item["relative_rms"],
                        "max_error": item["max_error"],
                        "attempts": item["attempted_native_evaluations"],
                        "waveform_qualified": item["relative_rms"]
                        <= report["gates"]["relative_rms"]
                        and item["max_error"] <= report["gates"]["max_absolute_error"],
                    }
                )
            if item["attempted_native_evaluations"] > 24:
                raise ValueError("Unequal search budget")
        rows.append(
            {
                "name": row["name"],
                "kind": row["kind"],
                "accepted": result["accepted"],
                "true_release_ms": row["true_release_ms"],
                "fitted_release_ms": result["best"]["release_ms"],
                "relative_release_error": abs(
                    result["best"]["release_ms"] / row["true_release_ms"] - 1
                ),
                "initial_neural_log_error": abs(
                    np.log10(independent_prediction / row["true_release_ms"])
                ),
                "analytic_log_error": abs(
                    np.log10(result["analytic_seed_ms"] / row["true_release_ms"])
                ),
                "relative_rms": result["best"]["relative_rms"],
                "reasons": result["reasons"],
            }
        )
    if max_features > 1e-10 or max_prediction > 1e-6 or max_metric > 1e-10:
        raise ValueError("Independent reconstruction mismatch")
    positive = [r for r in rows if r["kind"] != "negative-control"]
    negatives = [r for r in rows if r["kind"] == "negative-control"]
    summary = {
        "positive_admitted": sum(r["accepted"] for r in positive),
        "positive_total": len(positive),
        "negative_admitted": sum(r["accepted"] for r in negatives),
        "negative_total": len(negatives),
        "max_feature_error": max_features,
        "max_torch_prediction_error_ms": max_prediction,
        "max_raw_metric_error": max_metric,
        "native_renders_independently_checked": native_renders,
        "native_samples_independently_checked": total_samples,
        "methods": {
            name: {
                "waveform_qualified": sum(r["waveform_qualified"] for r in metrics),
                "total": len(metrics),
                "median_attempts": float(np.median([r["attempts"] for r in metrics])),
            }
            for name, metrics in method_metrics.items()
        },
        "median_positive_initial_neural_log_error": float(
            np.median([r["initial_neural_log_error"] for r in positive])
        ),
        "median_positive_analytic_log_error": float(
            np.median([r["analytic_log_error"] for r in positive])
        ),
        "rows": rows,
        "qualification": "Synthetic fixed-profile native evidence only. No claim of general ML benefit, hardware emulation, listener preference or real-time inference.",
    }
    args.output.write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps({k: v for k, v in summary.items() if k != "rows"}), flush=True)


if __name__ == "__main__":
    main()
