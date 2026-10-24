"""Apply the fixed nominal/mismatch/measured/control guards to preserved runs."""

import json
from pathlib import Path
import re

ROOT = Path(__file__).parent
BASE = ROOT.parent / "xtc-feasibility"


def objects(stdout):
    result = []
    for line in stdout.splitlines():
        start = line.find("{")
        if start < 0:
            continue
        try:
            value, _ = json.JSONDecoder().raw_decode(line[start:])
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict) and "measurement" in value:
            result.append(value)
    return result


def load(path):
    return json.loads(path.read_text())


runs = {name: load(ROOT / ("candidate-" + name + ".json")) for name in
        ["nominal", "mismatch", "measured", "fade-control", "bypass-control", "aligned-bypass"]}
not_executed = [name for name, run in runs.items()
                if not re.search(r"running [1-9]\d* tests?", run["stdout"])]
if not_executed:
    print(json.dumps({"status": "prerequisite_or_compile_failure", "tests_not_executed": not_executed,
                      "candidate_acceptance_established": False}, indent=2))
    raise SystemExit(2)
failures = [name + ": test command failed" for name, run in runs.items() if run["exit_code"] != 0]
if "Summary: 17/17 tests passed" not in runs["nominal"]["stdout"]:
    failures.append("unchanged nominal 17/17 targets")
baseline = objects(load(BASE / "baseline-mismatch.json")["stdout"])
candidate = objects(runs["mismatch"]["stdout"])
mismatch = {}
for normalization in [False, True]:
    def selected(rows):
        return [r for r in rows if r["measurement"] == "rendered_xtc_mismatch"
                and r["spectral_normalization"] == normalization]
    before, after = selected(baseline), selected(candidate)
    if len(before) != 600 or len(after) != 600:
        failures.append(f"mismatch row count normalization={normalization}")
        continue
    key = lambda r: (r["frequency_hz"], r["head_x_m"], r["yaw_rad"], r["echo_gain"], r["cue"])
    if {key(r) for r in before} != {key(r) for r in after}:
        failures.append(f"mismatch coordinates normalization={normalization}")
    degraded = sum(r["separation_improvement_db"] < 0 for r in after)
    worst = min(r["separation_improvement_db"] for r in after)
    old_worst = min(r["separation_improvement_db"] for r in before)
    if degraded > 146 or worst < old_worst - 1e-6:
        failures.append(f"mismatch degradation normalization={normalization}")
    mismatch[str(normalization)] = {"cues": len(after), "degraded": degraded,
                                   "worst_improvement_db": worst, "baseline_worst_db": old_worst}

before = [r for r in objects(load(BASE / "baseline-measured.json")["stdout"])
          if r["measurement"] == "measured_kemar_yaw"]
after = [r for r in objects(runs["measured"]["stdout"]) if r["measurement"] == "measured_kemar_yaw"]
measured = {}
if len(before) != 140 or len(after) != 140:
    failures.append("measured 140-cue count")
else:
    key = lambda r: (r["normalization"], r["frequency_hz"], r["yaw_deg"], r["cue"])
    old = {key(r): r for r in before}
    if set(old) != {key(r) for r in after}:
        failures.append("measured coordinates")
    else:
        degraded = sum(r["separation_improvement_db"] < 0 for r in after)
        worst = min(r["separation_improvement_db"] for r in after)
        old_worst = min(r["separation_improvement_db"] for r in before)
        minimum_gain_delta = min(r["desired_gain_db"] - old[key(r)]["desired_gain_db"] for r in after)
        if degraded > 4 or worst < old_worst - 1e-6 or minimum_gain_delta < -1e-6:
            failures.append("measured degradation/gain")
        measured = {"cues": 140, "degraded": degraded, "worst_improvement_db": worst,
                    "baseline_worst_db": old_worst, "minimum_desired_gain_delta_db": minimum_gain_delta}
result = {"candidate_acceptance_established": not failures, "failures": failures,
          "mismatch": mismatch, "measured": measured}
(ROOT / "acceptance.json").write_text(json.dumps(result, indent=2) + "\n")
print(json.dumps(result, indent=2))
raise SystemExit(bool(failures))
