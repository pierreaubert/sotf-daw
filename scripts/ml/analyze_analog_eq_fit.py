"""Independently reconstruct recorded fit losses, held gates and GP posterior."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from analog_eq_fit import posterior


def pcm(path):
    return np.fromfile(path, dtype="<f4").astype(float)


def score(actual, target):
    error = actual - target
    return {
        "relative_rms": float(np.linalg.norm(error) / np.linalg.norm(target)),
        "max_abs": float(np.max(abs(error))),
        "sample_peak": float(np.max(abs(actual))),
    }


def analyze(evaluation):
    result = []
    for case in evaluation["cases"]:
        spec = json.loads(Path(case["spec"]).read_text())
        captures = json.loads(Path(spec["capture_manifest"]).read_text())
        folder = Path(case["report"]).parent
        report = json.loads(Path(case["report"]).read_text())
        trace = report["search"]["trace"]
        train = [r for r in captures["captures"] if r["split"] == "train"]
        max_loss_error = 0.0
        for i, row in enumerate(trace):
            groups = {}
            for j, record in enumerate(train):
                actual = pcm(folder / "search" / f"{i}-{j}" / "output.f32le")
                target = pcm(record["wet_file"])
                error = actual - target
                loss = float(error @ error / (target @ target))
                groups.setdefault(record["source_group"], []).append(loss)
            loss = float(np.mean([np.mean(v) for v in groups.values()]))
            max_loss_error = max(max_loss_error, abs(loss - row["loss"]))
        assert max_loss_error < 1e-12
        assert (
            report["native_objective_calls"] == len(trace) <= report["search"]["budget"]
        )
        assert report["search"]["best"]["loss"] == min(r["loss"] for r in trace)
        candidate = folder / "candidate.json"
        assert (
            hashlib.sha256(candidate.read_bytes()).hexdigest()
            == report["candidate_sha256"]
        )
        failures = set()
        max_metric_error = 0.0
        for split in ["validation", "test"]:
            records = [r for r in captures["captures"] if r["split"] == split]
            for i, record in enumerate(records):
                actual = pcm(folder / f"{split}-{i}" / "output.f32le")
                reopened = pcm(folder / f"{split}-{i}-reopen" / "output.f32le")
                target = pcm(record["wet_file"])
                current = score(actual[: len(target)], target)
                row = next(
                    r for r in report["qualification"] if r["id"] == record["id"]
                )
                for key, value in current.items():
                    max_metric_error = max(
                        max_metric_error, abs(value - row["metrics"][key])
                    )
                assert np.max(abs(actual - reopened)) == row["reopen_error"]
                assert np.max(abs(actual)) == row["observation_peak"]
                assert (
                    hashlib.sha256(
                        (folder / f"{split}-{i}" / "output.f32le").read_bytes()
                    ).hexdigest()
                    == row["output_sha256"]
                )
                policy = report["policy"]
                if (
                    current["relative_rms"] > policy["maximum_relative_rms"]
                    or current["max_abs"] > policy["maximum_abs_error"]
                ):
                    failures.add(split + "_waveform_mismatch")
                if np.max(abs(actual)) > policy["maximum_sample_peak"]:
                    failures.add(split + "_sample_headroom")
                if np.max(abs(actual - reopened)) > policy["reopen_tolerance"]:
                    failures.add("native_json_reopen_mismatch")
        assert max_metric_error < 1e-12
        assert sorted(failures) == report["reasons"]
        assert report["accepted"] == (not failures)
        assert (folder / "preset.json").exists() == report["accepted"]
        if report["accepted"]:
            assert (folder / "preset.json").read_bytes() == candidate.read_bytes()

        # Rebuild the GP with a direct dense solve, without the production kernel
        # or Cholesky implementation. This checks learned posterior calculations.
        observations = trace[:8]
        x = np.array([r["x"] for r in observations])
        models = np.array([r["model"] for r in observations])
        query = np.array([[0.17, 0.43, 0.79], [0.61, 0.27, 0.31]])
        categories = np.array(spec["models"][:2])

        def covariance(a, b, ma, mb):
            z = np.sqrt(5 * np.sum((a[:, None] - b[None, :]) ** 2, axis=2)) / 0.25
            return (1 + z + z**2 / 3) * np.exp(-z) * (ma[:, None] == mb[None, :])

        k = covariance(x, x, models, models) + np.eye(len(x)) * 1e-6
        cross = covariance(x, query, models, categories)
        y = np.log10(np.maximum([r["loss"] for r in observations], 1e-12))
        scale = max(float(y.std()), 0.1)
        mean = y.mean() + cross.T @ np.linalg.solve(k, y - y.mean())
        sigma = scale * np.sqrt(
            np.maximum(1 - np.sum(cross * np.linalg.solve(k, cross), axis=0), 0)
        )
        actual_mean, actual_sigma = posterior(observations, query, categories)
        posterior_error = float(
            max(np.max(abs(mean - actual_mean)), np.max(abs(sigma - actual_sigma)))
        )
        assert posterior_error < 1e-10
        result.append(
            {
                "case": case["case"],
                "method": case["method"],
                "native_objectives_recomputed": len(trace),
                "training_loss_max_error": max_loss_error,
                "held_metric_max_error": max_metric_error,
                "posterior_max_error": posterior_error,
                "accepted": report["accepted"],
                "reasons": sorted(failures),
            }
        )
    return {
        "cases": result,
        "scope": "Independent arithmetic on retained native PCM and search reports; generated fixtures only, no physical hardware or general ML advantage claim.",
    }


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--evaluation", type=Path, required=True)
    p.add_argument("--report", type=Path, required=True)
    args = p.parse_args()
    result = analyze(json.loads(args.evaluation.read_text()))
    with args.report.open("x") as out:
        out.write(json.dumps(result, indent=2, allow_nan=False) + "\n")
    print(json.dumps(result))


if __name__ == "__main__":
    main()
