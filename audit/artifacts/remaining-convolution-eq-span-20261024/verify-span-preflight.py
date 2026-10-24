"""Verify the span candidate against scalar code and the complete declared plan."""
from pathlib import Path
import hashlib
import json
import re
import sys

ART = Path(__file__).resolve().parent
ROOT = ART.parents[2]
read = lambda p: json.loads(p.read_text())
source = read(ART / "source.json")
measured = read(ART / "measured-source.json")
current_requested = "--current" in sys.argv
assert source["hashes_unchanged_after_validation"]
for path,snapshot in source["snapshot_paths"].items():
    assert hashlib.sha256((ROOT / snapshot).read_bytes()).hexdigest() == source["source_sha256"][path],path
for path, snapshot in source["snapshot_paths"].items():
    current = (ROOT / (path if current_requested else measured["snapshot_paths"][path])).read_text()
    if path.endswith("convolution_plugin.rs"):
        current = current.removesuffix('\n#[cfg(test)]\n#[path = "span_cost.rs"]\nmod span_cost;\n')
    assert current == (ROOT / snapshot).read_text(), path
checks = read(ART / "combined-validation.json")
assert len(checks) == 2 and all(x["exit_code"] == 0 for x in checks)
rows = [tuple(map(int,x)) for x in re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored",checks[0]["stdout"])]
assert tuple(sum(x[i] for x in rows) for i in range(3)) == (274,0,10)
assert "nupc_span_sparse cases=12 maximum_error=" in checks[0]["stdout"] and "exact_bits=true" in checks[0]["stdout"]

def body(text,name):
    start = text.index("fn " + name + "(")
    a = text.index("{",start)
    end,depth = a+1,1
    while depth:
        depth += (text[end] == "{") - (text[end] == "}")
        end += 1
    return text[start:end]
engine = "crates/sotf-plugins/crates/plugins-spatial/src/nupc/nupc_engine.rs"
partition = "crates/sotf-plugins/crates/plugins-spatial/src/nupc/partition_level.rs"
plugin = "crates/sotf-plugins/crates/sotf-plugin-convolution/src/lib/convolution_plugin.rs"
for path,names in [(engine,["process_sample","new_with_head","instantiate_with_phase","reset"]),(partition,["push_sample","process_block","reset"])]:
    old = (ROOT / source["baseline_snapshots"][path]).read_text()
    current = (ROOT / source["snapshot_paths"][path]).read_text()
    for name in names: assert body(current,name) == body(old,name),(path,name)
old = (ROOT / source["baseline_snapshots"][plugin]).read_text()
extracted = body((ART / "scalar-stream-baseline.rs.txt").read_text(),"process_stream_scalar_span_baseline")
assert extracted.replace("process_stream_scalar_span_baseline","process_stream",1) == body(old,"process_stream")
current = (ROOT / source["snapshot_paths"][plugin]).read_text()
assert body(current,"finish_output_block") == body(old,"finish_output_block")
assert body(current,"process_stream").split("// UPC path: uniform partitioned convolution.",1)[1] == body(old,"process_stream").split("// UPC path: uniform partitioned convolution.",1)[1]
plan = read(ART / "paired-plan.json")
for path,sha in plan["source_sha256"].items():
    candidate = ROOT / (path if current_requested else measured["snapshot_paths"][path])
    assert hashlib.sha256(candidate.read_bytes()).hexdigest() == sha,path
history = read(ROOT / "audit/artifacts/plugin-audit-20261018/head-fft-selection-load.json")
original = [x for x in history["rows"] if x["event"] == "case_result" and x["geometry"]["nupc"]]
key = lambda c: (c["rate"],c["channels"],c["ir_channels"],c["matrix"],c["head"])
baseline = {(x["geometry"]["rate_hz"],x["geometry"]["output_channels"],x["geometry"]["ir_channels"],x["geometry"]["true_stereo"],x["geometry"]["head"]):x for x in original}
changed = [c for c in plan["cases"] if not c["control"]]
controls = [c for c in plan["cases"] if c["control"]]
assert len(changed) == 42 and {key(c) for c in changed} == set(baseline)
for c in changed:
    before = baseline[key(c)]
    assert c["frames"] == before["ir_frames"] and c["warm"] == before["warm"]["callbacks"]
    assert c["calls"] == before["steady"][0]["callbacks"] and c["nupc"]
assert len(controls) == 6
for c in controls:
    assert not c["nupc"] and c["calls"] == (8192 if c["frames"] else 32768)
print(json.dumps({"scalar_methods_and_transform_arithmetic_unchanged":True,"tests_passed":274,"tests_ignored_before_manual_comparator":10,"large_sparse_cases":12,"changed_cases":42,"common_controls":6,"trials":192,"performance_accepted":False},indent=2))
