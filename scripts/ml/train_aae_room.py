"""Train a monotone forward AAE surrogate on reproducible native IR descriptors.

The final holdout is opened only after saving/hashing the selected weights.
No measured recording enters training. The inverse fitter verifies candidates
with the original renderer and compares with a non-neural native dictionary.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import time
from pathlib import Path

import numpy as np
from aae_room_features import (
    CONTROL_SPEC,
    FEATURE_SPEC,
    LAYOUTS,
    PRESETS,
    Renderer,
    actual_unit,
    describe,
    encode,
)

SPLITS = {
    "train": (20261025, 1536),
    "development": (20262025, 192),
    "holdout": (20263025, 256),
}


def generate(args):
    from scipy.stats import qmc

    args.output.mkdir(parents=True, exist_ok=True)
    hashes = {}
    with Renderer(args.renderer, args.temporary) as renderer:
        for split, (seed, count) in SPLITS.items():
            # Latin-hypercube coordinates cover physical bounds logarithmically
            # for positive scale/decay controls; time and mix remain linear.
            unit = qmc.LatinHypercube(6, seed=seed).random(count)
            order = np.random.default_rng(seed + 1).permutation(count)
            features, inputs, records = [], [], []
            for index, row in enumerate(unit):
                stratum = int(order[index]) % (len(LAYOUTS) * len(PRESETS))
                layout = LAYOUTS[stratum % len(LAYOUTS)]
                preset = PRESETS[stratum // len(LAYOUTS)]
                audio, meta = renderer.render(row, layout, preset)
                description = describe(audio, meta["sample_rate"])
                assert description["mask"].all()
                actual = actual_unit(meta["params"])
                features.append(description["features"])
                inputs.append(encode(actual, layout, preset))
                identity = hashlib.sha256(
                    json.dumps(
                        {
                            "params": meta["params"],
                            "rate": meta["sample_rate"],
                            "frames": meta["frames"],
                        },
                        sort_keys=True,
                    ).encode()
                ).hexdigest()
                if identity in hashes or meta["sha256"] in hashes:
                    raise RuntimeError(
                        "overlapping native request/audio between corpus rows"
                    )
                hashes[identity] = split
                hashes[meta["sha256"]] = split
                records.append(
                    {
                        "case_id": f"{split}:{seed}:{index}",
                        "identity": identity,
                        "audio_sha256": meta["sha256"],
                        "params": meta["params"],
                        "native_tail": meta["native_tail"],
                        "observed_last_tenth_energy_fraction": description[
                            "observed_last_tenth_energy_fraction"
                        ],
                    }
                )
                if (index + 1) % 128 == 0:
                    print(
                        json.dumps(
                            {"split": split, "generated": index + 1, "total": count}
                        ),
                        flush=True,
                    )
            np.savez_compressed(
                args.output / f"{split}.npz",
                x=np.array(inputs, dtype=np.float32),
                y=np.array(features, dtype=np.float32),
            )
            (args.output / f"{split}.json").write_text(
                json.dumps(
                    {
                        "seed": seed,
                        "count": count,
                        "renderer_sha256": renderer.binary_sha256,
                        "feature_spec": FEATURE_SPEC,
                        "records": records,
                    },
                    indent=2,
                )
                + "\n"
            )
    print(
        json.dumps(
            {"status": "generated", "split_sizes": {k: v[1] for k, v in SPLITS.items()}}
        ),
        flush=True,
    )


def train(args):
    import torch
    from torch import nn

    if (args.output / "model.json").exists():
        raise ValueError(
            "refusing to overwrite frozen model; choose a new version directory"
        )
    torch.set_num_threads(2)
    torch.manual_seed(20264025)
    train = np.load(args.data / "train.npz")
    dev = np.load(args.data / "development.npz")
    x, y = torch.from_numpy(train["x"]), torch.from_numpy(train["y"])
    dx, dy = torch.from_numpy(dev["x"]), torch.from_numpy(dev["y"])
    channels, points = 7, 32

    class Forward(nn.Module):
        def __init__(self):
            super().__init__()
            self.layers = nn.Sequential(
                nn.Linear(19, 128),
                nn.SiLU(),
                nn.Linear(128, 128),
                nn.SiLU(),
                nn.Linear(128, 224),
            )
            nn.init.constant_(self.layers[-1].bias, -2.0)

        def forward(self, value):
            increments = nn.functional.softplus(self.layers(value)).reshape(
                -1, channels, points
            )
            return -torch.clamp(torch.cumsum(increments, dim=2), max=40.0).reshape(
                -1, 224
            )

    model = Forward()
    optimizer = torch.optim.AdamW(model.parameters(), lr=0.002, weight_decay=0.00001)
    best = float("inf")
    best_state = None
    trace = []
    start = time.time()
    rng = torch.Generator().manual_seed(20264026)
    for epoch in range(args.epochs):
        model.train()
        order = torch.randperm(len(x), generator=rng)
        for indices in order.split(128):
            optimizer.zero_grad()
            loss = torch.mean((model(x[indices]) - y[indices]) ** 2)
            loss.backward()
            nn.utils.clip_grad_norm_(model.parameters(), 20.0)
            optimizer.step()
        if epoch % 10 == 0 or epoch + 1 == args.epochs:
            model.eval()
            with torch.no_grad():
                value = float(torch.mean((model(dx) - dy) ** 2))
            trace.append({"epoch": epoch, "development_mse_db2": value})
            if value < best:
                best = value
                best_state = {
                    k: v.detach().clone() for k, v in model.state_dict().items()
                }
            if epoch % 100 == 0:
                print(json.dumps(trace[-1]), flush=True)
    assert best_state is not None
    model.load_state_dict(best_state)
    layers = []
    for layer in model.layers:
        if isinstance(layer, nn.Linear):
            layers.append(
                {
                    "weight": layer.weight.detach().numpy().tolist(),
                    "bias": layer.bias.detach().numpy().tolist(),
                }
            )
    args.output.mkdir(parents=True, exist_ok=True)
    payload = {
        "format": "sotf_aae_monotone_forward_v1",
        "feature_spec": FEATURE_SPEC,
        "control_spec": CONTROL_SPEC,
        "architecture": {
            "input": 19,
            "hidden": [128, 128],
            "hidden_activation": "silu",
            "output": "negative_cumsum_softplus_clamp40",
            "curve_shape": [7, 32],
        },
        "layers": layers,
        "training": {
            "seed": 20264025,
            "epochs": args.epochs,
            "selected_by": "development_mse_only",
            "best_development_mse_db2": best,
            "seconds": time.time() - start,
            "data_sha256": {
                name: hashlib.sha256(
                    (args.data / f"{name}.npz").read_bytes()
                ).hexdigest()
                for name in ("train", "development")
            },
        },
    }
    path = args.output / "model.json"
    path.write_text(json.dumps(payload, separators=(",", ":"), allow_nan=False) + "\n")
    sha = hashlib.sha256(path.read_bytes()).hexdigest()
    (args.output / "model.sha256").write_text(sha + "  model.json\n")
    np.savez_compressed(
        args.output / "native_dictionary.npz", x=train["x"], y=train["y"]
    )
    # No parameter/checkpoint changes after this boundary. The final holdout is
    # independent of model selection and only now opened for the first time.
    from aae_room_model import Surrogate

    reference = Surrogate(path, sha)
    heldout = np.load(args.data / "holdout.npz")
    prediction = reference.predict(heldout["x"])
    error = np.sqrt(np.mean((prediction - heldout["y"]) ** 2, axis=1))
    with torch.no_grad():
        native_prediction = model(torch.from_numpy(heldout["x"])).numpy()
    parity = float(np.max(np.abs(prediction - native_prediction)))
    assert parity < 0.001, parity
    report = {
        "weights_sha256": sha,
        "final_holdout_count": len(error),
        "mean_profile_rmse_db": float(np.mean(error)),
        "median_profile_rmse_db": float(np.median(error)),
        "p95_profile_rmse_db": float(np.quantile(error, 0.95)),
        "worst_profile_rmse_db": float(np.max(error)),
        "numpy_torch_max_error_db": parity,
        "development_trace": trace,
        "scope": "Synthetic forward-model error only; not measured-room fit or adoption evidence.",
    }
    (args.output / "training-report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    print(
        json.dumps({k: v for k, v in report.items() if k != "development_trace"}),
        flush=True,
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    create = sub.add_parser("generate")
    create.add_argument("--renderer", type=Path, required=True)
    create.add_argument("--temporary", type=Path, required=True)
    create.add_argument("--output", type=Path, required=True)
    fit = sub.add_parser("train")
    fit.add_argument("--data", type=Path, required=True)
    fit.add_argument("--output", type=Path, required=True)
    fit.add_argument("--epochs", type=int, default=1500)
    args = parser.parse_args()
    (generate if args.command == "generate" else train)(args)


if __name__ == "__main__":
    main()
