"""Select IR preservation budgets on fresh development data, then freeze and test.

Weights remain the original model. Neither the prior rejected final corpus nor
this new final set enters selection. Classical methods each select their own
budget on the same development cases. Arrays stay outside the repository.
"""

from __future__ import annotations

import argparse
import hashlib
import itertools
import json
from pathlib import Path

import numpy as np
from ir_denoise import load_model, propose, sha
from ir_preservation import FORMAT
from train_ir_denoise import fixture, metrics

METHODS = ("neural", "wiener", "decay")
FAMILIES = (
    "training",
    "late_reflection",
    "two_slopes",
    "ringing",
    "colored_noise",
    "noise_step",
)
SHIFTS = ("late_packet", "truncated", "prefix_step", "linked_four")


def scene(seed, rate, family):
    base = family if family in FAMILIES else "training"
    clean, noisy, info = fixture(seed, rate, base)
    info["family"] = family
    center = None
    if family == "late_packet":
        center = info["arrival"] + round(rate * 0.9)
        length = round(rate * 0.015)
        rng = np.random.default_rng(seed + 100000)
        burst = rng.normal(size=(length, clean.shape[1])) * np.hanning(length)[:, None]
        burst *= np.sqrt(0.005 / np.sum(burst**2))
        clean[center : center + length] += burst.astype("<f4")
        noisy[center : center + length] += burst.astype("<f4")
    elif family == "truncated":
        stop = round(1.1 * rate)
        clean, noisy = clean[:stop].copy(), noisy[:stop].copy()
    elif family == "prefix_step":
        noisy[: info["noise_frames"] // 2] *= 4
    elif family == "linked_four":
        clean = np.column_stack([clean, clean * -0.25])
        noisy = np.column_stack([noisy, noisy * -0.25])
        # Ensure a four-channel test even for odd/even seed parity.
        if clean.shape[1] == 2:
            clean = np.column_stack([clean, clean * 0.5])
            noisy = np.column_stack([noisy, noisy * 0.5])
    elif family == "late_reflection":
        center = info["arrival"] + round(rate * 0.8)
    info.update(frames=len(clean), channels=clean.shape[1])
    if center is not None:
        info["structure_window"] = [
            max(info["arrival"], center - round(rate * 0.01)),
            min(len(clean), center + round(rate * 0.03)),
        ]
    return clean.astype("<f4"), noisy.astype("<f4"), info


def evaluate(clean, noisy, info, model, method, policy):
    try:
        output, checks, mask = propose(
            noisy,
            info["rate"],
            info["arrival"],
            info["noise_frames"],
            model,
            method,
            preservation_policy=policy,
        )
    except ValueError as error:
        output, mask = noisy.copy(), np.ones(len(noisy))
        checks = {"accepted": False, "fallback_error": str(error)}
    quality = metrics(clean, noisy, output)
    if "structure_window" in info:
        first, last = info["structure_window"]
        target = clean[first:last].astype(float)
        baseline = float(np.sum(noisy[first:last].astype(float) * target))
        changed = float(np.sum(output[first:last].astype(float) * target))
        quality["added_structure_projection_loss_db"] = float(
            -20 * np.log10(max(changed / max(baseline, 1e-30), 1e-15))
        )
    return output, checks, quality, mask


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, allow_nan=False) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--model-sha256", required=True)
    parser.add_argument("--previous-evidence", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--temporary", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists() or args.temporary.exists():
        raise ValueError("preserve prior experiments")
    model = load_model(args.model, args.model_sha256)
    args.output.mkdir(parents=True)
    args.temporary.mkdir(parents=True)
    protocol = {
        "format": "sotf_ir_preservation_experiment_v1",
        "model_sha256": args.model_sha256,
        "development_seed": 20280025,
        "development_cases": 60,
        "development_rates": [16000, 48000],
        "final_seed": 20281025,
        "final_cases": 72,
        "final_rates": [16000, 44100, 48000, 96000],
        "shift_seed": 20282025,
        "shift_cases": 32,
        "shift_families": list(SHIFTS),
        "policy_grid": {
            "window_ms": [1.0, 5.0, 20.0],
            "noise_fraction": [0.25, 0.5, 1.0],
        },
        "selection": "Each method independently chooses among policies meeting development worst SNR delta >= -0.25 dB and max added structure projection loss <= 0.1 dB. Maximize median improvement, then worst improvement. If none qualifies, record failure and select max worst improvement, then median.",
        "freeze": "Write selected policies/hashes before generating or reading any new final/shift source. Keep original model weights unchanged.",
        "source_sha256": {
            name: sha(Path(__file__).parent / name)
            for name in [
                Path(__file__).name,
                "ir_preservation.py",
                "ir_denoise.py",
                "train_ir_denoise.py",
            ]
        },
        "scope": "Generated mono/stereo plus linked four-channel shift, simulated IRs and stationary/piecewise noise; no measured-room or listening acceptance.",
    }
    write_json(args.output / "protocol.json", protocol)
    snapshot = args.output / "evaluation-source"
    snapshot.mkdir()
    for name in protocol["source_sha256"]:
        (snapshot / name).write_bytes((Path(__file__).parent / name).read_bytes())
    seen = set()
    previous = json.loads(
        (args.previous_evidence / "training-sources.json").read_text()
    )
    for rows in previous.values():
        seen.update(row["noisy_sha256"] for row in rows)
    for path in args.previous_evidence.glob("case-*.json"):
        seen.add(json.loads(path.read_text())["arrays"]["noisy"]["sha256"])
    previous_count = len(seen)
    development, source_records = [], []
    for index in range(protocol["development_cases"]):
        family = FAMILIES[index // 10]
        clean, noisy, info = scene(
            protocol["development_seed"] + index,
            protocol["development_rates"][index % 2],
            family,
        )
        digest = hashlib.sha256(noisy.tobytes()).hexdigest()
        if digest in seen:
            raise ValueError("development overlaps prior scenes")
        seen.add(digest)
        development.append((clean, noisy, info))
        source_records.append(
            {
                **info,
                "noisy_sha256": digest,
                "clean_sha256": hashlib.sha256(clean.tobytes()).hexdigest(),
            }
        )
    write_json(args.output / "development-sources.json", source_records)
    candidates = []
    policies = [
        {"format": FORMAT, "window_ms": w, "noise_fraction": f}
        for w, f in itertools.product(
            protocol["policy_grid"]["window_ms"],
            protocol["policy_grid"]["noise_fraction"],
        )
    ]
    selected, selected_hashes = {}, {}
    for method in METHODS:
        comparisons = []
        for policy in policies:
            scores = []
            for clean, noisy, info in development:
                _, checks, quality, _ = evaluate(
                    clean, noisy, info, model, method, policy
                )
                scores.append(
                    {
                        "seed": info["seed"],
                        "family": info["family"],
                        "accepted": checks["accepted"],
                        **quality,
                    }
                )
            worst = min(row["improvement_db"] for row in scores)
            loss = max(
                row.get("added_structure_projection_loss_db", 0.0) for row in scores
            )
            median = float(np.median([row["improvement_db"] for row in scores]))
            comparisons.append(
                {
                    "method": method,
                    "policy": policy,
                    "qualified": worst >= -0.25 and loss <= 0.1,
                    "worst_improvement_db": worst,
                    "median_improvement_db": median,
                    "max_added_structure_loss_db": loss,
                    "scores": scores,
                }
            )
        qualifying = [row for row in comparisons if row["qualified"]]
        chosen = (
            max(
                qualifying,
                key=lambda row: (
                    row["median_improvement_db"],
                    row["worst_improvement_db"],
                ),
            )
            if qualifying
            else max(
                comparisons,
                key=lambda row: (
                    row["worst_improvement_db"],
                    row["median_improvement_db"],
                ),
            )
        )
        selected[method] = chosen["policy"]
        file = args.output / (method + "-policy.json")
        write_json(file, chosen["policy"])
        selected_hashes[method] = sha(file)
        candidates.extend(comparisons)
    write_json(
        args.output / "development-selection.json",
        {
            "candidates": candidates,
            "selected": selected,
            "policy_sha256": selected_hashes,
        },
    )
    # Selected immutable policy files above precede every final source array.
    rows = []
    for index in range(protocol["final_cases"] + protocol["shift_cases"]):
        split = "final" if index < protocol["final_cases"] else "shift"
        offset = index if split == "final" else index - protocol["final_cases"]
        family = FAMILIES[offset // 12] if split == "final" else SHIFTS[offset // 8]
        rate = protocol["final_rates"][offset % 4]
        seed = protocol["final_seed" if split == "final" else "shift_seed"] + offset
        clean, noisy, info = scene(seed, rate, family)
        identity = hashlib.sha256(noisy.tobytes()).hexdigest()
        if identity in seen:
            raise ValueError("final overlap with prior or development sources")
        seen.add(identity)
        directory = args.temporary / f"case-{index:03d}"
        directory.mkdir()
        row = {
            "index": index,
            "split": split,
            "metadata": info,
            "directory": str(directory.resolve()),
            "arrays": {},
            "methods": {"unchanged": {"quality": metrics(clean, noisy, noisy)}},
        }
        for name, array in [("clean", clean), ("noisy", noisy)]:
            path = directory / (name + ".f32le")
            path.write_bytes(array.tobytes())
            row["arrays"][name] = {"sha256": sha(path), "floats": array.size}
        for method in METHODS:
            for label, policy in [
                (method, selected[method]),
                (method + "_unprotected", None),
            ]:
                output, checks, quality, mask = evaluate(
                    clean, noisy, info, model, method, policy
                )
                file = directory / (label + ".f32le")
                file.write_bytes(output.tobytes())
                row["arrays"][label] = {"sha256": sha(file), "floats": output.size}
                if policy is not None:
                    mask_path = directory / (label + "_mask.f64le")
                    mask_path.write_bytes(np.asarray(mask, dtype="<f8").tobytes())
                    row["arrays"][label + "_mask"] = {
                        "sha256": sha(mask_path),
                        "doubles": len(mask),
                    }
                row["methods"][label] = {
                    "checks": checks,
                    "quality": quality,
                    "policy_sha256": selected_hashes[method]
                    if policy is not None
                    else None,
                }
        write_json(args.output / f"case-{index:03d}.json", row)
        rows.append(row)
    summary = {
        "model_sha256": args.model_sha256,
        "policies": selected,
        "policy_sha256": selected_hashes,
        "prior_sources": previous_count,
        "development_cases": len(development),
        "cases": len(rows),
        "new_source_hashes_disjoint": len(seen)
        == previous_count + len(development) + len(rows),
        "splits": {},
    }
    for split in ["final", "shift"]:
        subset = [r for r in rows if r["split"] == split]
        summary["splits"][split] = {
            label: {
                "accepted": sum(
                    r["methods"][label]["checks"]["accepted"] for r in subset
                ),
                "median_improvement_db": float(
                    np.median(
                        [
                            r["methods"][label]["quality"]["improvement_db"]
                            for r in subset
                        ]
                    )
                ),
                "worst_improvement_db": min(
                    r["methods"][label]["quality"]["improvement_db"] for r in subset
                ),
                "max_added_structure_loss_db": max(
                    r["methods"][label]["quality"].get(
                        "added_structure_projection_loss_db", 0.0
                    )
                    for r in subset
                ),
            }
            for label in [*METHODS, *(m + "_unprotected" for m in METHODS)]
        }
        summary["splits"][split]["neural_better_than_best_classical"] = sum(
            r["methods"]["neural"]["quality"]["snr_db"]
            > max(r["methods"][m]["quality"]["snr_db"] for m in ["wiener", "decay"])
            + 1e-6
            for r in subset
        )
    write_json(args.output / "summary.json", summary)
    print(json.dumps(summary), flush=True)


if __name__ == "__main__":
    main()
