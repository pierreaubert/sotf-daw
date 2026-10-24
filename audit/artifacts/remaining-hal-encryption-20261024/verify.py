#!/usr/bin/env python3
"""Verify archived HAL ownership sources and retained validation outcomes."""
import hashlib
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
source = json.loads((HERE / "source-checkpoint.json").read_text())
for variant, files in source.items():
    for name, expected in files.items():
        frozen = HERE / (variant + "-" + Path(name).name + ".txt")
        assert hashlib.sha256(frozen.read_bytes()).hexdigest() == expected, frozen
rows = json.loads((HERE / "full-validation.json").read_text())
assert len(rows) == 4
assert all(row["exit_code"] == 0 for row in rows)
for variant, count in [("clean", 122), ("current", 137)]:
    row = next(row for row in rows if row["variant"] == variant and row["kind"] == "unit")
    assert f"{count} passed; 0 failed" in row["stdout"]
for name in ["clean-io-baseline.json", "dirty-field-baseline.json"]:
    row = json.loads((HERE / name).read_text())
    assert row["exit_code"] != 0, name
for kind in ["input", "output"]:
    row = json.loads((HERE / f"native-{kind}.json").read_text())
    assert row["exit_code"] == 0, kind
    assert row["source_before"] == row["source_after"], kind
    assert "32 native" in row["stdout"] or row["stdout"].count(f"native_hal_{kind} rate=") == 32, kind
print("Archived source hashes, baseline failures, driver suites and isolated native targets verified.")
