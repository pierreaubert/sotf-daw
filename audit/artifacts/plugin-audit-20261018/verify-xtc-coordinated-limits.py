"""Verify limiter invariants, retained failures and every rendered mismatch row."""
import hashlib
import itertools
import json
import math
import re
from pathlib import Path
ROOT = Path(__file__).resolve().parent
baseline = json.loads((ROOT / "xtc-coordinated-limits-baseline.json").read_text())
assert baseline["exit_code"] == 101
assert "columns=1944 limited_columns=1432 failed_columns=1388" in baseline["stdout"]
assert "columns=72 failed_columns=48 budget_failures=24" in baseline["stdout"]
validation = json.loads((ROOT / "xtc-coordinated-limits-validation.json").read_text())
assert len(validation) == 3 and all(r["exit_code"] == 0 for r in validation)
assert [r["command"][3] for r in validation] == ["test", "clippy", "run"]
output = validation[0]["stdout"]
counts = [tuple(map(int, m)) for m in re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored", output)]
assert sum(c[0] for c in counts) == 251 and sum(c[1] for c in counts) == 0 and sum(c[2] for c in counts) == 1, counts
for required in ["columns=1944 limited_columns=1432 failed_columns=0", "columns=72 failed_columns=0 budget_failures=0", "column_limit_curve cases=30735", "pose_mirror cases=216 maximum_error=0", "pose_center_continuity cases=720", "independent_inverse cases=324", "independent_brown_duda_prediction cases=360", "rendered_pose cases=120 maximum_mirror_error=0 maximum_partition_error=0", "lateral_pose_heap cases=60 allocations=0 frees=0 latency_samples=2048 reset_partition_error=0"]:
    assert required in output, required
assert "[ALL PASS] Xtc QA Complete." in validation[2]["stdout"]
nominal = json.loads((ROOT / "xtc-coordinated-limits-nominal-report.json").read_text())
assert nominal["exit_code"] == 0 and "Summary: 16/17 tests passed" in nominal["stdout"]
assert "measured 4.822, tolerance 0.000" in nominal["stdout"]
assert "measured 28.627, tolerance 20.000" in nominal["stdout"]
def mismatch_rows(text):
    result = []
    for line in text.splitlines():
        if line.startswith("{"):
            try: row = json.loads(line)
            except json.JSONDecodeError: continue
            if row.get("measurement") == "rendered_xtc_mismatch": result.append(row)
    return result
keys = lambda r: (r["frequency_hz"], r["head_x_m"], r["yaw_rad"], r["echo_gain"], r["cue"])
rows = mismatch_rows(output)
expected = set(itertools.product([48000*b/2048 for b in [10,22,43,85,171,341]], [-0.1,-0.02,0.0,0.02,0.1], [-0.3,0.0,0.3], [0.0,0.4], [0,1]))
assert len(rows) == 360 and len({keys(r) for r in rows}) == 360 and {keys(r) for r in rows} == expected
assert all(math.isfinite(r[f]) for r in rows for f in ["raw_depth_db", "desired_gain_db", "separation_improvement_db"])
degraded = sum(r["separation_improvement_db"] < 0 for r in rows)
worst = min(r["separation_improvement_db"] for r in rows)
summary = re.search(r"rendered_xtc_summary cases=360 degraded=(\d+) worst_normalized_db=([^ ]+) maximum_partition_error=([^\n]+)", output)
assert summary and int(summary[1]) == degraded and float(summary[2]) == worst and float(summary[3]) == 0
previous_output = json.loads((ROOT / "xtc-source-crossing-validation.json").read_text())[0]["stdout"]
previous = {keys(r): r for r in mismatch_rows(previous_output)}
assert set(previous) == expected
changes = [r["separation_improvement_db"]-previous[keys(r)]["separation_improvement_db"] for r in rows]
change = json.loads((ROOT / "xtc-coordinated-limits-default-change.json").read_text())
folder = ROOT.parents[2] / "crates/sotf-plugins/crates/sotf-plugin-xtc/src/filters"
for name, field in [("centered_continuous_paths_bits.txt", "previous_fixture_sha256"), ("centered_coordinated_limits_bits.txt", "current_fixture_sha256")]:
    assert hashlib.sha256((folder/name).read_bytes()).hexdigest() == change[field]
assert change["frequency_blend_blocks_unchanged"] == 3
print(json.dumps(dict(passed_tests=251, existing_ignored=1, inverse_steering_columns=1944, actively_limited_columns=1432, effort_steering_columns=72, legacy_peak_curve_cases=30735, rendered_pose_cases=120, zero_heap_pose_cases=60, latency_samples=2048, rendered_mismatch_cases=360, degraded_mismatch_cases=degraded, worst_normalized_mismatch_db=worst, previous_degraded_mismatch_cases=sum(r["separation_improvement_db"]<0 for r in previous.values()), minimum_separation_change_db=min(changes), maximum_separation_change_db=max(changes), comparison_resolution_db=1e-6, improved_rows_above_resolution=sum(d>1e-6 for d in changes), worsened_rows_above_resolution=sum(d < -1e-6 for d in changes), default_coefficient_change=change, nominal_report="16/17; 93.75Hz cancellation 4.822dB remains below 5dB", scope="Ray invariance holds through coordinated limits for a fixed linear plant; identity blending, transfer-model mismatch, measured/native/loaded deadlines, reconstructed true peaks and listening remain separate."),indent=2))
