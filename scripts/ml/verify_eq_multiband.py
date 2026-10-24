#!/usr/bin/env python3
"""Check the multi-band checkpoint, frozen recipe, and final holdout identities."""

import argparse
import hashlib
import json
from pathlib import Path


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("model", type=Path)
    args = parser.parse_args()
    root = args.model
    training = json.loads((root / "training-report.json").read_text())
    data = json.loads((root / "data-manifest.json").read_text())
    validation = json.loads((root / "validation.json").read_text())
    for name, expected in [
        ("weights.json", training["weights_sha256"]),
        ("best.pt", training["checkpoint_sha256"]),
        ("data-manifest.json", training["data_manifest_sha256"]),
        ("recipe.py", data["script_sha256"]),
        ("final-holdout.json", validation["final_holdout_sha256"]),
    ]:
        if sha(root / name) != expected:
            raise ValueError(f"Artifact hash mismatch: {name}")
    raw = json.loads((root / "weights.json").read_text())
    if (
        raw["contract"] != data["contract"]
        or raw["contract"] != "sotf-four-peak-two-shelf-v1"
    ):
        raise ValueError("Model contract mismatch")
    seen = set()
    sizes = {}
    for name, identities in data["split_ids"].items():
        current = set(identities)
        if len(current) != len(identities) or current & seen:
            raise ValueError("Training/development/initial-test identity overlap")
        seen |= current
        sizes[name] = len(current)
    final = json.loads((root / "final-holdout.json").read_text())
    current = set(final["fresh_holdout_ids"])
    if len(current) != final["fresh_holdout_count"] or current & seen:
        raise ValueError("Final holdout identity overlap or duplicate")
    if final["weights_sha256_before_evaluation"] != training["weights_sha256"]:
        raise ValueError("Final holdout used different weights")
    if validation["weights_sha256"] != training["weights_sha256"]:
        raise ValueError("Validation used different weights")
    print(
        json.dumps(
            {
                "weights_sha256": training["weights_sha256"],
                "split_sizes": sizes,
                "final_holdout": len(current),
                "initial_smoke_test_overlap": final["prior_smoke_initial_test_overlap"],
                "qualification": "Artifact/split integrity; the initial test report is preliminary and the neural initializer remains experimental",
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
