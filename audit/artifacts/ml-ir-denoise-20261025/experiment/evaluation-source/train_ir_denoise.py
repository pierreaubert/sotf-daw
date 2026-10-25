"""Train an offline IR tail mask on generated, independently split room/noise scenes.

No measured external IR, recording or pretrained weight enters training. The
final seeds are evaluated only after development selection and weight freezing.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from scipy.signal import lfilter

from ir_denoise import (
    FEATURES,
    FORMAT,
    HIDDEN,
    analyze,
    load_model,
    neural_gains,
    propose,
    sha,
)


def fixture(seed, rate, family="training"):
    rng = np.random.default_rng(seed)
    duration = 1.5 if family == "training" else 2.0
    count = round(duration * rate)
    channels = 1 + seed % 2
    arrival, prefix = round(0.08 * rate), round(0.06 * rate)
    t = np.maximum(np.arange(count) - arrival, 0) / rate
    rt60 = float(rng.uniform(0.15, 1.1))
    tail = rng.normal(size=(count, channels))
    tail = lfilter([1], [1, -rng.uniform(0.1, 0.8)], tail, axis=0)
    tail *= np.exp(-np.log(1000) * t / rt60)[:, None]
    tail[:arrival] = 0
    tail *= np.sqrt(0.18 / max(float(np.sum(tail**2)), 1e-30))
    clean = tail
    clean[arrival] += rng.uniform(0.5, 0.9, size=channels)
    for offset in rng.uniform(0.003, 0.045, 7):
        clean[arrival + round(offset * rate)] += rng.normal(0, 0.08, size=channels)
    if family == "late_reflection":
        clean[arrival + round(0.8 * rate)] += 0.12
    elif family == "two_slopes":
        extra = rng.normal(size=clean.shape) * np.exp(-np.log(1000) * t / 1.8)[:, None]
        extra[:arrival] = 0
        clean += extra * np.sqrt(0.05 / np.sum(extra**2))
    elif family == "ringing":
        ringing = np.sin(2 * np.pi * 180 * t) * np.exp(-t * 8)
        ringing[:arrival] = 0
        clean += (ringing * np.sqrt(0.1 / np.sum(ringing**2)))[:, None]
    snr = float(rng.uniform(22, 48) if family == "training" else rng.uniform(14, 44))
    noise = rng.normal(size=clean.shape)
    if family == "colored_noise":
        noise = lfilter([1], [1, -0.95], noise, axis=0)
    noise *= np.sqrt(np.sum(clean**2) / (10 ** (snr / 10) * np.sum(noise**2)))
    if family == "noise_step":
        noise[count // 2 :] *= 4
    noisy = (clean + noise).astype("<f4")
    return (
        clean.astype("<f4"),
        noisy,
        {
            "seed": seed,
            "rate": rate,
            "family": family,
            "frames": count,
            "channels": channels,
            "arrival": arrival,
            "noise_frames": prefix,
            "rt60_parameter_seconds": rt60,
            "nominal_snr_db": snr,
        },
    )


def training_rows(clean, noisy, metadata):
    features, starts, hop, _, _ = analyze(
        noisy, metadata["rate"], metadata["arrival"], metadata["noise_frames"]
    )
    labels, weights = [], []
    noise = float(np.mean(noisy[: metadata["noise_frames"]].astype(float) ** 2))
    for start in starts:
        observed = noisy[start : start + hop].astype(float)
        target = clean[start : start + hop].astype(float)
        energy = float(np.sum(observed**2))
        labels.append(np.clip(np.sum(target * observed) / max(energy, 1e-30), 0.05, 1))
        weights.append(np.clip(energy / max(noise * observed.size, 1e-30), 1, 100))
    selected = starts >= metadata["arrival"] + round(metadata["rate"] * 0.05)
    return features[selected], np.array(labels)[selected], np.array(weights)[selected]


def metrics(clean, noisy, result):
    clean, noisy, result = [array.astype(float) for array in [clean, noisy, result]]
    error, original = np.sum((result - clean) ** 2), np.sum((noisy - clean) ** 2)
    gain = float(np.sum(result * clean) / max(float(np.sum(clean**2)), 1e-30))
    return {
        "snr_db": float(
            10
            * np.log10(max(float(np.sum(clean**2)), 1e-30) / max(float(error), 1e-30))
        ),
        "improvement_db": float(
            10 * np.log10(max(float(original), 1e-30) / max(float(error), 1e-30))
        ),
        "projection_loss_db": float(-20 * np.log10(max(gain, 1e-15))),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--temporary", type=Path, required=True)
    parser.add_argument("--epochs", type=int, default=250)
    args = parser.parse_args()
    if args.output.exists() or args.temporary.exists() or not 1 <= args.epochs <= 2000:
        raise ValueError("choose fresh paths and bounded training epochs")
    args.output.mkdir(parents=True)
    args.temporary.mkdir(parents=True)
    protocol = {
        "format": FORMAT,
        "seed": 20276025,
        "train_scenes": 256,
        "development_scenes": 48,
        "final_scenes": 48,
        "shift_scenes": 30,
        "rates": [16000, 48000],
        "epochs": args.epochs,
        "source_sha256": {
            name: sha(Path(__file__).parent / name)
            for name in [Path(__file__).name, "ir_denoise.py"]
        },
        "selection": "Select weights on development loss only. Freeze weights/hash before generating final/shift arrays. No measured/external training data.",
        "acceptance": "All methods use the same early protection and response checks; compare exported fallback-inclusive output, not only accepted proposals.",
    }
    (args.output / "protocol.json").write_text(json.dumps(protocol, indent=2) + "\n")
    splits, seen, source_manifest = {}, set(), {}
    for split, start, count in [
        ("train", 20276025, 256),
        ("development", 20277025, 48),
    ]:
        rows = []
        source_manifest[split] = []
        for i in range(count):
            clean, noisy, metadata = fixture(start + i, protocol["rates"][i % 2])
            identity = hashlib.sha256(noisy.tobytes()).hexdigest()
            if identity in seen:
                raise ValueError("duplicate generated source")
            seen.add(identity)
            source_manifest[split].append(
                {
                    **metadata,
                    "noisy_sha256": identity,
                    "clean_sha256": hashlib.sha256(clean.tobytes()).hexdigest(),
                }
            )
            rows.append(training_rows(clean, noisy, metadata))
        splits[split] = [np.concatenate([r[j] for r in rows]) for j in range(3)]
    (args.output / "training-sources.json").write_text(
        json.dumps(source_manifest, indent=2) + "\n"
    )
    import torch

    torch.set_num_threads(1)
    torch.manual_seed(20276025)
    network = torch.nn.Sequential(
        torch.nn.Linear(FEATURES, HIDDEN), torch.nn.Tanh(), torch.nn.Linear(HIDDEN, 1)
    )
    optimizer = torch.optim.Adam(network.parameters(), lr=0.005)
    arrays = {
        name: [torch.tensor(x, dtype=torch.float32) for x in rows]
        for name, rows in splits.items()
    }
    best, state, trace = float("inf"), None, []
    for epoch in range(args.epochs):
        x, y, weight = arrays["train"]
        optimizer.zero_grad()
        prediction = 0.05 + 0.95 * torch.sigmoid(network(x).squeeze(1))
        loss = ((prediction - y).square() * weight).sum() / weight.sum()
        loss.backward()
        optimizer.step()
        with torch.no_grad():
            x, y, weight = arrays["development"]
            prediction = 0.05 + 0.95 * torch.sigmoid(network(x).squeeze(1))
            development = float(
                (((prediction - y).square() * weight).sum() / weight.sum()).item()
            )
        if development < best:
            best = development
            state = {
                key: value.detach().clone()
                for key, value in network.state_dict().items()
            }
        trace.append(
            {
                "epoch": epoch,
                "train_loss": float(loss.detach()),
                "development_loss": development,
            }
        )
    network.load_state_dict(state)
    weights = network.state_dict()
    model = {
        "format": FORMAT,
        "w1": weights["0.weight"].tolist(),
        "b1": weights["0.bias"].tolist(),
        "w2": weights["2.weight"].squeeze(0).tolist(),
        "b2": float(weights["2.bias"][0]),
    }
    path = args.output / "model.json"
    path.write_text(json.dumps(model, separators=(",", ":")) + "\n")
    identity = sha(path)
    frozen = load_model(path, identity)
    with torch.no_grad():
        x = splits["development"][0]
        expected = (
            0.05
            + 0.95
            * torch.sigmoid(network(torch.tensor(x, dtype=torch.float32)).squeeze(1))
        ).numpy()
    parity = float(np.max(np.abs(expected - neural_gains(x, frozen))))
    if parity > 1e-5:
        raise ValueError("NumPy/Torch prediction mismatch")
    (args.output / "training.json").write_text(
        json.dumps(
            {
                "model_sha256": identity,
                "best_development_loss": best,
                "numpy_torch_max_error": parity,
                "trace": trace,
            },
            indent=2,
        )
        + "\n"
    )
    # The frozen file above precedes creation or reading of every final sample.
    rows = []
    families = [
        "late_reflection",
        "two_slopes",
        "ringing",
        "colored_noise",
        "noise_step",
    ]
    for index in range(78):
        split = "final" if index < 48 else "shift"
        rate = protocol["rates"][index % 2]
        family = "training" if split == "final" else families[(index - 48) // 6]
        clean, noisy, metadata = fixture(20278025 + index, rate, family)
        directory = args.temporary / f"case-{index:03d}"
        directory.mkdir()
        row = {
            "index": index,
            "split": split,
            "metadata": metadata,
            "directory": str(directory.resolve()),
            "arrays": {},
            "methods": {},
        }
        for name, array in [("clean", clean), ("noisy", noisy)]:
            p = directory / (name + ".f32le")
            p.write_bytes(array.tobytes())
            row["arrays"][name] = {"sha256": sha(p), "floats": array.size}
        if row["arrays"]["noisy"]["sha256"] in seen:
            raise ValueError("held-out overlap")
        seen.add(row["arrays"]["noisy"]["sha256"])
        row["methods"]["unchanged"] = {"quality": metrics(clean, noisy, noisy)}
        for method in ["neural", "wiener", "decay"]:
            result, checks, _ = propose(
                noisy,
                rate,
                metadata["arrival"],
                metadata["noise_frames"],
                frozen,
                method,
            )
            p = directory / (method + ".f32le")
            p.write_bytes(result.tobytes())
            row["arrays"][method] = {"sha256": sha(p), "floats": result.size}
            row["methods"][method] = {
                "checks": checks,
                "quality": metrics(clean, noisy, result),
            }
        rows.append(row)
        (args.output / f"case-{index:03d}.json").write_text(
            json.dumps(row, indent=2, allow_nan=False) + "\n"
        )
    summary = {
        "model_sha256": identity,
        "best_development_loss": best,
        "numpy_torch_max_error": parity,
        "cases": len(rows),
        "training_scenes": 256,
        "development_scenes": 48,
        "source_hashes_disjoint": True,
        "splits": {},
    }
    for split in ["final", "shift"]:
        subset = [r for r in rows if r["split"] == split]
        summary["splits"][split] = {
            method: {
                "accepted": sum(
                    r["methods"][method]["checks"]["accepted"] for r in subset
                ),
                "median_improvement_db": float(
                    np.median(
                        [
                            r["methods"][method]["quality"]["improvement_db"]
                            for r in subset
                        ]
                    )
                ),
                "worst_improvement_db": min(
                    r["methods"][method]["quality"]["improvement_db"] for r in subset
                ),
            }
            for method in ["neural", "wiener", "decay"]
        }
        summary["splits"][split]["neural_better_than_best_classical"] = sum(
            r["methods"]["neural"]["quality"]["snr_db"]
            > max(r["methods"][m]["quality"]["snr_db"] for m in ["wiener", "decay"])
            + 1e-6
            for r in subset
        )
    (args.output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary), flush=True)


if __name__ == "__main__":
    main()
