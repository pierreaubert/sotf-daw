#!/usr/bin/env python3
"""Score frozen paired EQ checkpoints on fresh curves using the full fit gate."""

import argparse
import contextlib
import io
import json
from pathlib import Path
from types import SimpleNamespace

import eq_multiband as eq
import numpy as np
import torch
from eq_model_loading import load_model
from scipy.signal import bilinear, sosfreqz
from train_eq_refined import collect_ids


def independent_response(filters, frequencies, rate):
    """Analog peak/shelf prototypes mapped by SciPy's bilinear transform."""
    sections = []
    for band in filters:
        frequency, gain = band["freq"], band["db_gain"]
        w = 2 * rate * np.tan(np.pi * frequency / rate)
        a = 10 ** (gain / 40)
        if band["filter_type"] == "peak":
            q = band["q"]
            numerator, denominator = [1, w * a / q, w * w], [1, w / (a * q), w * w]
        elif band["filter_type"] == "lowshelf":
            numerator = [a, a * np.sqrt(2 * a) * w, a * a * w * w]
            denominator = [a, np.sqrt(2 * a) * w, w * w]
        elif band["filter_type"] == "highshelf":
            numerator = [a * a, a * np.sqrt(2 * a) * w, a * w * w]
            denominator = [1, np.sqrt(2 * a) * w, a * w * w]
        else:
            raise ValueError("Unexpected exported filter type")
        b, d = bilinear(numerator, denominator, fs=rate)
        sections.append(np.r_[b, d])
    if not sections:
        return np.zeros_like(frequencies)
    _, h = sosfreqz(sections, worN=frequencies, fs=rate)
    return 20 * np.log10(np.abs(h))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--experiment", type=Path, required=True)
    parser.add_argument("--base", type=Path, required=True)
    parser.add_argument("--protocol", type=Path, required=True)
    parser.add_argument("--exclude", type=Path, action="append", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    torch.set_num_threads(2)
    protocol = json.loads(args.protocol.read_text())
    split = eq.dataset(protocol["parameter_cases"], protocol["final_seed"])
    excluded = collect_ids(
        json.loads((args.experiment / "data-manifest.json").read_text())
    )
    excluded.update(
        json.loads((args.experiment / "data-manifest.json").read_text())["excluded_ids"]
    )
    for path in args.exclude:
        excluded.update(collect_ids(json.loads(path.read_text())))
    if excluded & set(split[3]):
        raise ValueError("Final targets overlap previous training or evaluation")
    models = {"base": args.base}
    models.update(
        {
            name: args.experiment / name / "weights.json"
            for name in ("original-labels", "refined-labels")
        }
    )
    frozen = {name: eq.sha(path) for name, path in models.items()}
    loaded = {
        name: load_model(path, eq.CONTRACT, eq.network)[0]
        for name, path in models.items()
    }
    args.output.mkdir(parents=True, exist_ok=False)
    (args.output / "frozen-models.json").write_text(json.dumps(frozen, indent=2) + "\n")
    cases = []
    for index, p in enumerate(split[0]):
        for rate in protocol["rates"]:
            cases.append(
                (
                    f"family-{index}-{rate}",
                    eq.response(p, rate=rate),
                    rate,
                    split[3][index],
                )
            )
    for rate in protocol["shift_rates"]:
        cases.append(
            (f"shift-{rate}", eq.response(split[0][0], rate=rate), rate, split[3][0])
        )
    rng = np.random.default_rng(protocol["noise_seed"])
    for rate in [48000, 96000]:
        target = eq.response(split[0][1], rate=rate) + rng.normal(0, 0.05, len(eq.GRID))
        cases.append((f"noise-{rate}", target, rate, split[3][1]))
    cases.extend(
        [
            ("jagged", np.where(np.arange(len(eq.GRID)) % 2, 3.0, -3.0), 48000, None),
            ("flat", np.zeros(len(eq.GRID)), 48000, None),
            ("near-zero", np.full(len(eq.GRID), 0.009), 48000, None),
        ]
    )
    rows = []
    for name, target, rate, identity in cases:
        target_path = args.output / (name + "-target.json")
        target_path.write_text(
            json.dumps(
                {
                    "frequencies_hz": eq.GRID.tolist(),
                    "target_db": target.tolist(),
                    "sample_rate": rate,
                }
            )
            + "\n"
        )
        per_case = []
        for arm, weights in models.items():
            result_path = args.output / (name + "-" + arm + ".json")
            try:
                with contextlib.redirect_stdout(io.StringIO()):
                    eq.fit(
                        SimpleNamespace(
                            target=target_path,
                            weights=weights,
                            output=result_path,
                            max_evaluations=protocol["solver_budget"],
                        )
                    )
            except SystemExit as error:
                if error.code != 2:
                    raise
            result = json.loads(result_path.read_text())
            independent = None
            if result["accepted"]:
                error = (
                    independent_response(result["parameters"]["filters"], eq.GRID, rate)
                    - target
                )
                independent = {
                    "rms_db": float(np.sqrt(np.mean(error**2))),
                    "max_db": float(np.max(np.abs(error))),
                }
                if independent["rms_db"] > 0.100001 or independent["max_db"] > 0.500001:
                    raise ValueError("Independent exported response failed accuracy")
                if (
                    result["method"] != "canonical-flat"
                    and abs(independent["rms_db"] - result["sampled_rms_db"]) > 1e-6
                ):
                    raise ValueError(
                        "Independent exported response differs from fit report"
                    )
            elif result["parameters"] is not None:
                raise ValueError("Rejected fit exposed controls")
            rows.append(
                {
                    "case": name,
                    "arm": arm,
                    "parameter_identity": identity,
                    "result": result,
                    "independent": independent,
                }
            )
            per_case.append(result)
        # All arms retain identical deterministic fallback searches.
        if name not in ("flat", "near-zero"):
            for index in [1, 2]:
                for candidate in [1, 2]:
                    a, b = (
                        per_case[0]["candidates"][candidate],
                        per_case[index]["candidates"][candidate],
                    )
                    for key in ("mse", "maximum_db", "residual_calls", "accepted"):
                        if a[key] != b[key]:
                            raise ValueError("Classical fallback changed between arms")
        print(
            json.dumps(
                {
                    "case": name,
                    "accepted": [r["accepted"] for r in per_case],
                    "methods": [r["method"] for r in per_case],
                }
            ),
            flush=True,
        )
    summary = {
        "protocol_sha256": eq.sha(args.protocol),
        "frozen_weights": frozen,
        "unique_parameter_identities": split[3],
        "cases": len(cases),
        "rows": rows,
        "qualification": "Synthetic curves and correlated rate/noise variants; no measured-target, perceptual or cold-cost claim",
    }
    for arm in models:
        selected = [r for r in rows if r["arm"] == arm]
        neural = [
            r["result"]["candidates"][0]
            for r in selected
            if "candidates" in r["result"]
        ]
        summary[arm] = {
            "combined_accepted": sum(r["result"]["accepted"] for r in selected),
            "neural_accepted": sum(r["accepted"] for r in neural),
            "neural_cases": len(neural),
            "median_neural_residual_calls": float(
                np.median([r["residual_calls"] for r in neural])
            ),
        }
    if frozen != {name: eq.sha(path) for name, path in models.items()}:
        raise ValueError("Models changed during final evaluation")
    summary["raw_fresh_mse_db2"] = {
        name: eq.evaluate(model, split)[0] for name, model in loaded.items()
    }
    (args.output / "comparison.json").write_text(
        json.dumps(summary, indent=2, allow_nan=False) + "\n"
    )


if __name__ == "__main__":
    main()
