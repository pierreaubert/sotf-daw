#!/usr/bin/env python3
"""Train a small offline release initializer on disjoint native synthetic pairs."""

import argparse
import copy
import hashlib
import json
import os
from pathlib import Path

import numpy as np
import torch
from limiter_native import render
from limiter_release import FEATURES, FORMAT, Model, descriptors
from limiter_release_cases import signal
from torch import nn


def dataset(count, seed, renderer, root):
    features, labels, ids, wave_ids = [], [], [], []
    for index in range(count):
        dry, fixed, identity, releases = signal(seed, index)
        for variant, release in enumerate(releases):
            target, _ = render(
                dry,
                48000,
                renderer,
                root / f"{index:04d}-{variant}",
                params=fixed | {"release_ms": release},
            )
            features.append(descriptors(dry, target, 48000, fixed)[0])
            labels.append(np.log10(release) - 2)
            ids.append(f"{seed}:{index}")
            wave_ids.append(identity)
    return (
        np.asarray(features, np.float32),
        np.asarray(labels, np.float32)[:, None],
        ids,
        wave_ids,
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--identities", type=int, default=256)
    parser.add_argument("--development", type=int, default=48)
    parser.add_argument("--epochs", type=int, default=300)
    parser.add_argument("--seed", type=int, default=20421026)
    args = parser.parse_args()
    if (
        not 1 <= args.identities <= 2048
        or not 1 <= args.development <= 512
        or not 1 <= args.epochs <= 2000
    ):
        raise ValueError("Unsupported training counts")
    args.output.mkdir(parents=True, exist_ok=False)
    train = dataset(
        args.identities, args.seed, args.renderer, args.output / "native-train"
    )
    dev = dataset(
        args.development,
        args.seed + 10000,
        args.renderer,
        args.output / "native-development",
    )
    if set(train[2]) & set(dev[2]) or set(train[3]) & set(dev[3]):
        raise ValueError("Training/development identity leakage")
    torch.set_num_threads(2)
    torch.manual_seed(20421026)
    model = nn.Sequential(
        nn.Linear(FEATURES, 32), nn.Tanh(), nn.Linear(32, 1), nn.Tanh()
    )
    optimizer = torch.optim.Adam(model.parameters(), lr=0.003)
    tx, ty, dx, dy = [torch.from_numpy(a) for a in [train[0], train[1], dev[0], dev[1]]]
    best, state, selected = float("inf"), None, 0
    history = []
    # No remote telemetry: this task has no authorization to publish training data.
    metrics = args.output / "metrics.jsonl"
    for epoch in range(args.epochs):
        model.train()
        for batch in torch.randperm(len(tx)).split(64):
            optimizer.zero_grad()
            loss = (model(tx[batch]) - ty[batch]).square().mean()
            if not torch.isfinite(loss):
                raise ValueError("Non-finite release training loss")
            loss.backward()
            torch.nn.utils.clip_grad_norm_(model.parameters(), 10)
            optimizer.step()
        model.eval()
        with torch.no_grad():
            score = float((model(dx) - dy).square().mean())
        row = {
            "step": epoch + 1,
            "loss": float(loss.detach()),
            "development_log10_mse": score,
            "lr": 0.003,
        }
        history.append(row)
        with metrics.open("a") as stream:
            stream.write(json.dumps(row) + "\n")
        if score < best:
            best, selected, state = score, epoch + 1, copy.deepcopy(model.state_dict())
        if (epoch + 1) % 50 == 0:
            print(json.dumps(row), flush=True)
    portable = {
        "format": FORMAT,
        "features": FEATURES,
        "layers": [
            {
                "weight": state[f"{i}.weight"].tolist(),
                "bias": state[f"{i}.bias"].tolist(),
            }
            for i in [0, 2]
        ],
    }
    checkpoint = args.output / "model.json"
    checkpoint.write_text(json.dumps(portable, allow_nan=False) + "\n")
    digest = hashlib.sha256(checkpoint.read_bytes()).hexdigest()
    loaded = Model(checkpoint, digest)
    predictions = np.array([np.log10(loaded.predict_features(x)) - 2 for x in dev[0]])
    reloaded_mse = float(np.mean((predictions - dev[1][:, 0]) ** 2))
    if not np.isclose(reloaded_mse, best, rtol=1e-4, atol=1e-7):
        raise ValueError("Portable checkpoint mismatch")
    report = {
        "weights_sha256": digest,
        "selected_epoch": selected,
        "development_log10_mse": best,
        "portable_development_log10_mse": reloaded_mse,
        "history": history,
        "parameters": sum(p.numel() for p in model.parameters()),
        "data": "Synthetic native renders only; two release variants per disjoint signal identity; all variants remain within split",
        "license": "GPL-3.0-or-later",
        "seed": args.seed,
        "development_seed": args.seed + 10000,
        "split_ids": {
            "train": sorted(set(train[2])),
            "development": sorted(set(dev[2])),
        },
        "waveform_sha256": {
            "train": sorted(set(train[3])),
            "development": sorted(set(dev[3])),
        },
        "renderer_sha256": hashlib.sha256(args.renderer.read_bytes()).hexdigest(),
        "qualification": "Initializer only; native refinement, ambiguity and waveform qualification required",
        "tracker_environment_present": [
            k
            for k in ["WANDB_API_KEY", "TRACKIO_SPACE_ID", "MLFLOW_TRACKING_URI"]
            if os.environ.get(k)
        ],
    }
    (args.output / "training-report.json").write_text(
        json.dumps(report, indent=2) + "\n"
    )
    np.savez(
        args.output / "feature-cache.npz",
        train_x=train[0],
        train_y=train[1],
        dev_x=dev[0],
        dev_y=dev[1],
    )
    for name in [
        "train_limiter_release.py",
        "limiter_release.py",
        "limiter_release_cases.py",
        "limiter_native.py",
    ]:
        (args.output / name).write_bytes(Path(__file__).with_name(name).read_bytes())
    print(
        json.dumps(
            {
                k: v
                for k, v in report.items()
                if k not in {"history", "split_ids", "waveform_sha256"}
            }
        ),
        flush=True,
    )


if __name__ == "__main__":
    main()
