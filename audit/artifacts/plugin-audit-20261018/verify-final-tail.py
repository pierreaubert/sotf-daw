"""Verify standalone final-tail sizing, shared transforms and fixed full-load evidence."""
import difflib
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

def totals(output):
    rows = [tuple(map(int, m)) for m in re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored", output)]
    return tuple(sum(row[i] for row in rows) for i in range(3))

validation = read("final-tail-validation.json")
assert len(validation) == 4 and all(row["exit_code"] == 0 for row in validation)
output = validation[0]["stdout"]
assert totals(output) == (273, 0, 7)
assert "Physical callback misses: 0." in validation[2]["stdout"]
initial = read("final-tail-initial-baseline.json")
assert initial["exit_code"] == 101 and "lost taps or excessive final padding" in initial["stderr"]
red = read("final-tail-baseline.json")
assert red["exit_code"] == 101
pattern = r"final_tail_layout cases=(\d+) failures=(\d+) last_blocks=(\d+) largest_blocks=(\d+)"
assert tuple(map(int, re.search(pattern, red["stdout"]).groups())) == (120, 110, 2951424, 2951424)
focused = read("final-tail-focused.json")
assert focused["exit_code"] == 0
assert tuple(map(int, re.search(pattern, focused["stdout"]).groups())) == (120, 0, 14336, 1476608)
assert re.search(pattern, output).groups() == re.search(pattern, focused["stdout"]).groups()
sharing = read("final-tail-sharing-baseline.json")
assert sharing["exit_code"] == 101 and "final_tail_sharing cases=40 failures=40" in sharing["stdout"]
assert "final_tail_sharing cases=40 failures=0" in focused["stdout"] and "final_tail_sharing cases=40 failures=0" in output
split = read("final-tail-split-validation.json")
assert len(split) == 4 and all(row["exit_code"] == 0 for row in split) and totals(split[0]["stdout"]) == (272, 0, 7)
selection = read("final-tail-selection.json")
assert selection["sharing_baseline_cases"] == selection["sharing_baseline_failures"] == 40
errors = {}
for name, cases in [("nupc_coalesced_unmerged",45), ("nupc_real_direct_reference",54), ("nupc_large_sparse_reference",4), ("nupc_phase_f64",36)]:
    m = re.search(name + r" cases=(\d+) maximum_error=([0-9.e+-]+)", output)
    assert m and int(m[1]) == cases and float(m[2]) < 2e-5
    errors[name] = float(m[2])

source = read("final-tail-source.json")
assert source["hashes_unchanged_after_load"]
for name, sha in source["source_sha256"].items():
    snapshot = source["candidate_snapshot_paths"].get(name, name)
    assert hashlib.sha256((REPO / snapshot).read_bytes()).hexdigest() == sha, name
for name, sha in source["baseline_artifact_sha256"].items():
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == sha, name

def previous(path):
    return subprocess.run(["rtk", "proxy", "git", "show", source["base_commit"] + ":" + path], capture_output=True, text=True, check=True).stdout

def body(text, signature):
    a = text.index(signature)
    b = text.index("{", a)
    depth, c = 1, b + 1
    while depth:
        if text[c] == "{": depth += 1
        elif text[c] == "}": depth -= 1
        c += 1
    return text[a:c]

plugin = "crates/sotf-plugins/crates/sotf-plugin-convolution/src/lib/convolution_plugin.rs"
assert previous(plugin) == (REPO / plugin).read_text(), "plugin/admission/host behavior changed"
assert source["candidate_rejected"]
for path, sha in source["restored_baseline_sha256"].items():
    # The paired retry adds only this cfg(test) diagnostic to restored DSP.
    restored = (REPO / path).read_text().removesuffix('\n#[cfg(test)]\n#[path = "final_tail_cost.rs"]\nmod final_tail_cost;\n')
    assert restored == previous(path), path
    assert hashlib.sha256(restored.encode()).hexdigest() == sha, path
assert not (REPO / "crates/sotf-plugins/crates/plugins-spatial/src/nupc/final_tail_tests.rs").exists()
patch = ""
for path, snapshot in source["candidate_snapshot_paths"].items():
    result = subprocess.run(["rtk", "proxy", "git", "show", source["base_commit"] + ":" + path], capture_output=True, text=True)
    before_text = result.stdout if result.returncode == 0 else ""
    patch += "".join(difflib.unified_diff(before_text.splitlines(True), (REPO / snapshot).read_text().splitlines(True), fromfile="a/" + path if result.returncode == 0 else "/dev/null", tofile="b/" + path, n=0))
assert (ROOT / "final-tail-candidate.patch").read_text() == patch
assert hashlib.sha256(patch.encode()).hexdigest() == source["candidate_patch_sha256"]
engine = "crates/sotf-plugins/crates/plugins-spatial/src/nupc/nupc_engine.rs"
for signature in ["pub fn new(ir: &[f32], min_block: usize) -> Self", "pub fn instantiate(&self)", "pub fn instantiate_with_phase(", "pub fn process_sample(", "pub fn reset(&mut self)", "pub fn largest_block_size("]:
    assert body(previous(engine), signature) == body((REPO / source["candidate_snapshot_paths"][engine]).read_text(), signature), signature
coalesced = "crates/sotf-plugins/crates/plugins-spatial/src/nupc/coalesced_tests.rs"
for signature in ["fn unmerged(", "fn merged_levels_match_separate_delayed_groups_through_reset_and_complete_tail("]:
    assert body(previous(coalesced), signature) == body((REPO / source["candidate_snapshot_paths"][coalesced]).read_text(), signature), signature
harness = "crates/sotf-plugins/crates/sotf-plugin-convolution/bin/qa_convolution_load.rs"
for signature in ["fn measure(", "fn preflight(", "fn fixture(", "fn main()"]:
    assert body(previous(harness), signature) == body((REPO / harness).read_text(), signature), signature

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

old = read("head-fft-selection-load.json")
new = read("final-tail-load.json")
assert [json.loads(line) for line in (ROOT / "final-tail-load.jsonl").read_text().splitlines() if line.strip()] == new["rows"]
a, old_summary = load(old, False)
b, new_summary = load(new, True)
comparisons = []
for k, row in b.items():
    before = a[k]
    assert row["geometry"] == before["geometry"], k
    for field in ["ir_frames", "estimate_bytes", "latency_frames", "next_frame_rejection", "measured_largest_period_frames"]:
        assert row[field] == before[field], (k, field)
    for left, right in zip([row["warm"], *row["steady"]], [before["warm"], *before["steady"]]):
        for field in ["callback_frames", "callbacks", "reported_quantum_frames", "physical_budget_us", "reported_budget_us"]:
            assert left[field] == right[field], (k, field)
    count = lambda r: sum(p["physical_misses"] for p in [r["warm"], *r["steady"]])
    comparisons.append(dict(geometry=row["geometry"], old_misses=count(before), new_misses=count(row), old_retained_bytes=before["construction_retained_requested_delta_bytes"], new_retained_bytes=row["construction_retained_requested_delta_bytes"], old_actual_period=before["actual_largest_period_frames"], new_actual_period=row["actual_largest_period_frames"]))
heads = [r for r in comparisons if r["geometry"]["head"]]
assert len(heads) == 21
head_miss_guard = sum(r["new_misses"] for r in heads) < sum(r["old_misses"] for r in heads)
memory_guard = sum(r["new_retained_bytes"] for r in heads) < sum(r["old_retained_bytes"] for r in heads)
failure = read("final-tail-acceptance-failure.json")
assert failure["exit_code"] == 1 and "AssertionError" in failure["stderr"] and "new_misses" in failure["stderr"]
assert not head_miss_guard and memory_guard
replayed = read("final-tail-replayed-acceptance.json")
assert replayed["exit_code"] == 1
replay_values = json.loads(replayed["stdout"])
assert replay_values["head_before"] == sum(r["old_misses"] for r in heads)
assert replay_values["head_after"] == sum(r["new_misses"] for r in heads)
assert replay_values["head_miss_guard_passed"] == head_miss_guard
assert replay_values["memory_guard_passed"] == memory_guard
restored = read("final-tail-restored-validation.json")
assert len(restored) == 4 and all(row["exit_code"] == 0 for row in restored)
assert totals(restored[0]["stdout"]) == (271, 0, 7)
ratios = [r["new_retained_bytes"] / r["old_retained_bytes"] for r in heads]
unchanged_controls = [r for r in heads if r["new_actual_period"] == r["old_actual_period"]]
print(json.dumps(dict(candidate_functional_verified=True, candidate_rejected=True, historical_head_miss_guard_passed=head_miss_guard, memory_guard_passed=memory_guard, production_dsp_restored_exactly=True, restored_tests_passed=271, restored_tests_ignored=7, candidate_tests_passed=273, candidate_tests_ignored=7, standalone_cases=120, sharing_cases=40, maximum_errors=errors, plugin_and_admission_unchanged=True, callbacks_budgets_and_period_floor_unchanged=True, actual_periods_verified=True, previous_load=old_summary, load=new_summary, unchanged_head_controls=len(unchanged_controls), unchanged_head_old_misses=sum(r["old_misses"] for r in unchanged_controls), unchanged_head_new_misses=sum(r["new_misses"] for r in unchanged_controls), head_old_misses=sum(r["old_misses"] for r in heads), head_new_misses=sum(r["new_misses"] for r in heads), head_geometries_fewer_misses=sum(r["new_misses"] < r["old_misses"] for r in heads), head_geometries_equal_misses=sum(r["new_misses"] == r["old_misses"] for r in heads), head_geometries_more_misses=sum(r["new_misses"] > r["old_misses"] for r in heads), head_retained_ratio_min=min(ratios), head_retained_ratio_max=max(ratios), comparisons=comparisons, scope="Same 63 admitted geometries, 562686 callbacks and strict budgets. Actual periods may shrink but the preceding untrimmed period remains a measurement floor. Local historical timing is unmatched; no causal/native-device or new linear-EQ capacity acceptance."), indent=2))
