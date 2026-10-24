"""Verify retry sources, unchanged full workload, and original acceptance guards."""
import hashlib
import itertools
import json
import math
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent
REPO = ROOT.parents[2]
def read(name):
    return json.loads((ROOT / name).read_text())

key = lambda row: tuple(row["geometry"][field] for field in ["rate_hz", "output_channels", "ir_channels", "true_stereo", "nupc", "head"])
expected = {(rate, out, ir, matrix, nupc, head) for rate, (out, ir, matrix), (nupc, head) in itertools.product([48000,96000,192000], [(2,1,False),(2,2,False),(12,1,False),(12,12,False),(32,1,False),(32,32,False),(2,4,True)], [(False,False),(True,False),(True,True)])}

def largest(length, base):
    if length == 0: return 1
    period, coverage = base, 4 * base
    while coverage < length:
        period *= 2
        coverage += 4 * period
    return period

def load(data, trimmed):
    rows = [row for row in data["rows"] if row["event"] == "case_result"]
    assert len(rows) == 63 and {key(row) for row in rows} == expected
    assert not any(row["event"] == "case_error" for row in data["rows"])
    failed = misses = callbacks = 0
    for row in rows:
        assert row["retained_delta_within_estimate"] and row["construction_retained_requested_delta_bytes"] <= row["estimate_bytes"]
        g, length = row["geometry"], row["ir_frames"]
        head = min(257, length) if g["head"] else 0
        base = min(1024, head) if head else 1024
        legacy = largest(length - head, base) if g["nupc"] else 1024
        normal_base = 1 << (base.bit_length() - 1) if head else base
        baseline = largest(length - head, normal_base) if g["nupc"] else 1024
        actual = baseline
        # A standalone last level is the only group at its size when its
        # entire remaining span is shorter than one block. Earlier levels
        # cover 4 * (largest-base) frames of the tail before that level.
        if trimmed and head and length > head:
            remaining = length - head - 4 * (baseline - normal_base)
            if remaining < baseline:
                last = max(normal_base, 1 << (remaining - 1).bit_length())
                actual = max(baseline // 2 if baseline > normal_base else normal_base, last)
        period = max(legacy, baseline, actual)
        assert row["legacy_largest_period_frames"] == legacy
        assert row["actual_largest_period_frames"] == actual
        assert row["measured_largest_period_frames"] == period
        if trimmed: assert row["baseline_largest_period_frames"] == baseline
        warm, steady = row["warm"], row["steady"]
        assert warm["callback_frames"] == 128 and warm["callbacks"] == (length + row["latency_frames"] + 37 + 127) // 128
        assert [p["callback_frames"] for p in steady] == [128,512]
        for phase in steady:
            assert phase["callbacks"] == max(256, (4 * period + phase["callback_frames"] - 1) // phase["callback_frames"])
            assert phase["callbacks"] * phase["callback_frames"] >= 4 * actual
        phases = [warm, *steady]
        for phase in phases:
            frames = phase["callback_frames"]
            assert math.isclose(phase["physical_budget_us"], frames / g["rate_hz"] * 1e6, rel_tol=1e-12)
            assert math.isclose(phase["reported_budget_us"], max(frames, phase["reported_quantum_frames"]) / g["rate_hz"] * 1e6, rel_tol=1e-12)
            assert phase["allocations"] == phase["frees"] == 0 and phase["finite"]
            assert 0 <= phase["reported_misses"] <= phase["physical_misses"] <= phase["callbacks"]
            assert 0 <= phase["p50_us"] <= phase["p95_us"] <= phase["p99_us"] <= phase["max_us"]
            misses += phase["physical_misses"]
            callbacks += phase["callbacks"]
        failed += int(any(p["physical_misses"] or p["reported_misses"] for p in phases))
    summaries = [row for row in data["rows"] if row["event"] == "summary"]
    assert len(summaries) == 1
    summary = summaries[0]
    assert summary["cases"] == 63 and summary["errors"] == 0 and summary["failed_cases"] == failed
    assert summary["physical_misses"] == misses and summary["timing_margin"] == 1.0
    assert summary["pass"] == (failed == 0) and data["exit_code"] == (1 if failed else 0)
    assert callbacks == 562686
    return {key(row): row for row in rows}, dict(cases=63, callbacks=callbacks, failed_cases=failed, physical_misses=misses, deadline_pass=summary["pass"])


source = read("final-tail-retry-source.json")
assert source["hashes_unchanged_after_load"]
assert source["candidate_rejected"] and source["production_dsp_restored_to"] == "eab4937"
snapshots = read("final-tail-source.json")["candidate_snapshot_paths"]
module = '\n#[cfg(test)]\n#[path = "final_tail_cost.rs"]\nmod final_tail_cost;\n'
def measured_bytes(path):
    content = (REPO / snapshots.get(path, path)).read_bytes()
    if path.endswith("nupc_engine.rs"):
        content += module.encode()
    return content
for path, sha in source["source_sha256"].items():
    assert hashlib.sha256(measured_bytes(path)).hexdigest() == sha, path
for path, sha in read("final-tail-source.json")["source_sha256"].items():
    content = measured_bytes(path)
    if path.endswith("nupc_engine.rs"):
        content = content.removesuffix(module.encode())
    assert hashlib.sha256(content).hexdigest() == sha, path
for path, sha in read("final-tail-source.json")["restored_baseline_sha256"].items():
    assert hashlib.sha256((REPO / path).read_bytes().removesuffix(module.encode())).hexdigest() == sha, path
assert not (REPO / "crates/sotf-plugins/crates/plugins-spatial/src/nupc/final_tail_tests.rs").exists()
validation = read("final-tail-retry-validation.json")
assert len(validation) == 4 and all(x["exit_code"] == 0 for x in validation)
totals = [tuple(map(int, m)) for m in re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored", validation[0]["stdout"])]
assert tuple(sum(r[i] for r in totals) for i in range(3)) == (273, 0, 8)
assert "Physical callback misses: 0." in validation[2]["stdout"]
assert read("final-tail-paired-analysis.json")["screen_passed"]
old = read("head-fft-selection-load.json")
new = read("final-tail-retry-load.json")
assert [json.loads(x) for x in (ROOT / "final-tail-retry-load.jsonl").read_text().splitlines() if x.strip()] == new["rows"]
a, before = load(old, False)
b, after = load(new, True)
count = lambda r: sum(p["physical_misses"] for p in [r["warm"], *r["steady"]])
comparisons = []
for key_, row in b.items():
    prior = a[key_]
    assert row["geometry"] == prior["geometry"]
    for field in ["ir_frames", "estimate_bytes", "latency_frames", "next_frame_rejection", "measured_largest_period_frames"]:
        assert row[field] == prior[field], (key_, field)
    for x, y in zip([row["warm"], *row["steady"]], [prior["warm"], *prior["steady"]]):
        for field in ["callback_frames", "callbacks", "reported_quantum_frames", "physical_budget_us", "reported_budget_us"]:
            assert x[field] == y[field], (key_, field)
    comparisons.append({"geometry":row["geometry"],"old_misses":count(prior),"new_misses":count(row),"old_retained_bytes":prior["construction_retained_requested_delta_bytes"],"new_retained_bytes":row["construction_retained_requested_delta_bytes"],"old_period":prior["actual_largest_period_frames"],"new_period":row["actual_largest_period_frames"]})
heads = [r for r in comparisons if r["geometry"]["head"]]
head_before = sum(r["old_misses"] for r in heads)
head_after = sum(r["new_misses"] for r in heads)
retained_before = sum(r["old_retained_bytes"] for r in heads)
retained_after = sum(r["new_retained_bytes"] for r in heads)
guard = head_after < head_before and retained_after < retained_before
assert not guard
replayed = read("final-tail-retry-replayed-acceptance.json")
assert replayed["exit_code"] == 1
values = json.loads(replayed["stdout"])
assert values["head_before"] == head_before and values["head_after"] == head_after
assert values["retained_before"] == retained_before and values["retained_after"] == retained_after
assert not values["head_miss_guard_passed"] and values["memory_guard_passed"]
restored = read("final-tail-retry-restored-validation.json")
assert len(restored) == 4 and all(x["exit_code"] == 0 for x in restored)
restored_totals = [tuple(map(int, m)) for m in re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored", restored[0]["stdout"])]
assert tuple(sum(r[i] for r in restored_totals) for i in range(3)) == (271, 0, 8)
assert "Physical callback misses: 0." in restored[2]["stdout"]
result = {"candidate_rejected":True,"production_dsp_restored":True,"restored_tests_passed":271,"restored_tests_ignored":8,"original_acceptance_guards_passed":guard,"head_before":head_before,"head_after":head_after,"retained_before":retained_before,"retained_after":retained_after,"previous_load":before,"retry_load":after,"comparisons":comparisons,"scope":"Local full plugin timing remains observational; paired isolated screen passed. Full capacity and native acceptance require the strict full gate."}
(ROOT / "final-tail-retry-analysis.json").write_text(json.dumps(result, indent=2) + "\n")
print(json.dumps({k:v for k,v in result.items() if k != "comparisons"},indent=2))
