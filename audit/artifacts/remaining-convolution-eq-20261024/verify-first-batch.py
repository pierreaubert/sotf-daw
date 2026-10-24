"""Replay first-batch numerical, source and unchanged full-workload evidence."""
from pathlib import Path
import hashlib
import itertools
import json
import re

ART = Path(__file__).resolve().parent
ROOT = ART.parents[2]
HIST = ROOT / "audit/artifacts/plugin-audit-20261018"
read = lambda p: json.loads(p.read_text())
lines = lambda p: [json.loads(x) for x in p.read_text().splitlines() if x.strip()]
source = read(ART / "first-batch-source.json")
assert source["hashes_unchanged_after_load"]
for path, snapshot in source["snapshot_paths"].items():
    assert hashlib.sha256((ROOT / snapshot).read_bytes()).hexdigest() == source["snapshot_sha256"][path]
    if path in source["source_sha256"]:
        assert source["source_sha256"][path] == source["snapshot_sha256"][path]
checks = read(ART / "combined-validation.json")
assert len(checks) == 3 and all(x["exit_code"] == 0 for x in checks)
totals = [tuple(map(int, m)) for m in re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored", checks[0]["stdout"])]
assert tuple(sum(r[i] for r in totals) for i in range(3)) == (273, 0, 8)
assert "prepared_unit_delay_preserves_samples_and_reset_through_arbitrary_callbacks ... ok" in checks[0]["stdout"]
for name, cases in [("nupc_real_direct_reference",54),("nupc_large_sparse_reference",4),("nupc_coalesced_unmerged",45),("nupc_phase_f64",36)]:
    match = re.search(name + r" cases=(\d+) maximum_error=([0-9.e+-]+)", checks[0]["stdout"])
    assert match and int(match[1]) == cases and float(match[2]) < 2e-5
prebuild = read(ART / "prebuild-validation.json")
assert prebuild[0]["exit_code"] == 101 and "magnitude error -0.246297" in prebuild[0]["stderr"]
assert prebuild[1]["exit_code"] == 0
assert read(ART / "fir-m1-fixed-support-witness-recheck.json")["exit_code"] == 0
commands = read(ART / "first-batch-commands.json")
assert [x["exit_code"] for x in commands] == [1,1,0,0]
old_eq = [x["row"] for x in read(HIST / "nupc-coalesced-linear-eq-load.json")["records"] if "row" in x]
eq_data = lines(ART / "first-batch-qa-dynamic-linear-phase-eq.stdout")
eq = [x["row"] for x in eq_data if "row" in x]
assert len(eq) == len(old_eq) == 108
key = lambda r: tuple(r[x] for x in ["rate_hz","channels","stages","phase","callback_frames"])
expected = set(itertools.product([48000,96000,192000],[2,8,12],[1,2,10],["linear","minimum"],[128,512]))
assert {key(x) for x in eq} == expected
old_map = {key(x):x for x in old_eq}
slots = ["steady_before","priming_or_acceptance","blend","steady_after"]
callbacks = misses = failed = 0
for row in eq:
    old = old_map[key(row)]
    for field in ["taps_per_stage","reported_quantum_frames","physical_budget_us","reported_budget_us"]:
        assert row[field] == old[field]
    assert row["taps_per_stage"] == 8192 and row["accepted_generation"] == 1
    assert row["allocations"] == row["frees"] == 0
    assert row["finite_output"] and row["replay_bound_ok"] and row["settled"]
    case_misses = 0
    for slot in slots:
        phase = row[slot]
        assert phase["callbacks"] == old[slot]["callbacks"]
        assert phase["physical_misses"] == phase["reported_misses"]
        assert 0 <= phase["physical_misses"] <= phase["callbacks"]
        callbacks += phase["callbacks"]; case_misses += phase["physical_misses"]
    misses += case_misses; failed += bool(case_misses)
assert callbacks == 32958
assert eq_data[-1]["summary"]["deadline_margin"] == 1.0
assert (failed,misses) == (eq_data[-1]["summary"]["failed_rows"],eq_data[-1]["summary"]["physical_misses"])
eq_result = {"rows":108,"callbacks":callbacks,"failed_rows":failed,"physical_misses":misses,"full_gate_passed":failed == 0}
old_conv = [r for r in read(HIST / "head-fft-selection-load.json")["rows"] if r["event"] == "case_result"]
conv_data = lines(ART / "first-batch-qa-convolution-load.stdout")
conv = [r for r in conv_data if r["event"] == "case_result"]
assert len(conv) == len(old_conv) == 63
key = lambda r: json.dumps(r["geometry"],sort_keys=True)
old_map = {key(r):r for r in old_conv}
assert {key(r) for r in conv} == set(old_map)
callbacks = misses = failed = head_misses = 0
for row in conv:
    old = old_map[key(row)]
    for field in ["ir_frames","estimate_bytes","latency_frames","next_frame_rejection","legacy_largest_period_frames","actual_largest_period_frames","measured_largest_period_frames"]:
        assert row[field] == old[field], (key(row),field)
    assert row["retained_delta_within_estimate"]
    assert row["construction_retained_requested_delta_bytes"] <= row["estimate_bytes"]
    case_misses = 0
    for phase, prior in zip([row["warm"],*row["steady"]],[old["warm"],*old["steady"]]):
        for field in ["callbacks","callback_frames","physical_budget_us","reported_budget_us","reported_quantum_frames"]:
            assert phase[field] == prior[field]
        assert phase["allocations"] == phase["frees"] == 0 and phase["finite"]
        assert 0 <= phase["reported_misses"] <= phase["physical_misses"] <= phase["callbacks"]
        callbacks += phase["callbacks"]; case_misses += phase["physical_misses"]
    misses += case_misses; failed += bool(case_misses)
    if row["geometry"]["head"]: head_misses += case_misses
assert callbacks == 562686
summary = conv_data[-1]
assert summary["errors"] == 0 and summary["timing_margin"] == 1.0
assert (failed,misses) == (summary["failed_cases"],summary["physical_misses"])
assert summary["pass"] == (failed == 0)
assert "Physical callback misses: 0." in (ART / "first-batch-qa-convolution.stdout").read_text()
conv_result = {"cases":63,"callbacks":callbacks,"failed_cases":failed,"physical_misses":misses,"head_misses":head_misses,"full_gate_passed":failed == 0}
result = {"tests_passed":273,"tests_ignored":8,"clippy_format_passed":True,"all_numerical_heap_replay_invariants_passed":True,"fir_m1_passed":False,"linear_eq":eq_result,"convolution":conv_result,"scope":"Production edits numerically verified; full capacity and FIR-M1 remain failing. Historical timing comparisons are observational; retention of overlap/UPC performance changes awaits controlled paired profiling."}
(ART / "first-batch-analysis.json").write_text(json.dumps(result,indent=2)+"\n")
print(json.dumps(result,indent=2))
