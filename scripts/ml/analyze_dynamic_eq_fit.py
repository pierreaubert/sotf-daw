#!/usr/bin/env python3
"""Independent raw-native objective, gate and surrogate acquisition accounting."""

import argparse
import hashlib
import json
import math
from pathlib import Path

import numpy as np
from scipy.spatial.distance import cdist
from scipy.special import ndtr
from scipy.stats import qmc


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def metric(x, y, target):
    error = y.astype(float) - target
    numerator = math.fsum(float(v) * float(v) for v in error.flat) / error.size
    denominator = math.fsum(float(v) * float(v) for v in x.flat) / x.size
    return {
        "relative_rms": math.sqrt(numerator / denominator),
        "max_abs": float(np.max(abs(error))),
    }


def kernel(a, b):
    z = np.sqrt(5) * cdist(a, b) / 0.25
    return (1 + z + z * z / 3) * np.exp(-z)


def acquisition(x, y, query):
    logs = np.log10(np.maximum(y, 1e-12))
    mean = logs.mean()
    scale = max(float(logs.std()), 0.1)
    k = kernel(x, x) + np.eye(len(x)) * 1e-6
    cross = kernel(x, query)
    mu = mean + cross.T @ np.linalg.solve(k, logs - mean)
    std = scale * np.sqrt(
        np.maximum(1 - np.einsum("ij,ij->j", cross, np.linalg.solve(k, cross)), 0)
    )
    improvement = logs.min() - mu
    z = improvement / np.maximum(std, 1e-12)
    return improvement * ndtr(z) + std * np.exp(-z * z / 2) / np.sqrt(2 * np.pi)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evaluation", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    evaluation = json.loads(args.evaluation.read_text())
    max_loss_error = max_metric_error = max_reopen = 0.0
    renders = queries = 0
    comparisons = []
    for entry in evaluation["cases"]:
        data = json.loads(Path(entry["target"]).read_text())
        folder = Path(entry["report"]).parent
        report = json.loads(Path(entry["report"]).read_text())
        clips = []
        for c in data["clips"]:
            raw = Path(c["input_file"]).read_bytes()
            ref = Path(c["reference_file"]).read_bytes()
            assert sha(raw) == c["input_sha256"] and sha(ref) == c["reference_sha256"]
            clips.append(
                (
                    c,
                    np.frombuffer(raw, dtype="<f4").reshape(-1, data["channels"]),
                    np.frombuffer(ref, dtype="<f4").reshape(-1, data["channels"]),
                )
            )
        train = [r for r in clips if r[0]["role"] == "train"]

        def native(path, source, channels=data["channels"]):
            nonlocal renders
            m = json.loads((path / "metadata.json").read_text())
            raw = (path / "output.f32le").read_bytes()
            input_raw = (path / "input.f32le").read_bytes()
            assert (
                sha(raw) == m["output_sha256"]
                and sha(input_raw) == m["input_sha256"] == source["input_sha256"]
            )
            assert m["latency_samples"] == 0 and m["appended_zero_frames"] == 0
            renders += 1
            return np.frombuffer(raw, dtype="<f4").reshape(-1, channels), m

        rows = report["search"]["trace"] + report["sensitivity_probes"]
        losses = []
        for i, row in enumerate(rows):
            errors = []
            for j, (c, x, reference) in enumerate(train):
                y, m = native(folder / "search" / f"evaluation-{i}-clip-{j}", c)
                p = m["requested_params"]
                for k, value in row["controls"].items():
                    assert p[k] == value
                for band in p["bands"]:
                    assert (
                        band["band_threshold"] == p["threshold"]
                        and band["band_ratio"] == p["ratio"]
                    )
                errors.append(metric(x, y, reference)["relative_rms"] ** 2)
            loss = math.fsum(errors) / len(errors)
            losses.append(loss)
            max_loss_error = max(max_loss_error, abs(loss - row["loss"]))
        assert len(report["search"]["trace"]) <= report["search"]["budget"]
        best = report["search"]["best"]
        best_x = np.array(best["x"])
        best_loss = min(losses[: len(report["search"]["trace"])])
        assert abs(best_loss - best["loss"]) < 1e-12
        reasons = set()
        for i, ((c, x, reference), stored) in enumerate(
            zip(clips, report["qualification"], strict=True)
        ):
            y, m = native(folder / "qualification" / str(i), c)
            check = metric(x, y, reference)
            baseline = metric(x, x, reference)
            for key, value in check.items():
                max_metric_error = max(
                    max_metric_error, abs(value - stored["metrics"][key])
                )
            if check["relative_rms"] > 0.002 or check["max_abs"] > 0.01:
                reasons.add(c["role"] + "_waveform_mismatch")
            if (
                baseline["relative_rms"] - check["relative_rms"] < 0.001
                or check["relative_rms"] > 0.5 * baseline["relative_rms"]
            ):
                reasons.add("no_material_advantage_over_dry")
            reopened, _ = native(folder / "reopen" / str(i), c)
            error = float(np.max(abs(y.astype(float) - reopened)))
            max_reopen = max(max_reopen, error)
            if error > 1e-7:
                reasons.add("preset_reopening_mismatch")
        for row, loss in zip(rows, losses, strict=True):
            if (
                np.max(abs(np.array(row["x"]) - best_x)) >= 0.099
                and math.sqrt(loss) <= math.sqrt(best_loss) + 0.0005
            ):
                reasons.add("observed_parameter_ambiguity")
        assert sorted(reasons) == report["reasons"]
        assert report["accepted"] == (not reasons)
        assert (folder / "preset.json").exists() == report["accepted"]
        assert report["native_render_calls"] == len(rows) * len(train) + 2 * len(clips)
        if entry["method"] == "surrogate":
            trace = report["search"]["trace"]
            policy = report["search"]["policy"]
            rng = np.random.default_rng(policy["seed"])
            pool = qmc.Sobol(4, scramble=True, seed=policy["seed"]).random_base2(10)
            for n in range(8, policy["global_proposals"]):
                x = np.array([r["x"] for r in trace[:n]])
                observations = np.array(losses[:n])
                best_point = x[int(np.argmin(observations))]
                query = np.vstack(
                    [
                        pool[32:288],
                        np.clip(best_point + rng.normal(size=(256, 4)) * 0.12, 0, 1),
                    ]
                )

                def key(point):
                    low = np.array([-60.0, 0.0, np.log(0.1), np.log(10.0)])
                    high = np.array([0.0, np.log(20.0), np.log(100.0), np.log(1000.0)])
                    physical = low + point * (high - low)
                    return tuple(
                        np.asarray(
                            [physical[0], *np.exp(physical[1:])], dtype=np.float32
                        )
                    )

                observed = {key(point) for point in x}
                query = query[[key(point) not in observed for point in query]]
                # Compare EI values; near ties need not choose bitwise-equal indices.
                chosen = np.array(trace[n]["x"])
                score = acquisition(x, observations, np.vstack([query, chosen]))
                assert score[-1] >= float(score[:-1].max()) - 1e-8
                queries += 1
        comparisons.append(
            {
                "case": entry["case"],
                "method": entry["method"],
                "accepted": report["accepted"],
                "reasons": report["reasons"],
                "search_evaluations": report["search"]["evaluations"],
                "native_render_calls": report["native_render_calls"],
            }
        )
    assert max_loss_error < 1e-12 and max_metric_error < 1e-10
    result = {
        "runs": len(comparisons),
        "raw_native_renders_verified": renders,
        "independent_gp_acquisition_decisions": queries,
        "maximum_objective_difference": max_loss_error,
        "maximum_relative_rms_or_max_metric_difference": max_metric_error,
        "max_reopen_error": max_reopen,
        "admission_decisions_reconstructed": len(comparisons),
        "comparisons": comparisons,
        "qualification": "Reconstruction of bounded synthetic experiments; observed sensitivity screening is not global identifiability or calibrated confidence.",
    }
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
