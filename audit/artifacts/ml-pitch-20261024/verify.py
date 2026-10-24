#!/usr/bin/env python3
"""Verify this ML checkpoint without claiming full audit acceptance."""

import hashlib
import json
from pathlib import Path


def read(path):
    return json.loads(path.read_text())


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


root = Path(__file__).resolve().parents[3]
archive = Path(__file__).resolve().parent
for name, expected in read(archive / "source-manifest.json").items():
    assert digest(root / name) == expected, name
for name in ["release-tests", "default-integration", "engine-check", "strict-default-qa"]:
    assert read(archive / (name + ".json"))["exit_code"] == 0, name
crepe = root / "crates/sotf-plugins/models/crepe-tiny"
manifest = read(crepe / "manifest.json")
assert digest(crepe / "crepe-tiny.onnx") == manifest["sha256"]
assert digest(root / "scripts/ml/export_crepe.py") == manifest["export_script_sha256"]
assert digest(crepe / "parity.json") == manifest["parity_sha256"]
eq = root / "crates/sotf-plugins/models/eq-single-peak"
report = read(eq / "training-report.json")
assert digest(eq / "weights.json") == report["weights_sha256"]
assert digest(eq / "best.pt") == report["checkpoint_sha256"]
assert digest(eq / "data-manifest.json") == report["data_manifest_sha256"]
data = read(eq / "data-manifest.json")
assert digest(eq / "recipe.py") == data["script_sha256"]
identities = {name: set(values) for name, values in data["split_ids"].items()}
assert not identities["train"] & identities["validation"]
assert not identities["train"] & identities["test"]
assert not identities["validation"] & identities["test"]
eq_archive = root / "audit/artifacts/ml-eq-20261024"
for name in ["native-tests", "strict-native-eq"]:
    assert read(eq_archive / (name + ".json"))["exit_code"] == 0, name
assert digest(eq_archive / "native-proposal.json") == digest(
    root / "crates/sotf-plugins/crates/sotf-plugin-eq/tests/data/ml-single-peak-proposal.json"
)
print("PASS: frozen source/model/recipe hashes, disjoint EQ splits, native rendering and recorded checks.")
print("Full ML program, held-out quality, native UI and real-DAW acceptance remain incomplete.")
