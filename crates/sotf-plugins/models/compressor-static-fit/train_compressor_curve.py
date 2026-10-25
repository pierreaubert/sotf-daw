#!/usr/bin/env python3
"""Train a synthetic static-curve initializer; never infer timing from curves."""

import argparse
import copy
import hashlib
import json
from pathlib import Path

import torch
from compressor_curve import FORMAT, GRID, LOWER, UPPER, dataset
from torch import nn


def differentiable_response(p):
    levels = torch.tensor(GRID, dtype=p.dtype)
    difference = levels - p[:, 0, None]
    width, slope = p[:, 2, None], 1 - torch.exp(-p[:, 1, None])
    hard = difference.clamp(min=0) * slope
    soft = torch.where(
        difference > width / 2,
        difference * slope,
        (difference + width / 2).clamp(min=0).square()
        * slope
        / (2 * width.clamp(min=0.1)),
    )
    return levels + p[:, 3, None] - torch.where(width < 0.1, hard, soft)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--examples", type=int, default=4096)
    parser.add_argument("--development", type=int, default=512)
    parser.add_argument("--epochs", type=int, default=100)
    parser.add_argument("--seed", type=int, default=20341026)
    args = parser.parse_args()
    if min(args.examples, args.development, args.epochs) < 1:
        raise ValueError("Positive counts required")
    args.output.mkdir(parents=True, exist_ok=False)
    torch.set_num_threads(2)
    torch.manual_seed(20261026)
    train, development = (
        dataset(args.examples, args.seed),
        dataset(args.development, args.seed + 10000),
    )
    if set(train[2]) & set(development[2]):
        raise ValueError("Split identity overlap")
    model = nn.Sequential(
        nn.Linear(128, 64),
        nn.SiLU(),
        nn.Linear(64, 64),
        nn.SiLU(),
        nn.Linear(64, 4),
        nn.Tanh(),
    )
    optimizer = torch.optim.Adam(model.parameters(), lr=0.001)
    lo, span = (
        torch.tensor(LOWER, dtype=torch.float32),
        torch.tensor(UPPER - LOWER, dtype=torch.float32),
    )
    x = torch.tensor((train[1] - GRID) / 84, dtype=torch.float32)
    y = torch.tensor(train[1], dtype=torch.float32)
    labels = torch.tensor(
        2 * (train[0] - LOWER) / (UPPER - LOWER) - 1, dtype=torch.float32
    )
    dx = torch.tensor((development[1] - GRID) / 84, dtype=torch.float32)
    dy = torch.tensor(development[1], dtype=torch.float32)
    best, state, selected, history = float("inf"), None, 0, []
    for epoch in range(args.epochs):
        for batch in torch.randperm(len(x)).split(128):
            optimizer.zero_grad()
            normalized = model(x[batch])
            p = lo + (normalized + 1) * span / 2
            loss = (differentiable_response(p) - y[batch]).square().mean() + 0.1 * (
                normalized - labels[batch]
            ).square().mean()
            if not torch.isfinite(loss):
                raise ValueError("Non-finite training loss")
            loss.backward()
            torch.nn.utils.clip_grad_norm_(model.parameters(), 10)
            optimizer.step()
        with torch.no_grad():
            dev = float(
                (differentiable_response(lo + (model(dx) + 1) * span / 2) - dy)
                .square()
                .mean()
            )
        history.append({"epoch": epoch + 1, "development_mse_db2": dev})
        if dev < best:
            best, selected, state = dev, epoch + 1, copy.deepcopy(model.state_dict())
    portable = {
        "format": FORMAT,
        "grid_dbfs": GRID.tolist(),
        "layers": [
            {
                "weight": state[f"{index}.weight"].tolist(),
                "bias": state[f"{index}.bias"].tolist(),
            }
            for index in [0, 2, 4]
        ],
    }
    (args.output / "model.json").write_text(
        json.dumps(portable, allow_nan=False) + "\n"
    )
    manifest = {
        "split_ids": {"train": train[2], "development": development[2]},
        "seed": args.seed,
        "data": "Synthetic static gain-computer curves; no hardware/audio or timing labels",
        "license": "GPL-3.0-or-later",
        "arguments": {
            k: str(v) if isinstance(v, Path) else v for k, v in vars(args).items()
        },
    }
    (args.output / "data-manifest.json").write_text(
        json.dumps(manifest, indent=2) + "\n"
    )
    report = {
        "weights_sha256": hashlib.sha256(
            (args.output / "model.json").read_bytes()
        ).hexdigest(),
        "selected_epoch": selected,
        "development_mse_db2": best,
        "history": history,
        "qualification": "Initializer only; bounded refinement, classical comparison and actual native validation required",
    }
    (args.output / "training-report.json").write_text(
        json.dumps(report, indent=2) + "\n"
    )
    for name in ["train_compressor_curve.py", "compressor_curve.py"]:
        (args.output / name).write_bytes(Path(__file__).with_name(name).read_bytes())
    print(json.dumps({k: v for k, v in report.items() if k != "history"}), flush=True)


if __name__ == "__main__":
    main()
