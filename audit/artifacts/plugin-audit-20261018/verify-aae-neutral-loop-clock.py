"""Verify loop-clock evidence coverage without converting failed decay rows to passes."""
import itertools
import json
import re
import struct
from pathlib import Path

ROOT = Path(__file__).resolve().parent
records = [json.loads(line) for line in (ROOT / "aae-neutral-loop-clock-corners.jsonl").read_text().splitlines()]
previous = [json.loads(line) for line in (ROOT / "aae-decay-corners-expanded.jsonl").read_text().splitlines()]
rows, summary = records[:-1], records[-1]
fields = ["rate", "room", "rt60", "bass", "treble", "modulation", "excitation", "drive_frames", "neutral_control", "frames"]
def key(row):
    return tuple(row[field] for field in fields)
def f32(value):
    return struct.unpack("f", struct.pack("f", value))[0]
assert len(rows) == 208 == summary["rows"]
assert len({key(row) for row in rows}) == 208
assert {key(row) for row in rows} == {key(row) for row in previous[:-1]}
for row in rows:
    fit = row["fits"][0]
    target_ok = abs(fit["rt60_s"] / row["rt60"] - 1.0) < 0.10 if row["neutral_control"] else (
        0.9 * f32(row["rt60"] * min(row["bass"], row["treble"])) <= fit["rt60_s"]
        <= 1.1 * f32(row["rt60"] * max(row["bass"], row["treble"]))
    )
    fit_ok = fit["r2"] > 0.98
    passed = row["finite"] and row["peak"] > 0 and row["last_to_first_quarter_energy"] < 1e-6 and target_ok and fit_ok
    assert (row["target_ok"], row["fit_ok"], row["passed"]) == (target_ok, fit_ok, passed)
failed = [row for row in rows if not row["passed"]]
assert summary["failed"] == len(failed)
assert summary["passed"] == (not failed)
assert all(row["finite"] for row in rows)
gates = json.loads((ROOT / "aae-neutral-loop-clock-gates.json").read_text())
assert len(gates) == 3 and all(gate["exit_code"] == 0 for gate in gates)
observations = [line for line in gates[0]["stdout"].splitlines() if line.startswith("neutral_weighting lifecycle=")]
assert len(observations) == 80
identities = {tuple(re.search(name + r"=([^ ]+)", line).group(1) for name in ["lifecycle", "rate", "room", "rt60"]) for line in observations}
assert identities == set(itertools.product(["new", "rt60_settled", "room_settled", "pending_admitted", "reset"], ["44100", "48000", "96000", "192000"], ["0.2", "3"], ["0.3", "6"]))
errors = [float(re.search(r"max_error_over_input=([^ ]+)", line).group(1)) for line in observations]
assert max(errors) < 3e-7
print(json.dumps({"coverage_rows": len(rows), "configuration_keys_preserved": True, "failed_rows": len(failed), "same_failure_keys": {key(row) for row in failed} == {key(row) for row in previous[:-1] if not row["passed"]}, "fit_failures": sum(not row["fit_ok"] for row in rows), "target_failures": sum(not row["target_ok"] for row in rows), "neutral_failures": sum(row["neutral_control"] and not row["passed"] for row in rows), "all_finite": True, "minimum_wide_r2": min(row["fits"][0]["r2"] for row in rows), "max_residual_energy_ratio": max(row["last_to_first_quarter_energy"] for row in rows), "reference_cases": 80, "maximum_error_over_input": max(errors), "quality_gate_passed": not failed}, indent=2))
