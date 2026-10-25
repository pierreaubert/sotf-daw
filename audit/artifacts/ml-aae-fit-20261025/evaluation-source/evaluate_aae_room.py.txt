"""Compare AAE inverse fitting against native dictionary fitting at equal budgets.

Targets are new native responses, not measured-room validation. Freeze the
development implementation before running a final evaluation with a new seed.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import time
from pathlib import Path

from aae_room_features import LAYOUTS, PRESETS, Renderer, describe
from aae_room_model import Surrogate
from fit_aae_room import fit
from scipy.stats import qmc


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--temporary", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--model-sha256", required=True)
    parser.add_argument("--dictionary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--seed", type=int, required=True)
    parser.add_argument("--cases", type=int, default=9)
    parser.add_argument("--budget", type=int, default=64)
    args = parser.parse_args()
    if args.output.exists():
        raise ValueError("evaluation output already exists; preserve previous evidence")
    args.output.mkdir(parents=True)
    if not 1 <= args.cases <= 360:
        raise ValueError("case count must be 1..360")
    controls = qmc.LatinHypercube(6, seed=args.seed).random(args.cases)
    setup_start = time.perf_counter()
    model = Surrogate(args.model, args.model_sha256)
    setup_seconds = time.perf_counter() - setup_start
    protocol = {
        "seed": args.seed,
        "count": args.cases,
        "native_budget_per_method": args.budget,
        "rate": 16000,
        "observation_seconds": 8,
        "model_sha256": model.sha256,
        "model_load_seconds": setup_seconds,
        "dictionary_sha256": hashlib.sha256(args.dictionary.read_bytes()).hexdigest(),
        "selection": "fixed controls; no retuning from these results",
        "scope": "synthetic inverse recovery only; measured-room validation pending",
        "source_sha256": {
            name: hashlib.sha256(
                (Path(__file__).parent / name).read_bytes()
            ).hexdigest()
            for name in [
                "aae_room_features.py",
                "aae_room_model.py",
                "fit_aae_room.py",
                "evaluate_aae_room.py",
            ]
        },
    }
    (args.output / "protocol.json").write_text(json.dumps(protocol, indent=2) + "\n")
    rows = []
    with Renderer(args.renderer, args.temporary) as renderer:
        protocol["renderer_sha256"] = renderer.binary_sha256
        (args.output / "protocol.json").write_text(
            json.dumps(protocol, indent=2) + "\n"
        )
        for index, unit in enumerate(controls):
            layout = LAYOUTS[index % len(LAYOUTS)]
            preset = PRESETS[
                (index // len(LAYOUTS) + index % len(LAYOUTS)) % len(PRESETS)
            ]
            audio, metadata = renderer.render(unit, layout, preset)
            target = describe(audio, 16000)
            record = {
                "case": index,
                "layout": layout,
                "target_preset": preset,
                "target_params": metadata["params"],
                "target_audio_sha256": metadata["sha256"],
                "target_native_tail": metadata["native_tail"],
            }
            # Alternate execution order to reduce order-related timing bias.
            for method in (
                ["classical", "neural"] if index % 2 == 0 else ["neural", "classical"]
            ):
                try:
                    record[method] = fit(
                        target,
                        renderer,
                        layout=layout,
                        rate=16000,
                        budget=args.budget,
                        dictionary=args.dictionary,
                        model=model if method == "neural" else None,
                    )
                except ValueError as error:
                    record[method] = {"error": str(error)}
            (args.output / f"case-{index:03d}.json").write_text(
                json.dumps(record, indent=2, allow_nan=False) + "\n"
            )
            rows.append(record)
            print(
                json.dumps(
                    {
                        "case": index,
                        **{
                            method: record[method].get(
                                "profile_rmse_db", record[method].get("error")
                            )
                            for method in ["classical", "neural"]
                        },
                    }
                ),
                flush=True,
            )
    summary = {"protocol": protocol, "methods": {}}
    for method in ["classical", "neural"]:
        valid = [row[method] for row in rows if "error" not in row[method]]
        summary["methods"][method] = {
            "admitted_cases": len(valid),
            "rejected_cases": len(rows) - len(valid),
            "qualified_cases": sum(row["qualified_profile_proxy"] for row in valid),
            "rmse_db": [row["profile_rmse_db"] for row in valid],
            "seconds": [row["fit_seconds"] for row in valid],
            "native_evaluations": [row["native_evaluations"] for row in valid],
        }
    paired = [
        row
        for row in rows
        if all("error" not in row[m] for m in ["classical", "neural"])
    ]
    summary["neural_strictly_better_cases"] = sum(
        row["neural"]["profile_rmse_db"] < row["classical"]["profile_rmse_db"]
        for row in paired
    )
    summary["paired_cases"] = len(paired)
    (args.output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary), flush=True)


if __name__ == "__main__":
    main()
