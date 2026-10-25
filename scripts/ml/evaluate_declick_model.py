#!/usr/bin/env python3
"""Frozen synthetic final repair/damage comparison, including ambiguous impulses."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from declick_cases import case
from declick_model import Model, qualify
from declick_native import features, groups, render


def metrics(actual, clean):
    error = actual.astype(np.float64) - clean
    return {
        "sse": float(np.sum(error**2)),
        "peak_error": float(np.max(np.abs(error))),
        "changed_frames": int(np.sum(np.any(actual != clean, axis=1))),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--protocol", type=Path, required=True)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    protocol = json.loads(args.protocol.read_text())
    report = json.loads(args.model.with_name("training-report.json").read_text())
    digest = hashlib.sha256(args.model.read_bytes()).hexdigest()
    if digest != report["weights_sha256"]:
        raise ValueError("Frozen checkpoint mismatch")
    model = Model(args.model, digest)
    manifest = json.loads(args.model.with_name("data-manifest.json").read_text())
    excluded = {
        r["signal_identity"]
        for name in ["train", "development"]
        for r in manifest[name]
    }
    args.output.mkdir(parents=True, exist_ok=False)
    args.work.mkdir(parents=True, exist_ok=False)
    (args.output / "frozen-model.json").write_text(
        json.dumps(
            {
                "weights_sha256": digest,
                "threshold": model.threshold,
                "selected_epoch": report["best_epoch"],
                "classical_threshold": report["classical_tail_threshold"],
            },
            indent=2,
        )
        + "\n"
    )
    rows = []
    cases = [
        (i, kind, 48000, 1)
        for i in range(protocol["final_base_signals"])
        for kind in protocol["final_kinds"]
    ]
    cases.extend([(0, "click", rate, 1) for rate in protocol["shift_rates"]])
    cases.append((0, "click", 48000, protocol["channel_shift"]))
    for index, kind, rate, channels in cases:
        name = f"{index}-{kind}-{rate}-{channels}"
        clean, dry, metadata = case(
            protocol["final_seed"] + index, kind, rate, channels
        )
        if metadata["signal_identity"] in excluded:
            raise ValueError("Final signal overlaps training/development")
        native, info = render(dry, rate, args.renderer, args.work / name)
        qualified, decisions = qualify(dry, native, rate, model)
        classical = dry.copy()
        for start, stop in groups(dry, native):
            if rate != 48000 or channels != 1:
                continue
            try:
                x = features(dry, native, start, stop, rate)
                ratio = np.max(
                    np.expm1(x[[3, 7, 11]]) / np.maximum(np.expm1(x[[2, 6, 10]]), 1e-6)
                )
            except ValueError:
                continue
            if ratio <= report["classical_tail_threshold"]["threshold"]:
                classical[start:stop] = native[start:stop]
        # Bitwise support and two-choice invariants, not a quality assumption.
        changed = np.any(dry != native, axis=1)
        np.testing.assert_array_equal(qualified[~changed], dry[~changed])
        for row in decisions:
            start, stop = row["start"], row["stop"]
            np.testing.assert_array_equal(
                qualified[start:stop],
                native[start:stop] if row["accepted"] else dry[start:stop],
            )
        np.savez(
            args.work / (name + ".npz"),
            clean=clean,
            dry=dry,
            native=native,
            learned=qualified,
            classical=classical,
        )
        rows.append(
            dict(
                metadata,
                case=name,
                native_latency=info["latency_samples"],
                decisions=decisions,
                dry=metrics(dry, clean),
                native=metrics(native, clean),
                learned=metrics(qualified, clean),
                classical=metrics(classical, clean),
            )
        )
    summary = {
        "model_sha256": digest,
        "cases": len(rows),
        "rows": rows,
        "strata": {
            kind: {
                method: {
                    "total_sse": sum(
                        r[method]["sse"]
                        for r in rows
                        if r["kind"] == kind
                        and r["sample_rate"] == 48000
                        and r["channels"] == 1
                    ),
                    "worst_peak_error": max(
                        r[method]["peak_error"]
                        for r in rows
                        if r["kind"] == kind
                        and r["sample_rate"] == 48000
                        and r["channels"] == 1
                    ),
                }
                for method in ["dry", "native", "learned", "classical"]
            }
            for kind in protocol["final_kinds"]
        },
        "qualification": "Synthetic waveform-error comparison only; ambiguous-programme and click can be observationally identical, so no semantic or perceptual safety guarantee",
    }
    if hashlib.sha256(args.model.read_bytes()).hexdigest() != digest:
        raise ValueError("Model changed during final evaluation")
    (args.output / "comparison.json").write_text(
        json.dumps(summary, indent=2, allow_nan=False) + "\n"
    )
    print(json.dumps({"cases": len(rows), "strata": summary["strata"]}), flush=True)


if __name__ == "__main__":
    main()
