#!/usr/bin/env python3
"""Independent feature/forward/repair-error checks on saved synthetic arrays."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
import torch


def independent_features(dry, native, start, stop, rate):
    values = [(stop - start) / 32, np.log(rate / 48000)]
    for duration in (0.002, 0.01, 0.04):
        samples = round(duration * rate)
        left, right = (
            dry[start - samples : start].astype(np.float64),
            dry[stop : stop + samples].astype(np.float64),
        )
        scale = max(
            1e-6,
            np.sqrt((np.sum(left**2) + np.sum(right**2)) / (left.size + right.size)),
        )
        delta = dry[start:stop].astype(np.float64) - native[start:stop]
        for block in (left, right, delta):
            values.append(
                np.clip(
                    np.log1p(np.linalg.norm(block) / np.sqrt(block.size) / scale), 0, 8
                )
            )
        block = dry[start - samples : stop + samples].astype(np.float64)
        diff = block[1:] - block[:-1]
        values.append(
            np.clip(np.log1p(np.linalg.norm(diff) / np.sqrt(diff.size) / scale), 0, 8)
        )
    original, repaired = (
        dry[start - 8 : stop + 8].astype(np.float64),
        native[start - 8 : stop + 8].astype(np.float64),
    )
    for _ in range(2):
        original, repaired = original[1:] - original[:-1], repaired[1:] - repaired[:-1]
        values.append(
            np.clip(
                np.log((np.mean(original**2) + 1e-12) / (np.mean(repaired**2) + 1e-12)),
                -8,
                8,
            )
        )
    return np.asarray(values, dtype=np.float32)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    args = parser.parse_args()
    torch.set_num_threads(2)
    summary = json.loads((args.artifacts / "final/comparison.json").read_text())
    if hashlib.sha256(args.model.read_bytes()).hexdigest() != summary["model_sha256"]:
        raise ValueError("Model identity changed")
    model = json.loads(args.model.read_bytes())
    maximum_forward_error, maximum_metric_error, scored = 0.0, 0.0, 0
    false_accepts, true_accepts, pairs = 0, 0, 0
    arrays = {}
    for row in summary["rows"]:
        path = args.work / (row["case"] + ".npz")
        arrays[row["case"]] = hashlib.sha256(path.read_bytes()).hexdigest()
        with np.load(path, allow_pickle=False) as values:
            clean, dry, native, learned = (
                values[name] for name in ["clean", "dry", "native", "learned"]
            )
            for method in ["dry", "native", "learned", "classical"]:
                difference = values[method].astype(np.float64) - clean
                sse = float(np.linalg.norm(difference) ** 2)
                maximum_metric_error = max(
                    maximum_metric_error, abs(sse - row[method]["sse"])
                )
            changed = np.any(dry != native, axis=1)
            np.testing.assert_array_equal(learned[~changed], dry[~changed])
            for decision in row["decisions"]:
                start, stop = decision["start"], decision["stop"]
                np.testing.assert_array_equal(
                    learned[start:stop],
                    native[start:stop] if decision["accepted"] else dry[start:stop],
                )
                if decision["probability"] is not None:
                    x = independent_features(
                        dry, native, start, stop, row["sample_rate"]
                    )
                    z = torch.tensor(
                        (x.astype(np.float64) - model["mean"]) / model["scale"]
                    )
                    hidden = torch.tanh(
                        torch.tensor(model["w1"], dtype=torch.float64) @ z
                        + torch.tensor(model["b1"], dtype=torch.float64)
                    )
                    probability = torch.sigmoid(
                        hidden @ torch.tensor(model["w2"], dtype=torch.float64)
                        + model["b2"]
                    ).item()
                    maximum_forward_error = max(
                        maximum_forward_error,
                        abs(probability - decision["probability"]),
                    )
                    if decision["accepted"] != (probability >= model["threshold"]):
                        raise ValueError("Admission threshold mismatch")
                    scored += 1
                if decision["accepted"]:
                    before = np.sum(
                        (dry[start:stop].astype(np.float64) - clean[start:stop]) ** 2
                    )
                    after = np.sum(
                        (native[start:stop].astype(np.float64) - clean[start:stop]) ** 2
                    )
                    false_accepts += int(after > before + 1e-12)
                    true_accepts += int(after < before - 1e-12)
            if (
                row["kind"] == "ambiguous-programme"
                and row["sample_rate"] == 48000
                and row["channels"] == 1
            ):
                other = row["case"].replace("ambiguous-programme", "click")
                with np.load(args.work / (other + ".npz"), allow_pickle=False) as click:
                    for name in ["dry", "native", "learned"]:
                        np.testing.assert_array_equal(values[name], click[name])
                    if np.array_equal(clean, click["clean"]):
                        raise ValueError("Ambiguity references unexpectedly equal")
                pairs += 1
    if maximum_forward_error > 1e-7 or maximum_metric_error > 1e-8:
        raise ValueError("Independent feature/forward/error comparison failed")
    result = {
        "cases": len(summary["rows"]),
        "scored_native_groups": scored,
        "accepted_beneficial_groups": true_accepts,
        "accepted_harmful_groups": false_accepts,
        "identical_observation_opposite_reference_pairs": pairs,
        "max_probability_discrepancy": maximum_forward_error,
        "max_sse_discrepancy": maximum_metric_error,
        "qualification": "Synthetic reconstruction error only; harmful accepted legitimate impulses preclude general adoption",
    }
    (args.artifacts / "independent-analysis.json").write_text(
        json.dumps(result, indent=2) + "\n"
    )
    (args.artifacts / "external-array-manifest.json").write_text(
        json.dumps(arrays, indent=2) + "\n"
    )
    print(json.dumps(result), flush=True)


if __name__ == "__main__":
    main()
