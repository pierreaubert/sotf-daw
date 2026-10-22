"""Verify retained red evidence, final checks, fixture provenance and rendered rows."""
import hashlib
import itertools
import json
import math
import re
from pathlib import Path
ROOT = Path(__file__).resolve().parent
baseline = json.loads((ROOT / "xtc-source-crossing-baseline.json").read_text())
assert baseline["exit_code"] == 101
assert "quarter_turn cases=80 failed_values=80" in baseline["stdout"]
validation = json.loads((ROOT / "xtc-source-crossing-validation.json").read_text())
assert len(validation) == 3 and all(r["exit_code"] == 0 for r in validation)
assert [r["command"][3] for r in validation] == ["test", "clippy", "run"]
focused = json.loads((ROOT / "xtc-source-crossing-final-focused.json").read_text())
assert focused["exit_code"] == 0 and "2 passed; 0 failed" in focused["stdout"]
final_inverse = json.loads((ROOT / "xtc-source-crossing-final-inverse-validation.json").read_text())
assert len(final_inverse) == 3 and all(r["exit_code"] == 0 for r in final_inverse)
assert "3 passed; 0 failed" in final_inverse[0]["stdout"]
assert "1 passed; 0 failed" in final_inverse[1]["stdout"]
nominal = json.loads((ROOT / "xtc-source-crossing-nominal-report.json").read_text())
assert nominal["exit_code"] == 0
assert "Summary: 16/17 tests passed" in nominal["stdout"]
assert "measured 28.627, tolerance 20.000" in nominal["stdout"]
assert "measured 4.954, tolerance 0.000" in nominal["stdout"]
output = validation[0]["stdout"]
counts = [tuple(map(int, m)) for m in re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored", output)]
assert counts and sum(c[0] for c in counts) == 247 and sum(c[1] for c in counts) == 0 and sum(c[2] for c in counts) == 1, counts
assert "quarter_turn cases=80 failed_values=0" in output
assert "source_crossings cases=240" in output
assert "pose_geometry cases=102" in output and "failed_values=0" in output
assert "independent_inverse cases=324" in output
assert "independent_brown_duda_prediction cases=360" in output
assert "pose_mirror cases=216 maximum_error=0" in output
assert "pose_center_continuity cases=720" in output
assert "rendered_pose cases=120 maximum_mirror_error=0 maximum_partition_error=0" in output
rows = []
for line in output.splitlines():
    if line.startswith("{"):
        try: value = json.loads(line)
        except json.JSONDecodeError: continue
        if value.get("measurement") == "rendered_xtc_mismatch": rows.append(value)
expected = set(itertools.product([48000*b/2048 for b in [10,22,43,85,171,341]], [-0.1,-0.02,0.0,0.02,0.1], [-0.3,0.0,0.3], [0.0,0.4], [0,1]))
keys = [(r["frequency_hz"],r["head_x_m"],r["yaw_rad"],r["echo_gain"],r["cue"]) for r in rows]
assert len(keys) == 360 and len(set(keys)) == 360 and set(keys) == expected
assert all(math.isfinite(r[f]) for r in rows for f in ["raw_depth_db","desired_gain_db","separation_improvement_db"])
degraded = sum(r["separation_improvement_db"] < 0 for r in rows)
worst = min(r["separation_improvement_db"] for r in rows)
summary = re.search(r"rendered_xtc_summary cases=360 degraded=(\d+) worst_normalized_db=([^ ]+) maximum_partition_error=([^\n]+)", output)
assert summary and int(summary[1]) == degraded and float(summary[2]) == worst and float(summary[3]) == 0
change = json.loads((ROOT / "xtc-source-crossing-default-change.json").read_text())
repository = ROOT.parents[2]
folder = repository / "crates/sotf-plugins/crates/sotf-plugin-xtc/src/filters"
for name,field in [("centered_default_bits.txt","old_fixture_sha256"),("centered_continuous_paths_bits.txt","new_fixture_sha256")]:
    assert hashlib.sha256((folder/name).read_bytes()).hexdigest() == change[field]
print(json.dumps(dict(passed_tests=sum(c[0] for c in counts), final_inverse_regressions=4, final_focused_crossing_tests=2, nominal_report_passed_comparisons=16, nominal_report_total_comparisons=17, existing_ignored=sum(c[2] for c in counts), quarter_turn_comparisons=80, median_crossing_cases=240, independent_geometry_poses=102, independent_inverse_cases=324, independent_brown_duda_predictions=360, mirror_cases=216, center_continuity_cases=720, rendered_pose_cases=120, rendered_mismatch_cases=360, degraded_mismatch_cases=degraded, worst_normalized_mismatch_db=worst, default_coefficient_change=change, scope="Controlled analytical/rendered tests; measured HRTF/room robustness, finite-distance ear-surface paths, native deadlines, reconstructed true peaks and listening acceptance remain open."),indent=2))
