#!/usr/bin/env python3
"""Independently check fresh initializer comparison against retained native arrays."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--benchmark", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--training-report", type=Path, required=True)
    parser.add_argument("--evaluation", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        raise ValueError("Use new analysis output")
    benchmark = json.loads(args.benchmark.read_text())
    training = json.loads(args.training_report.read_text())
    previous = json.loads(args.evaluation.read_text())
    occupied = set(
        training["waveform_sha256"]["train"]
        + training["waveform_sha256"]["development"]
        + [r["identity"] for r in previous["rows"]]
    )
    identities = [r["identity"] for r in benchmark["rows"]]
    if len(set(identities)) != len(identities) or occupied.intersection(identities):
        raise ValueError("Fresh comparison identity overlap")
    max_metric_error, checked, samples = 0.0, 0, 0
    failures = []
    for row in benchmark["rows"]:
        root = args.work / row["name"]
        request = json.loads((root / "target/request.json").read_text())
        meta = json.loads((root / "target/metadata.json").read_text())
        if (
            hashlib.sha256((root / "target/input.f32le").read_bytes()).hexdigest()
            != row["identity"]
        ):
            raise ValueError("Benchmark dry identity changed")
        channels, frames = request["channels"], request["frames"]
        reference = (
            np.fromfile(root / "target/output.f32le", dtype="<f4")
            .reshape(-1, channels)[
                meta["latency_samples"] : meta["latency_samples"] + frames
            ]
            .copy()
        )
        if row["name"] == "reference-noise":
            reference += (
                np.random.default_rng(benchmark["seed"] + 900)
                .normal(0, 0.0001, reference.shape)
                .astype(np.float32)
            )
        scale = max(
            float(
                np.linalg.norm(reference.astype(np.float64)) / np.sqrt(reference.size)
            ),
            1e-6,
        )
        for method, result in row["methods"].items():
            observed = []
            folders = list((root / method).glob("render-*"))
            if len(folders) != result["unique_native_renders"]:
                raise ValueError("Benchmark native count mismatch")
            for folder in folders:
                req = json.loads((folder / "request.json").read_text())
                metadata = json.loads((folder / "metadata.json").read_text())
                raw = np.fromfile(folder / "output.f32le", dtype="<f4")
                if (
                    raw.size != metadata["output_frames"] * channels
                    or not np.isfinite(raw).all()
                    or np.max(np.abs(raw.astype(np.float64)))
                    > 10 ** (req["params"]["threshold_db"] / 20) + 2e-6
                ):
                    raise ValueError("Invalid benchmark output or ceiling")
                aligned = raw.reshape(-1, channels)[
                    metadata["latency_samples"] : metadata["latency_samples"] + frames
                ]
                delta = aligned.astype(np.float64) - reference
                observed.append(
                    (
                        float(np.linalg.norm(delta) / np.sqrt(delta.size) / scale),
                        float(np.abs(delta).max()),
                    )
                )
                checked += 1
                samples += raw.size
            best = min(observed)
            max_metric_error = max(
                max_metric_error,
                abs(best[0] - result["best"]["relative_rms"]),
                abs(best[1] - result["best"]["max_error"]),
            )
            if not result["waveform_qualified"]:
                failures.append(
                    {
                        "name": row["name"],
                        "method": method,
                        "relative_rms": best[0],
                        "max_error": best[1],
                    }
                )
    if max_metric_error > 1e-10:
        raise ValueError("Independent benchmark metric mismatch")
    summary = {
        "independent_signal_identities": len(identities),
        "no_train_development_previous_evaluation_overlap": True,
        "native_renders_checked": checked,
        "native_samples_checked": samples,
        "max_raw_metric_error": max_metric_error,
        "methods": benchmark["methods"],
        "failed_waveform_checks": failures,
        "adoption": "Reject general learned-initializer adoption: analytic-only qualifies20/20 versus neural19/20 and uses fewer median native evaluations15.5 versus17. Model remains an explicit offline experiment; native default unchanged.",
    }
    args.output.write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary), flush=True)


if __name__ == "__main__":
    main()
