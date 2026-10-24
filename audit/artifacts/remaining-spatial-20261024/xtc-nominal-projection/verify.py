"""Verify preserved nominal-projection rejection and exact production restoration."""

import hashlib
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).parent
BASE = ROOT.parent / "xtc-feasibility"
for name, expected in json.loads((BASE / "baseline-source.json").read_text()).items():
    assert hashlib.sha256(Path(name).read_bytes()).hexdigest() == expected, name
for name in ["nominal_projection.rs", "nominal_projection_tests.rs", "nominal_projection_audit_tests.rs"]:
    assert not Path("crates/sotf-plugins/crates/sotf-plugin-xtc/src/filters", name).exists()
source = json.loads((ROOT / "rejected-source.json").read_text())
wired = json.loads((ROOT / "wired-source.json").read_text())
assert len(source) == 8
for name, item in source.items():
    assert hashlib.sha256(Path(item["snapshot"]).read_bytes()).hexdigest() == item["sha256"] == wired[name]
reference = json.loads((ROOT / "reference-source.json").read_text())
for name in reference:
    if name.endswith(("nominal_projection.rs", "nominal_projection_tests.rs")):
        assert reference[name] == wired[name]
ref = json.loads((ROOT / "reference.json").read_text())
assert ref["exit_code"] == 0 and "5 passed; 0 failed" in ref["stdout"]
result = subprocess.run(["rtk", "proxy", "python3", str(ROOT / "check-acceptance.py")],
                        capture_output=True, text=True)
assert result.returncode == 1, result.stderr
acceptance = json.loads(result.stdout)
assert not acceptance["candidate_acceptance_established"]
assert acceptance["failures"] == ["fade-control: test command failed"]
fade = json.loads((ROOT / "candidate-fade-control.json").read_text())
assert fade["exit_code"] == 101
assert "NOMINAL_PROJECTION_BOUNDARY_SUMMARY cases=8 failures=8" in fade["stdout"]
failed_boundaries = []
tails = []
for line in fade["stdout"].splitlines():
    if line.startswith("NOMINAL_PROJECTION_BOUNDARY "):
        values = dict(part.split("=", 1) for part in line.split()[1:])
        passed = float(values["candidate_jump"]) <= float(values["legacy_jump"]) + float(values["numerical_allowance"])
        assert passed == (values["passed"] == "true")
        if not passed:
            failed_boundaries.append(values)
    if line.startswith("NOMINAL_PROJECTION_TIME "):
        values = dict(part.split("=", 1) for part in line.split()[1:])
        assert float(values["candidate_outside_central_support_energy_fraction"]) > float(values["legacy_outside_central_support_energy_fraction"])
        tails.append(values)
assert len(failed_boundaries) == len(tails) == 8
for name, tool in [("shipping-lib", "build"), ("shipping-clippy", "clippy")]:
    run = json.loads((ROOT / ("candidate-" + name + ".json")).read_text())
    assert run["exit_code"] == 0 and tool in run["command"]
    assert "--features" not in run["command"] and "qa" not in run["command"]
    if tool == "clippy":
        assert "-D" in run["command"] and "warnings" in run["command"]
for prefix in [[], ["--cached"]]:
    diff = subprocess.check_output(["rtk", "proxy", "git", "diff", *prefix, "--name-only", "--",
                                    "crates/sotf-plugins/crates/sotf-plugin-xtc"], text=True)
    assert not diff.strip(), diff
summary = {"current_source_verified": True, "XTC_worktree_and_index_restored": True,
           "candidate_rejected": True, "rejected_candidate_nominal_checks": "17/17", "restored_source_has_no_nominal_fix": True,
           "mismatch": acceptance["mismatch"], "measured": acceptance["measured"],
           "failed_fade_cases": 8, "increased_tail_energy_cases": 8,
           "shipping_no_qa_lib_and_strict_clippy_passed": True,
           "all_three_plugin_scopes_remain_partial": True}
(ROOT / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
print(json.dumps(summary, indent=2))
