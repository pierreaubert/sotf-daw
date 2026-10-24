"""Verify the restored-source feasibility checkpoint and its explicit rejection."""

import hashlib
import json
from pathlib import Path
import re

ROOT = Path(__file__).parent
for name, expected in json.loads((ROOT / "baseline-source.json").read_text()).items():
    assert hashlib.sha256(Path(name).read_bytes()).hexdigest() == expected, name
for name, item in json.loads((ROOT / "rejected-source.json").read_text()).items():
    assert hashlib.sha256(Path(item["snapshot"]).read_bytes()).hexdigest() == item["sha256"], name
    assert item["sha256"] == json.loads((ROOT / "reference-source.json").read_text())[name]
for name in ["robust.rs", "robust_tests.rs", "raw_uncertainty.rs", "raw_uncertainty_tests.rs"]:
    assert not Path("crates/sotf-plugins/crates/sotf-plugin-xtc/src/validation", name).exists()

def run(name):
    item = json.loads((ROOT / name).read_text())
    assert item["exit_code"] == 0, name
    return item

references = run("final-reference.json")
assert "29 passed; 0 failed" in references["stdout"]
clippy = run("reference-clippy.json")
assert "clippy" in clippy["command"] and "-D" in clippy["command"]
raw = run("raw-ratio-reference.json")
match = re.search(r"RAW_RATIO_DISK_BOUNDARY cases=(\d+) maximum_depth_error_db=(\S+)", raw["stdout"])
assert int(match[1]) == 8 and float(match[2]) <= 2e-6
unit = re.search(
    r"BOUNDED_LF_UNIT_CEILING nominal_ceiling=(\S+) maximum_guaranteed_intended=(\S+) requested_new_worst_case_floor=(\S+)",
    references["stdout"],
)
assert float(unit[1]) == 1.0 and float(unit[2]) < float(unit[3])
nominal = run("nominal-correct-filter.json")
assert "Summary: 16/17 tests passed" in nominal["stdout"]
assert "expected 5.000, measured 4.811" in nominal["stdout"]
assert "running 0 tests" in run("baseline-nominal.json")["stdout"]
mismatch = run("baseline-mismatch.json")
rows = [json.loads(line) for line in mismatch["stdout"].splitlines() if line.startswith('{')]
for normalization in (False, True):
    values = [row["separation_improvement_db"] for row in rows
              if row.get("measurement") == "rendered_xtc_mismatch"
              and row["spectral_normalization"] == normalization]
    assert len(values) == 600 and sum(value < 0 for value in values) == 146
    assert min(values) > -5.38209 and min(values) < -5.38208
measured = run("baseline-measured.json")
assert "cues=140 degraded=4 worst_separation_improvement_db=-1.516376941042497" in measured["stdout"]
decision = json.loads((ROOT / "decision.json").read_text())
assert decision["candidate_rejected_as_unchanged_nominal_gain_remedy"]
assert not decision["prototype_wired_to_production"]
assert not decision["original_nominal_LF_or_mismatch_infeasibility_proved"]
assert decision["exploratory_nominal_amplitude_ceiling"] == 1.25
assert abs(decision["maximum_guaranteed_intended_under_unit_ceiling"] - float(unit[2])) < 1e-15
print(json.dumps({
    "current_source_verified": True,
    "production_restored": True,
    "reference_tests_passed": 29,
    "strict_all_target_clippy_passed": True,
    "exploratory_gain_ceiling_rejected": 1.25,
    "new_worst_case_gain_floor_unit_ceiling_infeasible": True,
    "original_nominal_LF_or_mismatch_infeasibility_proved": False,
    "default_nominal_passed": "16/17",
    "degraded_per_600_mismatch_rows": 146,
    "measured_degraded_per_140_cues": 4,
    "all_three_plugin_scopes_remain_partial": True,
}, indent=2))
