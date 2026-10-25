#!/usr/bin/env python3
"""Paired warm-start experiment: original labels versus refined neural labels.

Training and development targets are synthetic and independent of the final
evaluation. Both arms receive identical batches, steps and response loss. Only
the auxiliary parameter labels differ. This does not replace a deployed model.
"""

import argparse
import copy
import json
import time
from pathlib import Path

import eq_multiband as eq
import numpy as np
import torch
from eq_model_loading import load_model
from scipy.optimize import least_squares


def refine(parameters, rate, target, budget):
    calls = 0

    def residual(p):
        nonlocal calls
        calls += 1
        return eq.response(p, rate=rate) - target

    result = least_squares(
        residual,
        np.clip(parameters, eq.LOWER + 1e-8, eq.UPPER - 1e-8),
        bounds=(eq.LOWER, eq.UPPER),
        max_nfev=budget,
        ftol=1e-10,
        xtol=1e-10,
        gtol=1e-10,
    )
    error = eq.response(result.x, rate=rate) - target
    return result.x, {
        "mse_db2": float(np.mean(error**2)),
        "max_db": float(np.max(np.abs(error))),
        "solver_success": bool(result.success),
        "residual_calls": calls,
    }


def score(model, split, budget):
    raw_mse, predictions = eq.evaluate(model, split)
    rows = [
        refine(p, rate, target, budget)[1]
        for p, rate, target in zip(predictions, split[1], split[2])
    ]
    qualified = sum(
        row["solver_success"] and row["mse_db2"] <= 0.01 and row["max_db"] <= 0.5
        for row in rows
    )
    return {
        "raw_mse_db2": raw_mse,
        "sampled_solver_qualified": qualified,
        "mean_refined_mse_db2": float(np.mean([r["mse_db2"] for r in rows])),
        "median_residual_calls": float(np.median([r["residual_calls"] for r in rows])),
        "rows": rows,
    }


def selection_key(metrics):
    return (
        -metrics["sampled_solver_qualified"],
        metrics["mean_refined_mse_db2"],
        metrics["median_residual_calls"],
    )


def teacher_labels(model, split, budget):
    _, initial = eq.evaluate(model, split)
    labels, rows = [], []
    for p, rate, target in zip(initial, split[1], split[2]):
        refined, row = refine(p, rate, target, budget)
        before = float(np.mean((eq.response(p, rate=rate) - target) ** 2))
        valid = bool(
            np.isfinite(refined).all()
            and np.all(refined >= eq.LOWER)
            and np.all(refined <= eq.UPPER)
            and row["mse_db2"] < before
        )
        labels.append(refined if valid else p)
        rows.append(dict(row, initial_mse_db2=before, improved_label=valid))
    return np.asarray(labels), rows


def label_mask(parameters):
    mask = np.ones_like(parameters)
    for slot in range(4):
        active = np.abs(parameters[:, 3 * slot + 2]) >= 0.01
        mask[:, 3 * slot : 3 * slot + 2] = active[:, None]
    for slot in range(2):
        mask[:, 12 + 2 * slot] = np.abs(parameters[:, 13 + 2 * slot]) >= 0.01
    return mask


def fit_arm(initial, split, development, labels, output, args):
    output.mkdir()
    model = copy.deepcopy(initial)
    optimizer = torch.optim.Adam(model.parameters(), lr=args.learning_rate)
    x = torch.as_tensor(eq.inputs(split[2], split[1]), dtype=torch.float32)
    y, rates = torch.as_tensor(split[2]), torch.as_tensor(split[1])
    normalized_labels = torch.as_tensor(
        2 * (labels - eq.LOWER) / (eq.UPPER - eq.LOWER) - 1, dtype=torch.float32
    )
    mask = torch.as_tensor(label_mask(labels), dtype=torch.float32)
    generator = torch.Generator().manual_seed(args.seed + 3)
    best_metrics = score(model, development, args.development_budget)
    best, best_state, best_epoch = (
        selection_key(best_metrics),
        copy.deepcopy(model.state_dict()),
        0,
    )
    history = [{"epoch": 0, "development": best_metrics}]
    for epoch in range(1, args.epochs + 1):
        model.train()
        total = 0.0
        for batch in torch.randperm(len(x), generator=generator).split(32):
            optimizer.zero_grad()
            prediction = model(x[batch])
            response_error = (
                eq.differentiable_response(eq.physical(prediction), rates[batch])
                - y[batch]
            )
            loss = (
                response_error.square().mean()
                + 0.1
                * (
                    mask[batch] * (prediction - normalized_labels[batch]).square()
                ).mean()
            )
            if not torch.isfinite(loss):
                raise ValueError("Non-finite fine-tuning loss")
            loss.backward()
            torch.nn.utils.clip_grad_norm_(model.parameters(), 10)
            optimizer.step()
            total += loss.item() * len(batch)
        model.eval()
        metrics = score(model, development, args.development_budget)
        row = {"epoch": epoch, "training_loss": total / len(x), "development": metrics}
        history.append(row)
        print(
            json.dumps(
                {
                    "arm": output.name,
                    "epoch": epoch,
                    "loss": row["training_loss"],
                    "qualified": metrics["sampled_solver_qualified"],
                    "mse": metrics["mean_refined_mse_db2"],
                }
            ),
            flush=True,
        )
        if selection_key(metrics) < best:
            best, best_metrics, best_epoch = selection_key(metrics), metrics, epoch
            best_state = copy.deepcopy(model.state_dict())
    model.load_state_dict(best_state)
    portable = {
        "contract": eq.CONTRACT,
        "weights": {k: v.tolist() for k, v in best_state.items()},
    }
    (output / "weights.json").write_text(json.dumps(portable) + "\n")
    report = {
        "weights_sha256": eq.sha(output / "weights.json"),
        "best_epoch": best_epoch,
        "best_development": best_metrics,
        "history": history,
        "arm": output.name,
        "selection": "max sampled solver qualification; min refined MSE; min residual calls; initial model eligible",
        "qualification": "Development selection proxy; full native export qualification remains mandatory",
    }
    (output / "training-report.json").write_text(json.dumps(report, indent=2) + "\n")
    return report


def collect_ids(document):
    result = set()
    if isinstance(document, dict):
        for key, value in document.items():
            if key in (
                "split_ids",
                "fresh_holdout_ids",
                "unique_parameter_identities",
                "parameter_identity",
            ):

                def strings(item):
                    if isinstance(item, str):
                        result.add(item)
                    elif isinstance(item, dict):
                        for child in item.values():
                            strings(child)
                    elif isinstance(item, list):
                        for child in item:
                            strings(child)

                strings(value)
            else:
                result.update(collect_ids(value))
    elif isinstance(document, list):
        for value in document:
            result.update(collect_ids(value))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--weights", type=Path, required=True)
    parser.add_argument("--weights-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--exclude", type=Path, action="append", required=True)
    parser.add_argument("--examples", type=int, default=256)
    parser.add_argument("--development", type=int, default=12)
    parser.add_argument("--seed", type=int, default=20270125)
    parser.add_argument("--epochs", type=int, default=12)
    parser.add_argument("--teacher-budget", type=int, default=40)
    parser.add_argument("--development-budget", type=int, default=40)
    parser.add_argument("--learning-rate", type=float, default=0.0001)
    args = parser.parse_args()
    if (
        min(
            args.examples,
            args.development,
            args.epochs,
            args.teacher_budget,
            args.development_budget,
        )
        < 1
    ):
        raise ValueError("Counts and budgets must be positive")
    if not np.isfinite(args.learning_rate) or not 0 < args.learning_rate <= 0.01:
        raise ValueError("Invalid learning rate")
    if eq.sha(args.weights) != args.weights_sha256:
        raise ValueError("Explicit base model checksum mismatch")
    initial, digest = load_model(args.weights, eq.CONTRACT, eq.network)
    if digest != args.weights_sha256:
        raise ValueError("Base model changed while loading")
    args.output.mkdir(parents=True, exist_ok=False)
    torch.set_num_threads(2)
    train, development = (
        eq.dataset(args.examples, args.seed),
        eq.dataset(args.development, args.seed + 1),
    )
    excluded = set()
    for path in args.exclude:
        excluded.update(collect_ids(json.loads(path.read_text())))
    if (
        set(train[3]) & (set(development[3]) | excluded)
        or set(development[3]) & excluded
    ):
        raise ValueError("Training/development identity overlap")
    manifest = {
        "base_weights_sha256": digest,
        "split_ids": {"train": train[3], "development": development[3]},
        "excluded_ids": sorted(excluded),
        "arguments": {
            k: str(v) if isinstance(v, Path) else v
            for k, v in vars(args).items()
            if k != "exclude"
        },
        "exclusion_sources": {str(p): eq.sha(p) for p in args.exclude},
        "selection": "Development fixed-budget refinement only; no final holdout generation or scoring in training",
        "data": "New synthetic native EQ curves only; no recordings",
        "license": "GPL-3.0-or-later",
        "torch": torch.__version__,
        "numpy": np.__version__,
    }
    (args.output / "data-manifest.json").write_text(
        json.dumps(manifest, indent=2) + "\n"
    )
    for name in ("train_eq_refined.py", "eq_multiband.py", "eq_model_loading.py"):
        (args.output / name).write_bytes(Path(__file__).with_name(name).read_bytes())
    start = time.monotonic()
    labels, rows = teacher_labels(initial, train, args.teacher_budget)
    np.savez(
        args.output / "teacher-labels.npz",
        parameters=labels,
        rates=train[1],
        targets=train[2],
    )
    (args.output / "teacher-report.json").write_text(json.dumps(rows, indent=2) + "\n")
    for name, supervision in (
        ("original-labels", train[0]),
        ("refined-labels", labels),
    ):
        fit_arm(initial, train, development, supervision, args.output / name, args)
    (args.output / "elapsed.json").write_text(
        json.dumps({"seconds": time.monotonic() - start}) + "\n"
    )


if __name__ == "__main__":
    main()
