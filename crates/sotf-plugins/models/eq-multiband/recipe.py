#!/usr/bin/env python3
"""Offline learned proposals for four peaking bands and two native shelf filters.

The network initializes an exact bounded optimizer. It never realizes filters or
applies audio gain. Synthetic train/development/test identities are disjoint;
checkpoint selection uses development data only. The prior single-peak model is
kept intact because this model has different input and output contracts.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import math
import time
from pathlib import Path

import numpy as np
import torch
from scipy.optimize import least_squares
from scipy.signal import sosfreqz
from torch import nn

GRID = np.unique(
    np.concatenate([np.geomspace(20, 20000, 256), np.linspace(8000, 20000, 256)])
)
LOWER = np.array([math.log(20), math.log(0.35), -12] * 4 + [math.log(20), -12] * 2)
UPPER = np.array([math.log(20000), math.log(8), 12] * 4 + [math.log(20000), 12] * 2)
CONTRACT = "sotf-four-peak-two-shelf-v1"
TRAIN_RATES = [44100.0, 48000.0, 96000.0]


def sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def coefficients(parameters, rate, tensor=False):
    """Native RBJ peaks and S=1 shelves; encoded frequencies/Q are logarithmic."""
    xp = torch if tensor else np
    stack = lambda values: (
        xp.stack(values, dim=-1) if tensor else np.stack(values, axis=-1)
    )
    cat = lambda values, axis: (
        torch.cat(values, dim=axis) if tensor else np.concatenate(values, axis=axis)
    )
    peaks = parameters[..., :12].reshape(*parameters.shape[:-1], 4, 3)
    shelves = parameters[..., 12:].reshape(*parameters.shape[:-1], 2, 2)
    frequency = cat([xp.exp(peaks[..., 0]), xp.exp(shelves[..., 0])], -1)
    gains = cat([peaks[..., 2], shelves[..., 1]], -1)
    q = xp.exp(peaks[..., 1])
    omega = 2 * math.pi * frequency / rate[..., None]
    sn, cs = xp.sin(omega), xp.cos(omega)
    a = 10 ** (gains / 40)
    alpha = sn[..., :4] / (2 * q)
    ap, cp = a[..., :4], cs[..., :4]
    peak = stack(
        [
            1 + alpha * ap,
            -2 * cp,
            1 - alpha * ap,
            1 + alpha / ap,
            -2 * cp,
            1 - alpha / ap,
        ]
    )
    rows = [peak]
    for index, low in [(-2, True), (-1, False)]:
        v, c, sine = a[..., index], cs[..., index], sn[..., index]
        beta = xp.sqrt(2 * v) * sine
        if low:
            row = stack(
                [
                    v * ((v + 1) - (v - 1) * c + beta),
                    2 * v * ((v - 1) - (v + 1) * c),
                    v * ((v + 1) - (v - 1) * c - beta),
                    (v + 1) + (v - 1) * c + beta,
                    -2 * ((v - 1) + (v + 1) * c),
                    (v + 1) + (v - 1) * c - beta,
                ]
            )
        else:
            row = stack(
                [
                    v * ((v + 1) + (v - 1) * c + beta),
                    -2 * v * ((v - 1) + (v + 1) * c),
                    v * ((v + 1) + (v - 1) * c - beta),
                    (v + 1) - (v - 1) * c + beta,
                    2 * ((v - 1) - (v + 1) * c),
                    (v + 1) - (v - 1) * c - beta,
                ]
            )
        rows.append(row[..., None, :])
    return cat(rows, -2)


def response(parameters, frequencies=GRID, rate=48000.0):
    p = np.asarray(parameters, dtype=np.float64)
    rate = np.asarray(rate, dtype=np.float64)
    c = coefficients(p, rate)
    z = np.exp(-2j * np.pi * np.asarray(frequencies) / rate[..., None])[..., None, :]
    numerator = c[..., 0, None] + c[..., 1, None] * z + c[..., 2, None] * z * z
    denominator = c[..., 3, None] + c[..., 4, None] * z + c[..., 5, None] * z * z
    return (20 * np.log10(np.maximum(np.abs(numerator / denominator), 1e-15))).sum(
        axis=-2
    )


def differentiable_response(parameters, rate):
    p, rate = parameters.double(), rate.double()
    c = coefficients(p, rate, tensor=True)
    frequencies = torch.as_tensor(GRID, dtype=torch.float64)
    z = torch.exp(-2j * math.pi * frequencies / rate[..., None])[..., None, :]
    numerator = c[..., 0, None] + c[..., 1, None] * z + c[..., 2, None] * z * z
    denominator = c[..., 3, None] + c[..., 4, None] * z + c[..., 5, None] * z * z
    return (20 * torch.log10((numerator / denominator).abs().clamp(min=1e-15))).sum(
        dim=-2
    )


def dataset(count, seed):
    """Generate effective filter identities; unused bands have canonical values."""
    rng = np.random.default_rng(seed)
    parameters, rates, targets, identities = [], [], [], []
    while len(parameters) < count:
        p = rng.uniform(LOWER, UPPER)
        peaks = p[:12].reshape(4, 3)
        active = int(rng.integers(0, 5))
        peaks[:active, 0] = np.sort(peaks[:active, 0])
        peaks[:active, 2] = rng.choice([-1, 1], active) * rng.uniform(1, 9, active)
        peaks[active:] = [math.log(20000), 0, 0]
        shelves = p[12:].reshape(2, 2)
        for shelf in shelves:
            if rng.random() < 0.3:
                shelf[:] = [math.log(1000), 0]
            else:
                shelf[1] = rng.choice([-1, 1]) * rng.uniform(1, 9)
        if not np.any(p[[2, 5, 8, 11, 13, 15]]):
            continue  # Canonical flat is a separate deterministic no-op test.
        rate = rng.choice(TRAIN_RATES)
        target = response(p, rate=rate)
        if np.max(np.abs(target)) > 24:
            continue
        identity = hashlib.sha256(p.astype("<f8").tobytes()).hexdigest()
        parameters.append(p)
        rates.append(rate)
        targets.append(target)
        identities.append(identity)
    return np.asarray(parameters), np.asarray(rates), np.asarray(targets), identities


def network():
    return nn.Sequential(
        nn.Linear(len(GRID) + 1, 256),
        nn.SiLU(),
        nn.Linear(256, 128),
        nn.SiLU(),
        nn.Linear(128, 16),
        nn.Tanh(),
    )


def physical(prediction):
    lo = torch.as_tensor(LOWER, dtype=prediction.dtype)
    hi = torch.as_tensor(UPPER, dtype=prediction.dtype)
    return lo + (prediction + 1) * (hi - lo) / 2


def inputs(target, rates):
    return np.concatenate(
        [target / 24, np.log(np.asarray(rates)[..., None] / 48000)], axis=-1
    )


def evaluate(model, split):
    predicted = []
    with torch.no_grad():
        x = torch.as_tensor(inputs(split[2], split[1]), dtype=torch.float32)
        for batch in x.split(128):
            predicted.append(physical(model(batch)).numpy())
    p = np.concatenate(predicted)
    residual = response(p, rate=split[1]) - split[2]
    return float(np.mean(residual**2)), p


def train(args):
    torch.set_num_threads(args.threads)
    torch.manual_seed(20261025)
    if args.output.exists() and any(args.output.iterdir()):
        raise ValueError(
            "Use an empty output directory; preserve previous training artifacts"
        )
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / "recipe.py").write_bytes(Path(__file__).read_bytes())
    splits = {
        name: dataset(count, args.seed + offset)
        for name, count, offset in [
            ("train", args.examples, 301),
            ("development", args.development, 701),
            ("test", args.test, 1103),
        ]
    }
    ids = {name: set(split[3]) for name, split in splits.items()}
    if any(
        ids[a] & ids[b]
        for a, b in [
            ("train", "development"),
            ("train", "test"),
            ("development", "test"),
        ]
    ):
        raise ValueError("Split identity overlap")
    manifest = {
        "contract": CONTRACT,
        "grid_hz": GRID.tolist(),
        "training_rates": TRAIN_RATES,
        "split_ids": {name: split[3] for name, split in splits.items()},
        "split_seed": args.seed,
        "weights_seed": 20261025,
        "script_sha256": sha(Path(__file__)),
        "data": "Synthetic native filter responses; no external or held-out recordings",
        "license": "GPL-3.0-or-later",
        "epochs": args.epochs,
        "batch_size": 128,
        "learning_rate": 0.001,
        "selection": "minimum development response MSE; test evaluated once afterwards",
        "torch": torch.__version__,
        "numpy": np.__version__,
    }
    (args.output / "data-manifest.json").write_text(
        json.dumps(manifest, indent=2) + "\n"
    )
    model = network()
    optimizer = torch.optim.Adam(model.parameters(), lr=0.001)
    x = torch.as_tensor(
        inputs(splits["train"][2], splits["train"][1]), dtype=torch.float32
    )
    y = torch.as_tensor(splits["train"][2])
    rates = torch.as_tensor(splits["train"][1])
    labels = torch.as_tensor(
        2 * (splits["train"][0] - LOWER) / (UPPER - LOWER) - 1, dtype=torch.float32
    )
    mask = np.ones_like(splits["train"][0])
    for slot in range(4):
        active = splits["train"][0][:, 3 * slot + 2] != 0
        mask[:, 3 * slot : 3 * slot + 2] = active[:, None]
    for slot in range(2):
        mask[:, 12 + 2 * slot] = splits["train"][0][:, 13 + 2 * slot] != 0
    mask = torch.as_tensor(mask, dtype=torch.float32)
    history, best, best_epoch = [], float("inf"), 0
    start = time.monotonic()
    for epoch in range(args.epochs):
        model.train()
        total = 0.0
        for batch in torch.randperm(len(x)).split(128):
            optimizer.zero_grad()
            normalized = model(x[batch])
            error = (
                differentiable_response(physical(normalized), rates[batch]) - y[batch]
            )
            loss = (
                error.square().mean()
                + 0.1 * (mask[batch] * (normalized - labels[batch]).square()).mean()
            )
            if not torch.isfinite(loss):
                raise ValueError("Non-finite training loss")
            loss.backward()
            torch.nn.utils.clip_grad_norm_(model.parameters(), 10)
            optimizer.step()
            total += loss.item() * len(batch)
        model.eval()
        dev_mse, _ = evaluate(model, splits["development"])
        row = {
            "epoch": epoch + 1,
            "train_loss": total / len(x),
            "development_db_mse": dev_mse,
            "elapsed_seconds": time.monotonic() - start,
        }
        history.append(row)
        with (args.output / "metrics.jsonl").open("a") as stream:
            stream.write(json.dumps(row) + "\n")
        if dev_mse < best:
            best, best_epoch = dev_mse, epoch + 1
            torch.save(copy.deepcopy(model.state_dict()), args.output / "best.pt")
        if epoch == 0 or (epoch + 1) % 5 == 0:
            print(json.dumps(row), flush=True)
        if epoch + 1 - best_epoch >= args.patience:
            break
    model.load_state_dict(torch.load(args.output / "best.pt", weights_only=True))
    model.eval()
    test_mse, _ = evaluate(model, splits["test"])
    flat_mse = float(np.mean(splits["test"][2] ** 2))
    portable = {
        "contract": CONTRACT,
        "weights": {name: value.tolist() for name, value in model.state_dict().items()},
    }
    (args.output / "weights.json").write_text(json.dumps(portable) + "\n")
    report = {
        "checkpoint_sha256": sha(args.output / "best.pt"),
        "weights_sha256": sha(args.output / "weights.json"),
        "data_manifest_sha256": sha(args.output / "data-manifest.json"),
        "best_epoch": best_epoch,
        "best_development_db_mse": best,
        "test_response_db_mse": test_mse,
        "flat_baseline_db_mse": flat_mse,
        "test_explained_response_fraction": 1 - test_mse / flat_mse,
        "history": history,
        "elapsed_seconds": time.monotonic() - start,
        "qualification": "synthetic learned initializer; exact/native/refinement/classical comparisons required",
    }
    (args.output / "training-report.json").write_text(
        json.dumps(report, indent=2) + "\n"
    )
    print(
        json.dumps({key: value for key, value in report.items() if key != "history"}),
        flush=True,
    )


def canonical_parameters():
    return np.array([math.log(20000), 0, 0] * 4 + [math.log(1000), 0] * 2)


def classical_seed(frequencies, target, rate, edge_shelves=True):
    p = canonical_parameters()
    if edge_shelves:
        p[12:] = [
            math.log(150),
            np.clip(np.median(target[:4]), -12, 12),
            math.log(7000),
            np.clip(np.median(target[-4:]), -12, 12),
        ]
    else:
        p[[13, 15]] = np.clip(np.median(target), -12, 12)
    for slot in range(4):
        residual = target - response(p, frequencies, rate)
        peak = int(np.argmax(np.abs(residual)))
        gain = residual[peak]
        if abs(gain) < 0.01:
            break
        signed = np.sign(gain) * residual
        left = np.flatnonzero(signed[:peak] <= abs(gain) / 2)
        right = np.flatnonzero(signed[peak + 1 :] <= abs(gain) / 2)
        q = 1.0
        if len(left) and len(right):
            width = frequencies[peak + 1 + right[0]] - frequencies[left[-1]]
            q = float(np.clip(frequencies[peak] / width, 0.35, 8))
        p[slot * 3 : slot * 3 + 3] = [
            np.log(np.clip(frequencies[peak], 20, 20000)),
            np.log(q),
            np.clip(gain, -12, 12),
        ]
    return p


def validate_target(data):
    frequencies = np.asarray(data["frequencies_hz"], dtype=np.float64)
    target = np.asarray(data["target_db"], dtype=np.float64)
    rate = float(data["sample_rate"])
    if frequencies.ndim != 1 or frequencies.shape != target.shape or len(target) < 64:
        raise ValueError("Supply at least 64 matching frequency/target samples")
    if (
        not np.all(np.isfinite(frequencies))
        or not np.all(np.isfinite(target))
        or not np.isfinite(rate)
    ):
        raise ValueError("Non-finite target")
    if not 44100 <= rate <= 192000 or np.any(np.diff(frequencies) <= 0):
        raise ValueError("Unsupported sample rate or non-monotonic frequencies")
    if (
        frequencies[0] <= 0
        or frequencies[0] > 20
        or frequencies[-1] < 20000
        or frequencies[-1] >= rate / 2
    ):
        raise ValueError("Frequency samples must cover 20 Hz–20 kHz below Nyquist")
    if np.max(np.abs(target)) > 24:
        raise ValueError("Target exceeds the supported +/-24 dB range")
    return frequencies, target, rate


def proposal_parameters(p):
    filters = []
    for slot in range(4):
        f, q, gain = np.exp(p[slot * 3]), np.exp(p[slot * 3 + 1]), p[slot * 3 + 2]
        filters.append(
            {
                "filter_type": "peak",
                "freq": float(f),
                "q": float(q),
                "db_gain": float(gain),
                "order": 2,
            }
        )
    for slot, kind in enumerate(["lowshelf", "highshelf"]):
        filters.append(
            {
                "filter_type": kind,
                "freq": float(np.exp(p[12 + 2 * slot])),
                "q": math.sqrt(0.5),
                "db_gain": float(p[13 + 2 * slot]),
                "order": 2,
            }
        )
    return {"filters": filters, "auto_gain": {"enabled": False}}


def fit(args):
    start = time.monotonic()
    frequencies, target, rate = validate_target(json.loads(args.target.read_text()))
    if np.max(np.abs(target)) < 0.01:
        result = {
            "accepted": True,
            "method": "canonical-flat",
            "parameters": {"filters": [], "auto_gain": {"enabled": False}},
        }
    else:
        load_start = time.monotonic()
        report = json.loads((args.weights.parent / "training-report.json").read_text())
        if sha(args.weights) != report["weights_sha256"]:
            raise ValueError("Multiband weights SHA-256 mismatch")
        raw = json.loads(args.weights.read_text())
        if raw["contract"] != CONTRACT:
            raise ValueError("Unsupported multiband model contract")
        model = network()
        model.load_state_dict(
            {key: torch.as_tensor(value) for key, value in raw["weights"].items()},
            strict=True,
        )
        model.eval()
        load_seconds = time.monotonic() - load_start
        infer_start = time.monotonic()
        interpolated = np.interp(np.log(GRID), np.log(frequencies), target)
        with torch.no_grad():
            neural = physical(
                model(
                    torch.as_tensor(
                        inputs(interpolated, rate)[None], dtype=torch.float32
                    )
                )
            ).numpy()[0]
        inference_seconds = time.monotonic() - infer_start
        candidates = []
        for method, initial in [
            ("neural", neural),
            ("classical-edges", classical_seed(frequencies, target, rate)),
            ("classical-constant", classical_seed(frequencies, target, rate, False)),
        ]:
            calls = 0

            def residual(p):
                nonlocal calls
                calls += 1
                return response(p, frequencies, rate) - target

            then = time.monotonic()
            solved = least_squares(
                residual,
                np.clip(initial, LOWER + 1e-8, UPPER - 1e-8),
                bounds=(LOWER, UPPER),
                max_nfev=args.max_evaluations,
                ftol=1e-10,
                xtol=1e-10,
                gtol=1e-10,
            )
            error = response(solved.x, frequencies, rate) - target
            candidates.append(
                {
                    "method": method,
                    "mse": float(np.mean(error**2)),
                    "maximum_db": float(np.max(np.abs(error))),
                    "residual_calls": calls,
                    "scipy_evaluations": int(solved.nfev),
                    "solver_success": bool(solved.success),
                    "elapsed_seconds": time.monotonic() - then,
                    "parameters": solved.x,
                }
            )
        for candidate in candidates:
            p = candidate["parameters"]
            sections = coefficients(p, np.asarray(rate))
            sos = sections / sections[:, 3, None]
            _, independent_h = sosfreqz(sos, worN=frequencies, fs=rate)
            independent = 20 * np.log10(np.maximum(np.abs(independent_h), 1e-15))
            independent_error = float(
                np.max(np.abs(independent - response(p, frequencies, rate)))
            )
            stable = bool(
                all(np.all(np.abs(np.roots(row[3:])) < 1) for row in sections)
            )
            centers = np.exp(np.r_[p[:12:3], p[12::2]])
            dense = np.unique(
                np.r_[
                    np.geomspace(20, 20000, 8193),
                    frequencies,
                    np.clip(centers[:, None] * [0.99, 1, 1.01], 20, 20000).ravel(),
                ]
            )
            dense_peak = float(np.max(np.abs(response(p, dense, rate))))
            accepted = bool(
                candidate["solver_success"]
                and stable
                and math.sqrt(candidate["mse"]) <= 0.1
                and candidate["maximum_db"] <= 0.5
                and independent_error < 1e-6
                and dense_peak <= 24
            )
            candidate.update(
                accepted=accepted,
                poles_stable=stable,
                independent_scipy_max_error_db=independent_error,
                sampled_dense_response_peak_db=dense_peak,
            )
        best = min(candidates, key=lambda row: (not row["accepted"], row["mse"]))
        p = best["parameters"]
        accepted = best["accepted"]
        stable = best["poles_stable"]
        independent_error = best["independent_scipy_max_error_db"]
        dense_peak = best["sampled_dense_response_peak_db"]
        result = {
            "accepted": accepted,
            "method": best["method"],
            "sample_rate": rate,
            "sampled_rms_db": math.sqrt(best["mse"]),
            "sampled_max_db": best["maximum_db"],
            "sampled_dense_response_peak_db": dense_peak,
            "independent_scipy_max_error_db": independent_error,
            "poles_stable": stable,
            "model_load_seconds": load_seconds,
            "inference_seconds": inference_seconds,
            "candidates": [
                {key: value for key, value in row.items() if key != "parameters"}
                for row in candidates
            ],
            "parameters": proposal_parameters(p) if accepted else None,
            "weights_sha256": sha(args.weights),
            "qualification": "Supplied-sample accuracy and sampled dense gain guard; no continuous response, quality, calibration, or speed advantage guarantee",
            "model_sample_rate_shift": rate not in TRAIN_RATES,
        }
    result.update(
        elapsed_seconds=time.monotonic() - start, target_sha256=sha(args.target)
    )
    args.output.write_text(json.dumps(result, indent=2, allow_nan=False) + "\n")
    print(json.dumps(result, indent=2, allow_nan=False))
    if not result["accepted"]:
        raise SystemExit(2)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    training = sub.add_parser("train")
    training.add_argument("--output", type=Path, required=True)
    training.add_argument("--examples", type=int, default=8192)
    training.add_argument("--development", type=int, default=1024)
    training.add_argument("--test", type=int, default=1024)
    training.add_argument("--epochs", type=int, default=100)
    training.add_argument("--patience", type=int, default=20)
    training.add_argument("--threads", type=int, default=2)
    training.add_argument("--seed", type=int, default=20261024)
    fitting = sub.add_parser("fit")
    fitting.add_argument("--weights", type=Path, required=True)
    fitting.add_argument("--target", type=Path, required=True)
    fitting.add_argument("--output", type=Path, required=True)
    fitting.add_argument("--max-evaluations", type=int, default=200)
    args = parser.parse_args()
    if args.command == "train":
        if (
            min(
                args.examples,
                args.development,
                args.test,
                args.epochs,
                args.patience,
                args.threads,
            )
            <= 0
        ):
            parser.error(
                "Training counts, epochs, patience, and threads must be positive"
            )
        train(args)
    else:
        if not 1 <= args.max_evaluations <= 1000:
            parser.error("max-evaluations must be 1..1000")
        torch.set_num_threads(2)
        fit(args)


if __name__ == "__main__":
    main()
