"""Offline native Analog EQ preset fitting for explicitly calibrated hardware pairs."""

import argparse
import copy
import hashlib
import json
from pathlib import Path

import numpy as np
from analog_eq_fit import optimize, space
from analog_eq_native import render
from analog_hardware_capture import load_manifest, read_corpus
from fit_binaural_preferences import bounded_file, hash_value

FORMAT = "sotf-analog-eq-hardware-fit-v1"
POLICY = {
    "minimum_train_groups": 8,
    "minimum_validation_groups": 2,
    "minimum_test_groups": 2,
    "maximum_relative_rms": 0.01,
    "maximum_abs_error": 0.01,
    "maximum_sample_peak": 0.999,
    "reopen_tolerance": 1e-7,
}


def prepare(spec, renderer, budget):
    if (
        not isinstance(spec, dict)
        or spec.get("format") != FORMAT
        or spec["start_state"] != "quiescent"
    ):
        raise ValueError(
            "Declare calibrated paired hardware and quiescent native/capture start"
        )
    space(spec)
    hash_value(spec["renderer_sha256"])
    bounded_file(renderer, 128 * 1024 * 1024, spec["renderer_sha256"])
    raw = bounded_file(
        spec["capture_manifest"],
        1024 * 1024,
        hash_value(spec["capture_manifest_sha256"]),
    )
    data, identity = load_manifest(spec["capture_manifest"])
    if hashlib.sha256(raw).hexdigest() != identity:
        raise ValueError("Capture manifest changed during preparation")
    for split in ["train", "validation", "test"]:
        groups = {r["source_group"] for r in data["captures"] if r["split"] == split}
        if len(groups) < POLICY["minimum_" + split + "_groups"]:
            raise ValueError("Insufficient whole-source groups for " + split)
    train = copy.deepcopy(data)
    train["captures"] = [r for r in data["captures"] if r["split"] == "train"]
    if sum(r["frames"] for r in train["captures"]) * budget > 128_000_000:
        raise ValueError("Search exceeds128M native source-sample budget")
    signals, summary = read_corpus(train)
    return data, signals, summary


def metrics(y, target):
    y, target = np.asarray(y, dtype=float), np.asarray(target, dtype=float)
    if (
        y.shape != target.shape
        or y.ndim != 1
        or not len(y)
        or not np.isfinite(y).all()
        or not np.isfinite(target).all()
    ):
        raise ValueError("Invalid paired waveform geometry or finite values")
    error = y - target
    return {
        "relative_rms": float(
            np.sqrt(np.mean(error**2) / max(float(np.mean(target**2)), 1e-20))
        ),
        "max_abs": float(np.max(abs(error))),
        "sample_peak": float(np.max(abs(y))),
    }


def run(spec, renderer, output, budget=128, method="surrogate"):
    if (
        type(budget) is not int
        or not 40 <= budget <= 256
        or method not in {"surrogate", "classical"}
    ):
        raise ValueError("Use40..256 objective budget and a declared method")
    data, train_signals, summary = prepare(spec, renderer, budget)
    output = Path(output)
    output.mkdir(parents=True, exist_ok=False)
    rate = data["context"]["sample_rate"]
    training = [r for r in data["captures"] if r["split"] == "train"]
    count = 0

    def objective(parameters):
        nonlocal count
        grouped = {}
        for i, record in enumerate(training):
            dry, wet = train_signals[record["id"]]
            y, _ = render(
                dry[:, None],
                renderer,
                output / "search" / f"{count}-{i}",
                parameters,
                rate=rate,
            )
            loss = metrics(y[:, 0], wet)["relative_rms"] ** 2
            grouped.setdefault(record["source_group"], []).append(loss)
        count += 1
        return float(np.mean([np.mean(v) for v in grouped.values()]))

    search = optimize(objective, spec, budget, method)
    selected = search["best"]["parameters"]
    frozen = {"plugin_type": "analog_eq", "parameters": selected}
    candidate = output / "candidate.json"
    candidate.write_text(json.dumps(frozen, indent=2, allow_nan=False) + "\n")
    frozen_sha = hashlib.sha256(candidate.read_bytes()).hexdigest()
    # Freeze before validation/test target PCM is read. Neither split retunes parameters.
    rows = []
    reasons = []
    for split in ["validation", "test"]:
        subset = copy.deepcopy(data)
        subset["captures"] = [r for r in data["captures"] if r["split"] == split]
        signals, _ = read_corpus(subset)
        for i, record in enumerate(subset["captures"]):
            dry, wet = signals[record["id"]]
            y, m = render(
                dry[:, None],
                renderer,
                output / f"{split}-{i}",
                selected,
                rate=rate,
                zeros=4096,
            )
            effective = m["effective_params"]
            current = metrics(y[: len(dry), 0], wet)
            baseline, _ = render(
                dry[:, None],
                renderer,
                output / f"{split}-{i}-template",
                spec["template"],
                rate=rate,
            )
            original = metrics(baseline[:, 0], wet)
            reopened, _ = render(
                dry[:, None],
                renderer,
                output / f"{split}-{i}-reopen",
                json.loads(json.dumps(effective)),
                rate=rate,
                callback=17,
                zeros=4096,
            )
            reopen_error = float(np.max(abs(y.astype(float) - reopened)))
            observation_peak = float(np.max(abs(y)))
            if (
                current["relative_rms"] > POLICY["maximum_relative_rms"]
                or current["max_abs"] > POLICY["maximum_abs_error"]
            ):
                reasons.append(split + "_waveform_mismatch")
            if observation_peak > POLICY["maximum_sample_peak"]:
                reasons.append(split + "_sample_headroom")
            if reopen_error > POLICY["reopen_tolerance"]:
                reasons.append("native_json_reopen_mismatch")
            if effective != selected:
                raise ValueError(
                    "Native effective controls differ from selected candidate"
                )
            rows.append(
                {
                    "id": record["id"],
                    "source_group": record["source_group"],
                    "split": split,
                    "metrics": current,
                    "template_baseline": original,
                    "reopen_error": reopen_error,
                    "observation_peak": observation_peak,
                    "input_sha256": m["input_sha256"],
                    "output_sha256": m["output_sha256"],
                }
            )
    bounded_file(renderer, 128 * 1024 * 1024, spec["renderer_sha256"])
    if hashlib.sha256(candidate.read_bytes()).hexdigest() != frozen_sha:
        raise ValueError("Frozen candidate changed during held scoring")
    equivalents = [
        r for r in search["trace"] if r["loss"] <= search["best"]["loss"] + 1e-8
    ]
    report = {
        "accepted": not reasons,
        "parameters": selected if not reasons else None,
        "reasons": sorted(set(reasons)),
        "data_origin": data["context"]["data_origin"],
        "context": data["context"],
        "policy": POLICY,
        "renderer_sha256": spec["renderer_sha256"],
        "capture_manifest_sha256": spec["capture_manifest_sha256"],
        "candidate_sha256": frozen_sha,
        "search": search,
        "qualification": rows,
        "training_summary": summary,
        "native_objective_calls": count,
        "equivalent_observed_candidates": len(equivalents),
        "parameter_recovery_claim": False,
        "hardware_model_admitted": False,
        "automatic_apply": False,
        "scope": "Offline native preset matching within the declared search box and tested calibrated source/level/context. GP uncertainty is an optimization heuristic. No physical knob identity, learned runtime, general hardware fidelity or admission claim.",
    }
    if report["accepted"]:
        (output / "preset.json").write_bytes(candidate.read_bytes())
    if method == "surrogate":
        (output / "surrogate.json").write_text(
            json.dumps(
                {
                    "format": "sotf-analog-eq-gp-search-v1",
                    "context": data["context"],
                    "free": spec["free"],
                    "models": spec["models"],
                    "policy": search["policy"],
                    "observations": search["trace"],
                    "renderer_sha256": spec["renderer_sha256"],
                    "capture_manifest_sha256": spec["capture_manifest_sha256"],
                    "runtime_inference": False,
                },
                indent=2,
                allow_nan=False,
            )
            + "\n"
        )
    (output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--spec", type=Path, required=True)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--budget", type=int, default=128)
    parser.add_argument(
        "--method", choices=["surrogate", "classical"], default="surrogate"
    )
    args = parser.parse_args()
    with args.spec.open("rb") as source:
        raw = source.read(1024 * 1024 + 1)
    if len(raw) > 1024 * 1024:
        raise ValueError("Fit spec exceeds1MiB")
    report = run(json.loads(raw), args.renderer, args.output, args.budget, args.method)
    print(
        json.dumps(
            {
                "accepted": report["accepted"],
                "reasons": report["reasons"],
                "automatic_apply": False,
                "hardware_model_admitted": False,
            }
        )
    )
    if not report["accepted"]:
        raise SystemExit(2)


if __name__ == "__main__":
    main()
