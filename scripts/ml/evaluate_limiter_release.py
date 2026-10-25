#!/usr/bin/env python3
"""Fresh synthetic native/shift evaluation of learned and classical release fitting."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from limiter_native import PARAMS, render
from limiter_release import AMBIGUITY_SLACK, MAX_LIMIT, RMS_LIMIT, Model, fit
from limiter_release_cases import signal


def cases(seed):
    for index in range(12):
        x, fixed, identity, releases = signal(seed, index)
        yield (
            f"fresh-{index:02d}",
            x,
            48000,
            fixed,
            releases[0],
            identity,
            "native-positive",
            None,
        )
    for index, (name, rate, channels, family, seconds, noise) in enumerate(
        [
            ("rate-44100", 44100, 2, 0, 4, 0),
            ("rate-96000", 96000, 1, 1, 4, 0),
            ("rate-192000-six-channel", 192000, 6, 2, 3, 0),
            ("fm-carrier", 48000, 2, 3, 4, 0),
            ("square-carrier", 48000, 1, 4, 4, 0),
            ("short-context", 48000, 2, 0, 2, 0),
            ("long-context", 48000, 1, 1, 8, 0),
            ("reference-noise", 48000, 2, 2, 4, 0.0001),
        ]
    ):
        x, fixed, identity, releases = signal(
            seed, 100 + index, rate, channels, family, seconds
        )
        yield name, x, rate, fixed, releases[0], identity, "shift-positive", noise
    fixed = {k: v for k, v in PARAMS.items() if k != "release_ms"}
    for name in [
        "silence",
        "no-limiting",
        "constant-clipping",
        "impulse-silence",
        "incompatible-reference",
    ]:
        x = np.zeros((192000, 1), np.float32)
        if name == "no-limiting":
            x.fill(0.01)
        elif name == "constant-clipping":
            x.fill(2)
        elif name == "impulse-silence":
            x[0] = 2
        elif name == "incompatible-reference":
            x, fixed, _, _ = signal(seed, 200, channels=1)
        identity = hashlib.sha256(x.astype("<f4").tobytes()).hexdigest()
        yield name, x, 48000, fixed, 100.0, identity, "negative-control", None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--model-sha256", required=True)
    parser.add_argument("--training-report", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--seed", type=int, default=20471026)
    args = parser.parse_args()
    if args.output.exists() or args.work.exists():
        raise ValueError("Use new evaluation paths")
    model = Model(args.model, args.model_sha256)
    training = json.loads(args.training_report.read_text())
    occupied = set(
        training["waveform_sha256"]["train"]
        + training["waveform_sha256"]["development"]
    )
    rows = []
    for name, dry, rate, fixed, release, identity, kind, noise in cases(args.seed):
        if identity in occupied:
            raise ValueError("Final waveform identity overlaps training/development")
        root = args.work / name
        target, _ = render(
            dry,
            rate,
            args.renderer,
            root / "target",
            127,
            fixed | {"release_ms": release},
        )
        if noise:
            target = target + np.random.default_rng(args.seed + 900).normal(
                0, noise, target.shape
            ).astype(np.float32)
        if name == "incompatible-reference":
            target = target * 0.7
        dry.astype("<f4").tofile(root / "dry.f32le")
        target.astype("<f4").tofile(root / "reference.f32le")
        request = {
            "format": "sotf-limiter-release-target-v1",
            "alignment": "latency_removed",
            "sample_rate": rate,
            "frames": len(dry),
            "channels": dry.shape[1],
            "dry": "dry.f32le",
            "reference": "reference.f32le",
            "fixed_params": fixed,
        }
        (root / "target.json").write_text(json.dumps(request, indent=2) + "\n")
        result = fit(dry, target, rate, fixed, args.renderer, root / "fit", model)
        row = {
            "name": name,
            "kind": kind,
            "identity": identity,
            "seed": args.seed,
            "true_release_ms": release,
            "work": str(root.resolve()),
            "dry_sha256": hashlib.sha256((root / "dry.f32le").read_bytes()).hexdigest(),
            "reference_sha256": hashlib.sha256(
                (root / "reference.f32le").read_bytes()
            ).hexdigest(),
            "result": result,
        }
        rows.append(row)
        (root / "report.json").write_text(json.dumps(row, indent=2) + "\n")
        print(
            json.dumps(
                {
                    "name": name,
                    "accepted": result["accepted"],
                    "true_release_ms": release,
                    "fitted_release_ms": result["best"]["release_ms"],
                    "reasons": result["reasons"],
                }
            ),
            flush=True,
        )
    report = {
        "model_sha256": model.digest,
        "renderer_sha256": hashlib.sha256(args.renderer.read_bytes()).hexdigest(),
        "seed": args.seed,
        "gates": {
            "relative_rms": RMS_LIMIT,
            "max_absolute_error": MAX_LIMIT,
            "ambiguity_rms_slack": AMBIGUITY_SLACK,
            "release_separation_ratio": 1.1,
        },
        "rows": rows,
        "qualification": "25 fresh synthetic probes, 20 excited positives and five negative controls; one fixed native profile. No real recordings, hardware, listening or host validation.",
    }
    args.output.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
