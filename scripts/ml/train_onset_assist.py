"""Freeze a classifier from actual native causal features before held-source probes."""

import argparse
import hashlib
import json
import shutil
from pathlib import Path

import numpy as np
from onset_assist_cases import fixture
from onset_assist_model import export, load, objective, predict
from onset_assist_native import render
from scipy.optimize import minimize
from scipy.stats import rankdata


def collect(
    renderer, root, first, groups, kind="matched", rate=48000, model=None, sha=None
):
    records = []
    all_x = []
    all_y = []
    for i in range(groups):
        signal, source = fixture(first + i, rate, kind)
        output, data = render(
            signal, renderer, root / str(i), rate=rate, model=model, sha=sha
        )
        np.testing.assert_array_equal(output[:, 0], signal)
        if model is not None and not data["model_loaded"]:
            raise RuntimeError("Frozen model not loaded by native classifier")
        rows = data["rows"]
        x = np.array(
            [
                row["state"]["features"]
                if row["state"]["features"] is not None
                else [np.nan] * 6
                for row in rows
            ]
        )
        valid = np.isfinite(x).all(axis=1)
        if np.any((x[valid] < 0) | (x[valid] > 1)):
            raise RuntimeError("Native normalized feature domain mismatch")
        ends = np.array([row["end_frame"] for row in rows])
        hop = data["hop_frames"]
        y = np.array(
            [
                any(
                    end - hop <= event < end for event in source["desired_event_frames"]
                )
                for end in ends
            ],
            float,
        )
        native_p = np.array(
            [
                row["state"]["probability"]
                if row["state"]["probability"] is not None
                else np.nan
                for row in rows
            ]
        )
        record = dict(
            source,
            hop_frames=hop,
            feature_rows=len(rows),
            valid_feature_rows=int(valid.sum()),
            positive_windows=int(y.sum()),
            features_sha256=hashlib.sha256(x.astype("<f8").tobytes()).hexdigest(),
            target_sha256=hashlib.sha256(y.astype("<f8").tobytes()).hexdigest(),
            native_features_file=data["files"]["features_output"],
        )
        records.append(
            {
                "metadata": record,
                "x": x,
                "y": y,
                "valid": valid,
                "ends": ends,
                "native_p": native_p,
            }
        )
        all_x.append(x[valid])
        all_y.append(y[valid])
    return np.vstack(all_x), np.concatenate(all_y), records


def metrics(theta, x, y, records, require_native=False):
    p = predict(theta, x)
    target = y.astype(bool)
    selected = p.astype(np.float32) >= np.float32(0.9)
    tp = int(np.sum(selected & target))
    fp = int(np.sum(selected & ~target))
    positive = int(target.sum())
    auc = None
    if 0 < positive < len(y):
        auc = float(
            (rankdata(p)[target].sum() - positive * (positive + 1) / 2)
            / (positive * (len(y) - positive))
        )
    ece = 0.0
    for a, b in zip(np.linspace(0, 1, 11)[:-1], np.linspace(0, 1, 11)[1:], strict=True):
        mask = (p >= a) & ((p <= b) if b == 1 else (p < b))
        if mask.any():
            ece += mask.mean() * abs(p[mask].mean() - y[mask].mean())
    hit = 0
    events = 0
    false_windows = 0
    classical_hit = 0
    classical_false = 0
    seconds = 0
    delays = []
    parity = 0.0
    for record in records:
        valid = record["valid"]
        values = np.full(len(valid), np.nan)
        values[valid] = predict(theta, record["x"][valid])
        if require_native and valid.any():
            if not np.isfinite(record["native_p"][valid]).all():
                raise RuntimeError("Native valid-feature prediction missing")
            parity = max(
                parity, float(np.max(abs(values[valid] - record["native_p"][valid])))
            )
            values = record["native_p"]
        decisions = values.astype(np.float32) >= np.float32(0.9)
        baseline = np.zeros(len(valid), bool)
        baseline[valid] = (
            (record["x"][valid, 3] >= 0.5)
            & (record["x"][valid, 2] >= 0.65)
            & (record["x"][valid, 0] > 0.2)
        )
        meta = record["metadata"]
        rate = meta["sample_rate"]
        wanted = meta["desired_event_frames"]
        # A completed exclusive window end N updates the current sample N-1.
        ends = record["ends"] - 1
        for event in wanted:
            matches = ends[
                decisions
                & ((ends - event) >= 0)
                & ((ends - event) <= round(0.006 * rate))
            ]
            if len(matches):
                hit += 1
                delays.append(float((matches[0] - event) * 1000 / rate))
            classical_hit += int(
                np.any(
                    baseline
                    & ((ends - event) >= 0)
                    & ((ends - event) <= round(0.006 * rate))
                )
            )
        windows = np.array(
            [
                any(0 <= end - event <= round(0.006 * rate) for event in wanted)
                for end in ends
            ]
        )
        false_windows += int(np.sum(decisions & ~windows))
        classical_false += int(np.sum(baseline & ~windows))
        events += len(wanted)
        seconds += meta["frames"] / rate
    if parity > 2e-7:
        raise RuntimeError("Independent Python/native network parity failed")
    return {
        "valid_rows": len(y),
        "positive_windows": positive,
        "brier_score": float(np.mean((p - y) ** 2)) if len(y) else None,
        "auroc": auc,
        "ece_10_bins": float(ece),
        "precision_at_native_0_9": tp / (tp + fp) if tp + fp else None,
        "recall_at_native_0_9": tp / positive if positive else None,
        "events": events,
        "events_detected_within_6ms": hit,
        "event_recall_within_6ms": hit / events if events else None,
        "detection_delay_ms_median": float(np.median(delays)) if delays else None,
        "false_positive_analysis_windows_per_minute": false_windows * 60 / seconds,
        "fixed_feature_rule_event_recall_within_6ms": classical_hit / events
        if events
        else None,
        "fixed_feature_rule_false_positive_windows_per_minute": classical_false
        * 60
        / seconds,
        "maximum_python_native_probability_error": parity if require_native else None,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    renderer = args.output / "renderer.frozen"
    shutil.copy2(args.renderer, renderer)
    binary_hash = hashlib.sha256(renderer.read_bytes()).hexdigest()
    x, y, training = collect(renderer, args.output / "training", 20291120, 32)
    vx, vy, validation = collect(renderer, args.output / "validation", 20292120, 8)
    fit = minimize(
        objective,
        np.random.default_rng(20291119).normal(0, 0.2, 65),
        args=(x, y),
        jac=True,
        method="L-BFGS-B",
        bounds=[(-16, 16)] * 65,
        options={"maxiter": 300, "ftol": 1e-10, "gtol": 1e-6},
    )
    model = args.output / "model.json"
    model.write_text(json.dumps(export(fit.x), indent=2, allow_nan=False) + "\n")
    sha = hashlib.sha256(model.read_bytes()).hexdigest()
    theta = load(model, sha)
    report = {
        "model_sha256": sha,
        "frozen_renderer_sha256": binary_hash,
        "optimizer_success": bool(fit.success),
        "optimizer_message": str(fit.message),
        "iterations": int(fit.nit),
        "training_groups": [r["metadata"] for r in training],
        "validation_groups": [r["metadata"] for r in validation],
        "training": metrics(theta, x, y, training),
        "validation": metrics(theta, vx, vy, validation),
        "held": {},
        "deployment_admitted": False,
        "scope": "Generated mono desired-onset window labels; nuisance bursts are intentionally unlabelled. Class-balanced BCE training, natural-density evaluation. Model frozen before final groups; shifts reuse final source seeds and are correlated. Fixed feature rule is a detection comparator, not native shaped-audio quality. No calibrated confidence, recorded/perceptual or production admission.",
    }
    variants = [
        ("matched", 48000),
        ("matched", 44100),
        ("matched", 96000),
        ("slow_attack", 48000),
        ("noise_bursts", 48000),
        ("steady_only", 48000),
        ("level_shift", 48000),
        ("dense_events", 48000),
        ("phase_late", 48000),
        ("silence", 48000),
    ]
    for kind, rate in variants:
        hx, hy, groups = collect(
            renderer,
            args.output / f"held-{kind}-{rate}",
            20293120,
            16,
            kind,
            rate,
            model,
            sha,
        )
        report["held"][f"{kind}-{rate}"] = {
            "metrics": metrics(theta, hx, hy, groups, True),
            "groups": [r["metadata"] for r in groups],
        }
    if (
        hashlib.sha256(model.read_bytes()).hexdigest() != sha
        or hashlib.sha256(renderer.read_bytes()).hexdigest() != binary_hash
    ):
        raise RuntimeError("Frozen training/evaluation inputs changed")
    (args.output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    print(
        json.dumps(
            {
                "model_sha256": sha,
                "optimizer_success": bool(fit.success),
                "training": report["training"],
                "validation": report["validation"],
                "held": {k: v["metrics"] for k, v in report["held"].items()},
                "deployment_admitted": False,
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
