"""Check radial-damping corner coverage, raw decisions, and retained failures."""
import json
import struct
from pathlib import Path
ROOT = Path(__file__).resolve().parent
def read(name):
    return [json.loads(line) for line in (ROOT / name).read_text().splitlines()]
old = read("aae-neutral-loop-clock-corners.jsonl")
new = read("aae-radial-damping-final-corners.jsonl")
fields = ["rate", "room", "rt60", "bass", "treble", "modulation", "excitation", "drive_frames", "neutral_control", "frames"]
def key(row):
    return tuple(row[field] for field in fields)
def f32(value):
    return struct.unpack("f", struct.pack("f", value))[0]
previous = {key(row): row for row in old[:-1]}
rows, summary = new[:-1], new[-1]
assert len(rows) == len(previous) == summary["rows"] == 208
assert len({key(row) for row in rows}) == 208
assert {key(row) for row in rows} == previous.keys()
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
regressions = [row for row in rows if previous[key(row)]["passed"] and not row["passed"]]
fixed = [row for row in rows if not previous[key(row)]["passed"] and row["passed"]]
assert summary["failed"] == len(failed) and summary["passed"] == (not failed)
assert not regressions
# Equal-gain attenuation bypasses radial redesign and must remain bit-identical.
assert all(row == previous[key(row)] for row in rows if row["neutral_control"])
assert all(row["finite"] for row in rows)
print(json.dumps({"rows":208,"configuration_keys_preserved":True,"neutral_controls_exactly_preserved":True,"failed":len(failed),"fixed":len(fixed),"regressions":len(regressions),"fit_failures":sum(not row["fit_ok"] for row in rows),"target_failures":sum(not row["target_ok"] for row in rows),"all_finite":True,"max_late_energy_ratio":max(row["last_to_first_quarter_energy"] for row in rows),"minimum_wide_r2":min(row["fits"][0]["r2"] for row in rows),"fixed_rows":fixed,"failed_rows":failed,"quality_gate_passed":not failed},indent=2))
