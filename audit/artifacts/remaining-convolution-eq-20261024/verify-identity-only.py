"""Verify the isolated identity-delay increment and rejection/restoration evidence."""
from pathlib import Path
import hashlib
import json
import re
import subprocess
import sys

ART = Path(__file__).resolve().parent
ROOT = ART.parents[2]
read = lambda p: json.loads(p.read_text())
source = read(ART / "identity-only-source.json")
for path, snapshot in source["snapshot_paths"].items():
    assert hashlib.sha256((ROOT / snapshot).read_bytes()).hexdigest() == source["source_sha256"][path], path
    if "--current" in sys.argv:
        assert hashlib.sha256((ROOT / path).read_bytes()).hexdigest() == source["source_sha256"][path], path
for change in source.get("documentation_only_after_validation", []):
    assert all(line.startswith("///") for line in change["old"].splitlines())
    assert all(line.startswith("///") for line in change["new"].splitlines())
    text = (ROOT / source["snapshot_paths"][change["path"]]).read_text()
    assert text.count(change["new"]) == 1
    assert hashlib.sha256(text.replace(change["new"], change["old"], 1).encode()).hexdigest() == change["before_sha256"]
checks = read(ART / "identity-only-validation.json")
assert len(checks) == 3 and all(x["exit_code"] == 0 for x in checks)
rows = [tuple(map(int,x)) for x in re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored",checks[0]["stdout"])]
assert tuple(sum(x[i] for x in rows) for i in range(3)) == (273,0,10)
assert "prepared_unit_delay_preserves_samples_and_reset_through_arbitrary_callbacks ... ok" in checks[0]["stdout"]
snapshot = lambda path: (ROOT / source["snapshot_paths"][path]).read_text()
partition = "crates/sotf-plugins/crates/plugins-spatial/src/nupc/partition_level.rs"
plugin = "crates/sotf-plugins/crates/sotf-plugin-convolution/src/lib/convolution_plugin.rs"
coalesced = "crates/sotf-plugins/crates/plugins-spatial/src/nupc/coalesced_tests.rs"
assert snapshot(partition) == (ART / "partition-level-baseline.rs.txt").read_text()
assert snapshot(plugin).removesuffix('\n#[cfg(test)]\n#[path = "overlap_cost.rs"]\nmod overlap_cost;\n') == (ART / "convolution-plugin-baseline.rs.txt").read_text()
assert snapshot(coalesced).replace("        identity_delay: None,\n", "",1) == (ART / "coalesced-tests-baseline.rs.txt").read_text()
real = "crates/sotf-plugins/crates/plugins-spatial/src/nupc/real_fft_tests.rs"
assert snapshot(real).startswith((ART / "real-fft-tests-baseline.rs.txt").read_text())
# Host latency/support/priming arithmetic was not modified for this increment.
core = "crates/sotf-plugins/crates/sotf-plugin-linear-phase-eq/src/lib/linear_phase_eq_plugin.rs"
prior = subprocess.run(["rtk","proxy","git","show","b481bd8:"+core],cwd=ROOT,capture_output=True,text=True,check=True).stdout
assert (ROOT / core).read_text() == prior
pair = read(ART / "paired-analysis.json")
assert pair["comparators_verified"] and not any(x["screen_passed"] for x in pair["backends"].values())
assert read(ART / "paired-acceptance-failure.json")["exit_code"] == 1
result = {"identity_semantics_verified":True,"tests_passed":273,"tests_ignored":10,"clippy_format_passed":True,"rejected_performance_hunks_restored":True,"old_oracles_preserved":True,"host_latency_support_priming_unchanged":True,"full_capacity_accepted":False,"scope":source["identity_only_semantic_storage_scope"]}
(ART / "identity-only-analysis.json").write_text(json.dumps(result,indent=2)+"\n")
print(json.dumps(result,indent=2))
