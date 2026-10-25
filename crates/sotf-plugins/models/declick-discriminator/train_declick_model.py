#!/usr/bin/env python3
"""Train a bounded classifier on actual native repairs of new synthetic signals."""

import argparse
import copy
import hashlib
import json
from pathlib import Path

import numpy as np
import torch
from declick_cases import case
from declick_model import FORMAT
from declick_native import features, groups, render
from torch import nn


def collect(count, seed, renderer, output):
    x, labels, rows = [], [], []
    for index in range(count):
        for kind in ["click", "percussion"]:
            clean, dry, metadata = case(seed + index, kind)
            native, info = render(dry, 48000, renderer, output / f"{kind}-{index}")
            supported = 0
            for start, stop in groups(dry, native):
                try:
                    value = features(dry, native, start, stop, 48000)
                except ValueError:
                    continue
                before = np.sum(
                    (dry[start:stop].astype(np.float64) - clean[start:stop]) ** 2
                )
                after = np.sum(
                    (native[start:stop].astype(np.float64) - clean[start:stop]) ** 2
                )
                if abs(before - after) <= 1e-12:
                    continue
                x.append(value)
                labels.append(float(after < before))
                supported += 1
            rows.append(
                dict(
                    metadata,
                    native_latency=info["latency_samples"],
                    supported_groups=supported,
                )
            )
    if not labels or len(set(labels)) != 2:
        raise ValueError(
            "Training/development require native candidates in both classes"
        )
    return np.asarray(x, dtype=np.float32), np.asarray(labels, dtype=np.float32), rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--examples", type=int, default=128)
    parser.add_argument("--development", type=int, default=32)
    parser.add_argument("--epochs", type=int, default=150)
    parser.add_argument("--seed", type=int, default=20311026)
    parser.add_argument("--exclude", type=Path, action="append", default=[])
    args = parser.parse_args()
    if min(args.examples, args.development, args.epochs) < 1:
        raise ValueError("Counts must be positive")
    args.output.mkdir(parents=True, exist_ok=False)
    args.work.mkdir(parents=True, exist_ok=False)
    torch.set_num_threads(2)
    torch.manual_seed(20261026)
    train = collect(args.examples, args.seed, args.renderer, args.work / "train")
    development = collect(
        args.development, args.seed + 10000, args.renderer, args.work / "development"
    )
    if {r["signal_identity"] for r in train[2]} & {
        r["signal_identity"] for r in development[2]
    }:
        raise ValueError("Underlying signal identity overlap")

    def identities(value):
        found = set()
        if isinstance(value, dict):
            for key, child in value.items():
                if key == "signal_identity" and isinstance(child, str):
                    found.add(child)
                else:
                    found.update(identities(child))
        elif isinstance(value, list):
            for child in value:
                found.update(identities(child))
        return found

    used = identities(train[2]) | identities(development[2])
    for excluded in args.exclude:
        if used & identities(json.loads(excluded.read_text())):
            raise ValueError("Current split overlaps excluded signal identities")
    mean = train[0].mean(axis=0)
    scale = np.maximum(train[0].std(axis=0), 1e-3)
    x = torch.as_tensor((train[0] - mean) / scale)
    y = torch.as_tensor(train[1])[:, None]
    dev_x = torch.as_tensor((development[0] - mean) / scale)
    dev_y = torch.as_tensor(development[1])[:, None]
    model = nn.Sequential(nn.Linear(16, 16), nn.Tanh(), nn.Linear(16, 1))
    optimizer = torch.optim.Adam(model.parameters(), lr=0.003)
    best, best_epoch, state, history = float("inf"), 0, None, []
    for epoch in range(args.epochs):
        model.train()
        for batch in torch.randperm(len(x)).split(32):
            optimizer.zero_grad()
            loss = nn.functional.binary_cross_entropy_with_logits(
                model(x[batch]), y[batch]
            )
            if not torch.isfinite(loss):
                raise ValueError("Non-finite training loss")
            loss.backward()
            optimizer.step()
        model.eval()
        with torch.no_grad():
            dev_loss = float(
                nn.functional.binary_cross_entropy_with_logits(model(dev_x), dev_y)
            )
        history.append({"epoch": epoch + 1, "development_bce": dev_loss})
        if dev_loss < best:
            best, best_epoch, state = (
                dev_loss,
                epoch + 1,
                copy.deepcopy(model.state_dict()),
            )
    model.load_state_dict(state)
    with torch.no_grad():
        scores = model(dev_x).sigmoid().numpy()[:, 0]
    thresholds = []
    for threshold in [0.5, 0.7, 0.8, 0.9, 0.95, 0.99, 1.0]:
        decisions = scores >= threshold
        thresholds.append(
            {
                "threshold": threshold,
                "false_accepts": int(np.sum(decisions & (development[1] == 0))),
                "true_accepts": int(np.sum(decisions & (development[1] == 1))),
            }
        )
    choice = min(
        thresholds,
        key=lambda r: (r["false_accepts"], -r["true_accepts"], -r["threshold"]),
    )
    ratios = np.max(
        np.expm1(development[0][:, [3, 7, 11]])
        / np.maximum(np.expm1(development[0][:, [2, 6, 10]]), 1e-6),
        axis=1,
    )
    baseline_trials = []
    for threshold in [1.0, 1.1, 1.25, 1.5, 2.0, 4.0, 1000000.0]:
        decisions = ratios <= threshold
        baseline_trials.append(
            {
                "threshold": threshold,
                "false_accepts": int(np.sum(decisions & (development[1] == 0))),
                "true_accepts": int(np.sum(decisions & (development[1] == 1))),
            }
        )
    baseline_choice = min(
        baseline_trials,
        key=lambda r: (r["false_accepts"], -r["true_accepts"], r["threshold"]),
    )
    portable = {
        "format": FORMAT,
        "domain": {
            "sample_rate": 48000,
            "channels": 1,
            "native_params": "legacy-sensitivity5-linked-v1",
        },
        "mean": mean.tolist(),
        "scale": scale.tolist(),
        "w1": state["0.weight"].tolist(),
        "b1": state["0.bias"].tolist(),
        "w2": state["2.weight"][0].tolist(),
        "b2": float(state["2.bias"][0]),
        "threshold": choice["threshold"],
    }
    (args.output / "model.json").write_text(
        json.dumps(portable, allow_nan=False) + "\n"
    )
    manifest = {
        "train": train[2],
        "development": development[2],
        "seed": args.seed,
        "arguments": {
            k: str(v) if isinstance(v, Path) else v
            for k, v in vars(args).items()
            if k != "exclude"
        },
        "renderer_sha256": hashlib.sha256(args.renderer.read_bytes()).hexdigest(),
        "exclusion_sources": {
            str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in args.exclude
        },
        "data": "Synthetic clean/corrupt pairs; no recordings or perceptual labels",
        "license": "GPL-3.0-or-later",
    }
    (args.output / "data-manifest.json").write_text(
        json.dumps(manifest, indent=2) + "\n"
    )
    report = {
        "weights_sha256": hashlib.sha256(
            (args.output / "model.json").read_bytes()
        ).hexdigest(),
        "best_epoch": best_epoch,
        "best_development_bce": best,
        "train_groups": len(train[0]),
        "development_groups": len(development[0]),
        "threshold_trials": thresholds,
        "selected_threshold": choice,
        "classical_tail_threshold": baseline_choice,
        "classical_threshold_trials": baseline_trials,
        "history": history,
        "qualification": "Development-only selection; independent final repair/damage evaluation required",
    }
    (args.output / "training-report.json").write_text(
        json.dumps(report, indent=2) + "\n"
    )
    for name in [
        "train_declick_model.py",
        "declick_native.py",
        "declick_model.py",
        "declick_cases.py",
    ]:
        (args.output / name).write_bytes(Path(__file__).with_name(name).read_bytes())
    print(json.dumps({k: v for k, v in report.items() if k != "history"}), flush=True)


if __name__ == "__main__":
    main()
