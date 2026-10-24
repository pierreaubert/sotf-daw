"""Verify frozen production comparators and replay the predeclared ABBA screen."""
from pathlib import Path
import hashlib
import json
import math
import statistics

ART = Path(__file__).resolve().parent
ROOT = ART.parents[2]
read = lambda p: json.loads(p.read_text())
plan = read(ART / "paired-plan.json")
first = read(ART / "first-batch-source.json")
for path, sha in plan["source_sha256"].items():
    assert hashlib.sha256((ROOT / path).read_bytes()).hexdigest() == sha, path
for path, snapshot in first["snapshot_paths"].items():
    assert hashlib.sha256((ROOT / snapshot).read_bytes()).hexdigest() == first["snapshot_sha256"][path], path

def body(text, name):
    start = text.index("fn " + name + "(")
    a = text.index("{", start)
    end, depth = a + 1, 1
    while depth:
        depth += (text[end] == "{") - (text[end] == "}")
        end += 1
    return text[start:end]
module = '\n#[cfg(test)]\n#[path = "final_tail_cost.rs"]\nmod final_tail_cost;\n'
engine = "crates/sotf-plugins/crates/plugins-spatial/src/nupc/nupc_engine.rs"
plugin = "crates/sotf-plugins/crates/sotf-plugin-convolution/src/lib/convolution_plugin.rs"
for kind, engine_path, plugin_path in [
    ("legacy", ART / "nupc-engine-baseline.rs.txt", ART / "convolution-plugin-baseline.rs.txt"),
    ("candidate", ROOT / first["snapshot_paths"][engine], ROOT / first["snapshot_paths"][plugin]),
]:
    assert (ART / (kind + "-nupc-engine-cost.rs.txt")).read_text() == engine_path.read_text().removesuffix(module)
    extracted = body((ART / (kind + "-upc-stream-cost.rs.txt")).read_text(),"process_stream_"+kind+"_for_cost")
    assert extracted.replace("process_stream_"+kind+"_for_cost", "process_stream",1) == body(plugin_path.read_text(),"process_stream")
legacy = body((ART / "legacy-upc-stream-cost.rs.txt").read_text(),"process_stream_legacy_for_cost").replace("process_stream_legacy_for_cost","process_stream",1)
candidate = body((ART / "candidate-upc-stream-cost.rs.txt").read_text(),"process_stream_candidate_for_cost").replace("process_stream_candidate_for_cost","process_stream",1)
assert legacy.split("// UPC path: uniform partitioned convolution.",1)[0] == candidate.split("// UPC path: uniform partitioned convolution.",1)[0]
source = read(ART / "paired-measurement-source.json")
assert source["hashes_unchanged_after_measurement"] and source["source_sha256"] == plan["source_sha256"]
commands = read(ART / "paired-commands.json")
assert len(commands) == 2 and all(x["exit_code"] == 0 for x in commands)
rows = []
for backend in ["nupc","upc"]:
    rows.extend(json.loads(x.split("PAIRED ",1)[1]) for x in (ART / ("paired-"+backend+".stdout")).read_text().splitlines() if "PAIRED " in x)
assert len(rows) == 132
cases = {(c["backend"],c["case"],c["rate"]):c for c in plan["cases"]}
assert len(cases) == 33
observed = {}
for row in rows:
    observed.setdefault((row["backend"],row["case"],row["rate"]),[]).append(row)
assert set(observed) == set(cases)
comparisons = []
for key, group in observed.items():
    case = cases[key]
    control = case["case"].startswith("control")
    assert [r["trial"] for r in group] == [0,1,2,3]
    assert [r["candidate"] for r in group] == [False,True,True,False]
    if case["backend"] == "upc":
        warm = max(256,(case["ir_frames"]+1024+37+127)//128)
        calls = 1024
    else:
        tail = case["ir_frames"] - case["head"]
        base = case["min_block"] if not case["head"] else 1 << (min(case["min_block"],case["head"]).bit_length()-1)
        period = base if tail else 0
        coverage = 4 * base
        while coverage < tail:
            period *= 2; coverage += 4 * period
        warm = (case["ir_frames"]+(case["min_block"] if not case["head"] else 0)+37+127)//128
        calls = max(256,(4*period+127)//128)
    for row in group:
        assert row["warm"] == warm and row["calls"] == calls
        assert len(row["durations_ns"]) == calls
        assert sum(row["durations_ns"]) == row["total_ns"]
        assert all(x >= 0 for x in row["durations_ns"])
        assert math.isfinite(row["checksum"]) and 0 <= row["max_error"] <= plan["screen"]["waveform_max_error"]
        if control or not row["candidate"]: assert row["max_error"] == 0
    before = statistics.median(r["total_ns"] for r in group if not r["candidate"])
    after = statistics.median(r["total_ns"] for r in group if r["candidate"])
    comparisons.append({"backend":key[0],"case":key[1],"rate":key[2],"control":control,"legacy_median_total_ns":before,"candidate_median_total_ns":after,"ratio":after/before,"maximum_error":max(r["max_error"] for r in group)})
results = {}
for backend in ["nupc","upc"]:
    subset = [r for r in comparisons if r["backend"] == backend]
    changed = [r for r in subset if not r["control"]]
    controls = [r for r in subset if r["control"]]
    assert len(subset) == plan["case_counts"][backend] and len(controls) == 6
    ratio = sum(r["candidate_median_total_ns"] for r in changed) / sum(r["legacy_median_total_ns"] for r in changed)
    screen = plan["screen"]
    passed = ratio <= screen["changed_aggregate_ratio_max"] and all(r["ratio"] <= screen["changed_case_ratio_max"] for r in changed) and all(screen["control_ratio_min"] <= r["ratio"] <= screen["control_ratio_max"] for r in controls)
    results[backend] = {"screen_passed":passed,"changed_aggregate_ratio":ratio,"largest_changed_ratio":max(r["ratio"] for r in changed),"control_ratio_min":min(r["ratio"] for r in controls),"control_ratio_max":max(r["ratio"] for r in controls)}
result = {"comparators_verified":True,"callbacks":sum(r["calls"] for r in rows),"backends":results,"comparisons":comparisons,"scope":plan["scope"]}
(ART / "paired-analysis.json").write_text(json.dumps(result,indent=2)+"\n")
print(json.dumps({k:v for k,v in result.items() if k != "comparisons"},indent=2))
