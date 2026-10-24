"""Run the unchanged full convolution matrix and retain exact retry inputs."""
from pathlib import Path
import hashlib
import json
import os
import platform
import subprocess
import time

ROOT = Path(__file__).resolve().parents[3]
ART = Path(__file__).resolve().parent
previous = json.loads((ART / "final-tail-source.json").read_text())
# Refuse to replace candidate evidence with a run of restored production DSP.
for target, snapshot in previous["candidate_snapshot_paths"].items():
    expected = (ROOT / snapshot).read_text()
    if target.endswith("nupc_engine.rs"):
        expected += '\n#[cfg(test)]\n#[path = "final_tail_cost.rs"]\nmod final_tail_cost;\n'
    if not (ROOT / target).exists() or (ROOT / target).read_text() != expected:
        raise SystemExit("Reapply the archived candidate before running this measurement; production is restored.")
paths = list(previous["source_sha256"])
paths.append("crates/sotf-plugins/crates/plugins-spatial/src/nupc/final_tail_cost.rs")
def hashes():
    return {p:hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in paths}
source = {"source_sha256":hashes(),"baseline_commit":"eab4937","paired_screen":"final-tail-paired-analysis.json","workload_policy":"Identical full 63-case matrix, admission, budgets and fixed 562686 callbacks; max legacy, preceding prepared, actual periods."}
(ART / "final-tail-retry-source.json").write_text(json.dumps(source, indent=2) + "\n")
command = json.loads((ART / "final-tail-load.json").read_text())["command"]
start = time.monotonic()
with (ART / "final-tail-retry-load.jsonl").open("w") as out, (ART / "final-tail-retry-load.stderr.txt").open("w") as err:
    result = subprocess.run(command, cwd=ROOT, stdout=out, stderr=err, env={**os.environ,"CARGO_BUILD_JOBS":"2","TMPDIR":"/Volumes/home_tmp/tmp/sotf-daw-audit-20261018-nupc-temp"})
rows = [json.loads(line) for line in (ART / "final-tail-retry-load.jsonl").read_text().splitlines() if line.strip()]
summary = {"command":command,"exit_code":result.returncode,"elapsed_seconds":time.monotonic()-start,"platform":platform.platform(),"rows":rows}
(ART / "final-tail-retry-load.json").write_text(json.dumps(summary, indent=2) + "\n")
source["hashes_unchanged_after_load"] = hashes() == source["source_sha256"]
(ART / "final-tail-retry-source.json").write_text(json.dumps(source, indent=2) + "\n")
assert source["hashes_unchanged_after_load"]
print(json.dumps({k:v for k,v in summary.items() if k != "rows"}))
print(json.dumps(rows[-1] if rows else {}))
