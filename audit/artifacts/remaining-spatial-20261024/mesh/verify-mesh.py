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
            result[name][str(policy)] = {
                "max_band_ild_error_db": max(ild),
                "rms_band_ild_error_db": math.sqrt(sum(v * v for v in ild) / len(ild)),
                "max_wrapped_ipd_error_rad": max(ipd),
                "rms_wrapped_ipd_error_rad": math.sqrt(sum(v * v for v in ipd) / len(ipd)),
            }
    return result


baseline = summary(ROOT / "full-dataset-baseline.json")
candidate = summary(ROOT / "candidate-quality.json")
failures = []
for partition in ("development", "evaluation"):
    for metric, before in baseline[partition]["1"].items():
        after = candidate[partition]["1"][metric]
        # Fixed predeclared numerical tolerance, unrelated to measured scores.
        if after > before + 1e-6:
            failures.append({"partition": partition, "metric": metric, "before": before, "after": after})
result = {"baseline": baseline, "candidate": candidate, "failures": failures, "quality_pass": not failures}
(ROOT / "quality-comparison.json").write_text(json.dumps(result, indent=2) + "\n")
print(json.dumps(result, indent=2))
raise SystemExit(bool(failures))
