"""Freeze an offline candidate before reading validation/test target PCM."""

import argparse
import copy
import hashlib
import json
from pathlib import Path

from analog_hardware_capture import load_manifest, read_corpus
from analog_hardware_model import evaluate, fit, identity


def run(data, output, taps=8, hidden=12, iterations=300):
    output = Path(output)
    output.mkdir(parents=True, exist_ok=False)
    train = copy.deepcopy(data)
    train["captures"] = [r for r in train["captures"] if r["split"] == "train"]
    loaded, _ = read_corpus(train)
    model, diagnostics = fit(data, loaded, taps, hidden, iterations)
    model["capture_manifest_canonical_sha256"] = identity(data)
    raw = json.dumps(model, indent=2, allow_nan=False) + "\n"
    (output / "candidate.json").write_text(raw)
    # The neural parameters and classical comparison are now frozen on disk.
    held = []
    for split in ["validation", "test"]:
        subset = copy.deepcopy(data)
        subset["captures"] = [r for r in data["captures"] if r["split"] == split]
        signals, _ = read_corpus(subset)
        held.append(evaluate(model, data, signals, split))
    report = {
        "candidate_sha256": hashlib.sha256(raw.encode()).hexdigest(),
        "training": diagnostics,
        "held_splits": held,
        "offline_quality_passed": all(s["passed"] for s in held),
        "data_origin": data["context"]["data_origin"],
        "hardware_model_admitted": False,
        "automatic_apply": False,
        "scope": "Candidate training and held-source comparison only. No measured-hardware fidelity, native inference/alias/cost or host admission follows from synthetic fixtures or declared metadata.",
    }
    (output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    return model, report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--taps", type=int, default=8)
    parser.add_argument("--hidden", type=int, default=12)
    parser.add_argument("--iterations", type=int, default=300)
    args = parser.parse_args()
    data, _ = load_manifest(args.manifest)
    _, report = run(data, args.output, args.taps, args.hidden, args.iterations)
    print(
        json.dumps(
            {
                "offline_quality_passed": report["offline_quality_passed"],
                "hardware_model_admitted": False,
                "automatic_apply": False,
            }
        )
    )
    if not report["offline_quality_passed"]:
        raise SystemExit(2)


if __name__ == "__main__":
    main()
