"""Replay the predeclared symmetric scalar/span performance and control gates."""
from pathlib import Path
import hashlib
import json
import math
import statistics
import subprocess

ART = Path(__file__).resolve().parent
ROOT = ART.parents[2]
read = lambda p: json.loads(p.read_text())
subprocess.run(["rtk","proxy","python3",str(ART / "verify-span-preflight.py")],cwd=ROOT,capture_output=True,text=True,check=True)
plan = read(ART / "paired-plan.json")
source = read(ART / "paired-measurement-source.json")
assert source["hashes_unchanged_after_measurement"]
assert source["source_sha256"] == plan["source_sha256"]
assert source["plan_sha256"] == hashlib.sha256((ART / "paired-plan.json").read_bytes()).hexdigest()
setup = read(ART / "fixture-setup-validation.json")
assert setup["exit_code"] == 0 and setup["stdout"].count("SPAN_SETUP ") == 48
assert setup["binary_sha256"] in source["binary_sha256"].values()
commands = read(ART / "paired-commands.json")
assert len(commands) == 1 and commands[0]["exit_code"] == 0
rows = [json.loads(x.split("SPAN_PAIRED ",1)[1]) for x in (ART / "paired.stdout").read_text().splitlines() if "SPAN_PAIRED " in x]
assert len(rows) == 192
planned = {c["name"]:c for c in plan["cases"]}
assert len(planned) == 48
observed = {}
for row in rows: observed.setdefault(row["case"],[]).append(row)
assert set(observed) == set(planned)
comparisons = []
for name,group in observed.items():
    case = planned[name]
    assert [r["trial"] for r in group] == [0,1,2,3]
    assert [r["candidate"] for r in group] == [False,True,True,False]
    for row in group:
        assert row["rate"] == case["rate"] and row["control"] == case["control"]
        assert row["warm"] == case["warm"] and row["calls"] == case["calls"]
        assert row["function"] == ("common_scalar" if case["control"] else "span" if row["candidate"] else "scalar")
        assert len(row["durations_ns"]) == case["calls"]
        assert all(x >= 0 for x in row["durations_ns"])
        assert sum(row["durations_ns"]) == row["total_ns"]
        assert 0 <= row["max_error"] <= plan["screen"]["waveform_max_error"]
        assert math.isfinite(row["checksum"])
        if case["control"] or not row["candidate"]: assert row["max_error"] == 0
    before = statistics.median(r["total_ns"] for r in group if not r["candidate"])
    after = statistics.median(r["total_ns"] for r in group if r["candidate"])
    comparisons.append({"case":name,"rate":case["rate"],"control":case["control"],"scalar_median_total_ns":before,"span_median_total_ns":after,"ratio":after/before,"maximum_error":max(r["max_error"] for r in group)})
changed = [r for r in comparisons if not r["control"]]
controls = [r for r in comparisons if r["control"]]
assert len(changed) == 42 and len(controls) == 6
ratio = sum(r["span_median_total_ns"] for r in changed) / sum(r["scalar_median_total_ns"] for r in changed)
screen = plan["screen"]
passed = ratio <= screen["changed_aggregate_ratio_max"] and all(r["ratio"] <= screen["changed_case_ratio_max"] for r in changed) and all(screen["control_ratio_min"] <= r["ratio"] <= screen["control_ratio_max"] for r in controls)
result = {"comparators_verified":True,"screen_passed":passed,"changed_aggregate_ratio":ratio,"largest_changed_ratio":max(r["ratio"] for r in changed),"control_ratio_min":min(r["ratio"] for r in controls),"control_ratio_max":max(r["ratio"] for r in controls),"maximum_waveform_error":max(r["maximum_error"] for r in comparisons),"callbacks":sum(r["calls"] for r in rows),"comparisons":comparisons,"scope":plan["scope"]}
(ART / "paired-analysis.json").write_text(json.dumps(result,indent=2)+"\n")
print(json.dumps({k:v for k,v in result.items() if k != "comparisons"},indent=2))
