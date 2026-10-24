"""Recompute fixed partition quality guards from preserved complete raw runs."""

import json
import math
from pathlib import Path
import re

ROOT = Path(__file__).parent


def summary(path):
    raw = json.loads(path.read_text())
    assert raw["exit_code"] == 0
    rows = {}
    for line in raw["stdout"].splitlines():
        if "MEASURED_KEMAR_HELD_OUT index=" in line:
            match = re.search(
                r"index=(\d+).*policy=(\d+) measured_ild_db=(\S+) predicted_ild_db=(\S+)",
                line,
            )
            index, policy, measured, predicted = match.groups()
            rows.setdefault((int(index), int(policy)), {})["ild"] = abs(
                float(predicted) - float(measured)
            )
        if "MEASURED_KEMAR_FREQUENCY index=" in line:
            match = re.search(
                r"index=(\d+) policy=(\d+) wrapped_ipd_errors_rad=(\[.*?\])", line
            )
            index, policy, errors = match.groups()
            rows.setdefault((int(index), int(policy)), {})["ipd"] = json.loads(errors)
        if "MEASURED_KEMAR_TIME index=" in line:
            match = re.search(
                r"index=(\d+) policy=(\d+) pre_onset_energy_fraction=(\[.*?\]) beyond_measured_support_energy_fraction=(\[.*?\])", line
            )
            index, policy, early, late = match.groups()
            row = rows.setdefault((int(index), int(policy)), {})
            row["early"] = json.loads(early)
            row["late"] = json.loads(late)
    assert len(rows) == 1420
    assert all("ild" in row and len(row["ipd"]) == 7 for row in rows.values())
    development = set(range(0, 710, 17)) | {709}
    result = {}
    for name, indices in [
        ("development", development),
        ("evaluation", set(range(710)) - development),
        ("complete", set(range(710))),
    ]:
        result[name] = {}
        for policy in (0, 1):
            ild = [rows[index, policy]["ild"] for index in sorted(indices)]
            ipd = [v for index in sorted(indices) for v in rows[index, policy]["ipd"]]
            assert all(math.isfinite(v) for v in ild + ipd)
            early = [v for index in sorted(indices) for v in rows[index, policy]["early"]]
            late = [v for index in sorted(indices) for v in rows[index, policy]["late"]]
            assert all(0 <= v <= 1 for v in early + late)
            result[name][str(policy)] = {
                "max_pre_onset_energy_fraction": max(early),
                "rms_pre_onset_energy_fraction": math.sqrt(sum(v*v for v in early)/len(early)),
                "max_beyond_support_energy_fraction": max(late),
                "rms_beyond_support_energy_fraction": math.sqrt(sum(v*v for v in late)/len(late)),
                "max_band_ild_error_db": max(ild),
                "rms_band_ild_error_db": math.sqrt(sum(v * v for v in ild) / len(ild)),
                "max_wrapped_ipd_error_rad": max(ipd),
                "rms_wrapped_ipd_error_rad": math.sqrt(sum(v * v for v in ipd) / len(ipd)),
            }
    return result


baseline = summary(ROOT / "baseline.json")
candidate = summary(ROOT / "candidate.json")
failures = []
for partition in ("development", "evaluation"):
    for metric, before in baseline[partition]["1"].items():
        after = candidate[partition]["1"][metric]
        # Fixed predeclared numerical tolerance, unrelated to measured scores.
        if after > before + 1e-6:
            failures.append({"partition": partition, "metric": metric, "before": before, "after": after})
raw_candidate = json.loads((ROOT / "candidate.json").read_text())["stdout"]
phase_rows = re.findall(r"MEASURED_KEMAR_PHASE_COMPARE index=(\d+) maximum_per_ear_phase_difference_rad=(\S+)", raw_candidate)
assert {int(index) for index, _ in phase_rows} == set(range(710))
maximum_phase_difference = max(float(value) for _, value in phase_rows)
assert maximum_phase_difference <= 1e-6
result = {"baseline": baseline, "candidate": candidate, "maximum_preserved_phase_difference_rad": maximum_phase_difference, "failures": failures, "quality_pass": not failures}
(ROOT / "quality-comparison.json").write_text(json.dumps(result, indent=2) + "\n")
print(json.dumps(result, indent=2))
raise SystemExit(bool(failures))
