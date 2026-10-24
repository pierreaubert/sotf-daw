#!/usr/bin/env python3
"""Offline learned single-peak EQ proposals with exact refinement and rejection.

This is an experimental initializer, not a runtime filter or an arbitrary-curve
matcher. Training uses synthetic, disjoint parameter identities. The validation
split selects checkpoints; the test split is evaluated only after selection.
"""

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
from scipy.signal import freqz
from torch import nn

GRID = np.geomspace(20.0, 20_000.0, 128)
LOWER = np.array([math.log(50.0), math.log(0.35), -12.0])
UPPER = np.array([math.log(10_000.0), math.log(8.0), 12.0])
RATE = 48_000.0


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def response(encoded, frequencies=GRID, rate=RATE):
    """Exact RBJ peaking response; coordinates are log(f), log(Q), gain dB."""
    p = np.atleast_2d(encoded)
    frequency, q, gain = np.exp(p[:, 0]), np.exp(p[:, 1]), p[:, 2]
    a = 10.0 ** (gain / 40.0)
    omega = 2.0 * np.pi * frequency / rate
    alpha = np.sin(omega) / (2.0 * q)
    cosine = np.cos(omega)
    z = np.exp(-2j * np.pi * np.asarray(frequencies) / rate)[None, :]
    b0, b1, b2 = 1 + alpha * a, -2 * cosine, 1 - alpha * a
    a0, a1, a2 = 1 + alpha / a, -2 * cosine, 1 - alpha / a
    numerator = b0[:, None] + b1[:, None] * z + b2[:, None] * z * z
    denominator = a0[:, None] + a1[:, None] * z + a2[:, None] * z * z
    return 20 * np.log10(np.maximum(np.abs(numerator / denominator), 1e-15))


def differentiable_response(encoded):
    """Float64 response loss avoids low-frequency coefficient cancellation."""
    p = encoded.double()
    frequency, q, gain = p[:, 0].exp(), p[:, 1].exp(), p[:, 2]
    a = torch.pow(10.0, gain / 40.0)
    omega = 2 * math.pi * frequency / RATE
    alpha, cosine = omega.sin() / (2 * q), omega.cos()
    z = torch.exp(-2j * math.pi * torch.as_tensor(GRID) / RATE)[None, :]
    numerator = (
        (1 + alpha * a)[:, None]
        - 2 * cosine[:, None] * z
        + (1 - alpha * a)[:, None] * z * z
    )
    denominator = (
        (1 + alpha / a)[:, None]
        - 2 * cosine[:, None] * z
        + (1 - alpha / a)[:, None] * z * z
    )
    return 20 * torch.log10((numerator / denominator).abs().clamp(min=1e-15))


def dataset(count, seed):
    rng = np.random.default_rng(seed)
    p = rng.uniform(LOWER, UPPER, size=(count, 3))
    p[:, 2] = rng.choice([-1.0, 1.0], count) * rng.uniform(1.0, 12.0, count)
    ids = [hashlib.sha256(row.astype("<f8").tobytes()).hexdigest() for row in p]
    return p, response(p), ids


def network():
    return nn.Sequential(
        nn.Linear(128, 128),
        nn.SiLU(),
        nn.Linear(128, 64),
        nn.SiLU(),
        nn.Linear(64, 3),
        nn.Tanh(),
    )


def physical(prediction):
    lo = torch.as_tensor(LOWER, dtype=prediction.dtype)
    hi = torch.as_tensor(UPPER, dtype=prediction.dtype)
    return lo + (prediction + 1) * 0.5 * (hi - lo)


def train(args):
    torch.set_num_threads(2)
    torch.manual_seed(20261024)
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / "recipe.py").write_bytes(Path(__file__).read_bytes())
    splits = {
        name: dataset(count, seed)
        for name, count, seed in [
            ("train", args.examples, args.split_seed + 301),
            ("validation", args.validation, args.split_seed + 701),
            ("test", args.test, args.split_seed + 1103),
        ]
    }
    sets = {name: set(data[2]) for name, data in splits.items()}
    assert not sets["train"] & sets["validation"]
    assert not sets["train"] & sets["test"]
    assert not sets["validation"] & sets["test"]
    manifest = {
        "family": "single RBJ peaking EQ",
        "sample_rate": RATE,
        "grid_hz": GRID.tolist(),
        "data": "synthetic exact filter responses; no external recordings",
        "license": "GPL-3.0-or-later",
        "split_ids": {n: d[2] for n, d in splits.items()},
        "script_sha256": sha(__file__),
        "torch": torch.__version__,
        "seed": 20261024,
        "epochs": args.epochs,
        "learning_rate": 0.001,
        "split_seed": args.split_seed,
        "loss": "dB response MSE + 0.1 normalized parameter MSE",
        "acceptance": "experimental initializer; no superiority or native-filter acceptance inferred",
    }
    (args.output / "data-manifest.json").write_text(
        json.dumps(manifest, indent=2) + "\n"
    )
    model = network()
    optimizer = torch.optim.Adam(model.parameters(), lr=0.001)
    training = torch.as_tensor(splits["train"][1] / 12, dtype=torch.float32)
    labels = torch.as_tensor(
        2 * (splits["train"][0] - LOWER) / (UPPER - LOWER) - 1, dtype=torch.float32
    )
    validation = torch.as_tensor(splits["validation"][1] / 12, dtype=torch.float32)
    target_validation = torch.as_tensor(splits["validation"][1])
    best, best_state, history = float("inf"), None, []
    start = time.monotonic()
    for epoch in range(args.epochs):
        model.train()
        order = torch.randperm(len(training))
        total = 0.0
        for batch in order.split(256):
            optimizer.zero_grad()
            predicted = model(training[batch])
            error = (
                differentiable_response(physical(predicted))
                - 12 * training[batch].double()
            )
            loss = (error**2).mean() + 0.1 * ((predicted - labels[batch]) ** 2).mean()
            if not torch.isfinite(loss):
                raise RuntimeError("Non-finite training loss")
            loss.backward()
            torch.nn.utils.clip_grad_norm_(model.parameters(), 10.0)
            optimizer.step()
            total += loss.item() * len(batch)
        model.eval()
        with torch.no_grad():
            mse = (
                (
                    (
                        differentiable_response(physical(model(validation)))
                        - target_validation
                    )
                    ** 2
                )
                .mean()
                .item()
            )
        history.append(
            {
                "epoch": epoch + 1,
                "train_loss": total / len(training),
                "validation_db_mse": mse,
            }
        )
        if mse < best:
            best, best_state = mse, copy.deepcopy(model.state_dict())
            torch.save(best_state, args.output / "best.pt")
        if epoch == 0 or (epoch + 1) % 10 == 0:
            print(json.dumps(history[-1]), flush=True)
        if epoch >= 40 and min(x["validation_db_mse"] for x in history[-20:]) > best:
            break
    # Reload the actual serialized checkpoint before touching the sealed test split.
    model.load_state_dict(torch.load(args.output / "best.pt", weights_only=True))
    model.eval()
    with torch.no_grad():
        proposed = physical(
            model(torch.as_tensor(splits["test"][1] / 12, dtype=torch.float32))
        ).numpy()
    residual = response(proposed) - splits["test"][1]
    flat_mse = float(np.mean(splits["test"][1] ** 2))
    test_mse = float(np.mean(residual**2))
    report = {
        "checkpoint_sha256": sha(args.output / "best.pt"),
        "best_validation_db_mse": best,
        "test_response_db_mse": test_mse,
        "flat_baseline_db_mse": flat_mse,
        "test_explained_response_fraction": 1 - test_mse / flat_mse,
        "elapsed_seconds": time.monotonic() - start,
        "history": history,
        "test_evaluation": "once after checkpoint selection; synthetic in-family only",
        "requirements_remaining": [
            "native realization comparison",
            "classical heuristic/multistart cost comparison",
            "dense/feature-point qualification",
            "shift/zero-gain/out-of-family rejection",
        ],
    }
    # Separate portable JSON weights make offline inference independent of pickle files.
    weights = {name: value.tolist() for name, value in model.state_dict().items()}
    (args.output / "weights.json").write_text(
        json.dumps({"contract": "sotf-single-peak-v1", "weights": weights}) + "\n"
    )
    report["weights_sha256"] = sha(args.output / "weights.json")
    report["data_manifest_sha256"] = sha(args.output / "data-manifest.json")
    (args.output / "training-report.json").write_text(
        json.dumps(report, indent=2) + "\n"
    )
    print(json.dumps({k: v for k, v in report.items() if k != "history"}, indent=2))


def fit(args):
    start = time.monotonic()
    data = json.loads(args.target.read_text())
    frequencies = np.asarray(data["frequencies_hz"], dtype=np.float64)
    target = np.asarray(data["target_db"], dtype=np.float64)
    rate = float(data["sample_rate"])
    if frequencies.ndim != 1 or frequencies.shape != target.shape or len(target) < 16:
        raise ValueError(
            "Target needs matching one-dimensional frequency and dB arrays"
        )
    if (
        not np.all(np.isfinite(frequencies))
        or not np.all(np.isfinite(target))
        or not np.isfinite(rate)
    ):
        raise ValueError("Non-finite target")
    if (
        rate < 44_100
        or np.any(np.diff(frequencies) <= 0)
        or frequencies[0] > GRID[0]
        or frequencies[-1] < GRID[-1]
    ):
        raise ValueError("Target must cover 20 Hz–20 kHz monotonically at >=44.1 kHz")
    if (
        frequencies[0] <= 0
        or frequencies[-1] >= rate / 2
        or np.max(np.abs(target)) > 12.0
    ):
        raise ValueError("Target exceeds supported frequency/gain bounds")
    if np.max(np.abs(target)) < 0.01:
        result = {
            "accepted": True,
            "method": "canonical-flat",
            "parameters": {"filters": [], "auto_gain": {"enabled": False}},
        }
    else:
        training_report = json.loads(
            (args.weights.parent / "training-report.json").read_text()
        )
        if sha(args.weights) != training_report["weights_sha256"]:
            raise ValueError("EQ model weights SHA-256 mismatch")
        raw = json.loads(args.weights.read_text())
        if raw["contract"] != "sotf-single-peak-v1":
            raise ValueError("Unknown EQ model contract")
        model = network()
        model.load_state_dict(
            {n: torch.as_tensor(v) for n, v in raw["weights"].items()}, strict=True
        )
        model.eval()
        interpolated = np.interp(np.log(GRID), np.log(frequencies), target)
        with torch.no_grad():
            proposal = physical(
                model(torch.as_tensor(interpolated[None, :] / 12, dtype=torch.float32))
            ).numpy()[0]
        peak = int(np.argmax(np.abs(target)))
        classical = np.array(
            [np.log(np.clip(frequencies[peak], 50, 10000)), np.log(1.0), target[peak]]
        )
        candidates = []
        for method, initial in [
            ("neural", proposal),
            ("classical-fallback", classical),
        ]:
            solved = least_squares(
                lambda p: response(p, frequencies, rate)[0] - target,
                np.clip(initial, LOWER + 1e-8, UPPER - 1e-8),
                bounds=(LOWER, UPPER),
                max_nfev=100,
                ftol=1e-10,
                xtol=1e-10,
                gtol=1e-10,
            )
            error = response(solved.x, frequencies, rate)[0] - target
            candidates.append(
                (float(np.mean(error**2)), method, solved, float(np.max(np.abs(error))))
            )
            if (
                solved.success
                and np.sqrt(candidates[-1][0]) <= 0.1
                and candidates[-1][3] <= 0.5
            ):
                break
        mse, method, solved, maximum = min(candidates, key=lambda c: c[0])
        frequency, q, gain = np.exp(solved.x[0]), np.exp(solved.x[1]), solved.x[2]
        # Independently evaluate coefficients through scipy.signal.freqz.
        a = 10 ** (gain / 40)
        omega = 2 * np.pi * frequency / rate
        alpha = np.sin(omega) / (2 * q)
        numerator = [1 + alpha * a, -2 * np.cos(omega), 1 - alpha * a]
        denominator = [1 + alpha / a, -2 * np.cos(omega), 1 - alpha / a]
        _, h = freqz(numerator, denominator, worN=frequencies, fs=rate)
        independent = 20 * np.log10(np.maximum(np.abs(h), 1e-15))
        stable = bool(np.all(np.abs(np.roots(denominator)) < 1))
        accepted = bool(
            stable
            and solved.success
            and np.sqrt(mse) <= 0.1
            and maximum <= 0.5
            and np.max(np.abs(independent - response(solved.x, frequencies, rate)[0]))
            < 1e-7
        )
        result = {
            "accepted": accepted,
            "method": method,
            "sampled_rms_db": float(np.sqrt(mse)),
            "sampled_max_db": maximum,
            "poles_stable": stable,
            "evaluations": int(solved.nfev),
            "qualification": "supplied frequency samples only; no continuous guarantee",
            "parameters": {
                "filters": [
                    {
                        "filter_type": "peak",
                        "freq": float(frequency),
                        "q": float(q),
                        "db_gain": float(gain),
                        "order": 2,
                    }
                ],
                "auto_gain": {"enabled": False},
            }
            if accepted
            else None,
        }
    result["elapsed_seconds"] = time.monotonic() - start
    result["target_sha256"] = sha(args.target)
    result["weights_sha256"] = sha(args.weights)
    args.output.write_text(json.dumps(result, indent=2, allow_nan=False) + "\n")
    print(json.dumps(result, indent=2, allow_nan=False))
    if not result["accepted"]:
        raise SystemExit(2)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    training = sub.add_parser("train")
    training.add_argument("output", type=Path)
    training.add_argument("--examples", type=int, default=12000)
    training.add_argument("--validation", type=int, default=1500)
    training.add_argument("--test", type=int, default=1500)
    training.add_argument("--epochs", type=int, default=150)
    training.add_argument("--split-seed", type=int, default=20261025)
    fitting = sub.add_parser("fit")
    fitting.add_argument("weights", type=Path)
    fitting.add_argument("target", type=Path)
    fitting.add_argument("output", type=Path)
    args = parser.parse_args()
    if args.command == "train":
        if min(args.examples, args.validation, args.test, args.epochs) < 1:
            parser.error("Dataset sizes and epoch count must be positive")
        train(args)
    else:
        fit(args)


if __name__ == "__main__":
    main()
