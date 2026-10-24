"""Replay deterministic AAE and optional-denoiser correction evidence."""
from pathlib import Path
import hashlib
import json
import re
import subprocess

ART = Path(__file__).resolve().parent
REPO = ART.parents[2]
read = lambda name: json.loads((ART / name).read_text())
meta = read("checkpoint-initial.json")
# Initial deterministic checkpoint predates the separate density/temporal
# candidates. Preserve its exact measured sources instead of pretending that
# its original tests prove later architecture changes.
snapshots = {
    "crates/sotf-plugins/crates/sotf-plugin-aae/src/fdn.rs": ART / "aae-fdn-lfo-only.rs.txt",
    "crates/sotf-plugins/crates/sotf-plugin-denoiser/src/multi_resolution.rs": ART / "denoiser-pre-spectral-prior-multi_resolution.rs.txt",
    "crates/sotf-plugins/crates/sotf-plugin-denoiser/src/lib/denoiser_plugin.rs": ART / "denoiser-pre-spectral-prior-denoiser_plugin.rs.txt",
}
for path, digest in meta["source_sha256"].items():
    assert hashlib.sha256(snapshots.get(path, REPO / path).read_bytes()).hexdigest() == digest, path

def totals(text):
    rows = [tuple(map(int, row)) for row in re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored", text)]
    return tuple(sum(row[i] for row in rows) for i in range(3))

aae = read("aae-lfo-validation.json")
assert aae["exit_code"] == 0 and totals(aae["stdout"]) == (129, 0, 0)
match = re.search(r"fdn_lfo_reference cases=(\d+) samples=(\d+) maximum_error=([0-9.e+-]+) exact_reset=true", aae["stdout"])
assert match and int(match[1]) == 32 and int(match[2]) == 32000000 and float(match[3]) < 6e-8
legacy = read("aae-lfo-baseline-clock.json")
assert legacy["exit_code"] == 1
legacy_values = json.loads(legacy["stdout"])
assert legacy_values["cases"] == 32 and legacy_values["samples"] == 32000000
assert legacy_values["maximum_error"] > 0.08 and not legacy_values["passed"]
path = "crates/sotf-plugins/crates/sotf-plugin-aae/src/fdn.rs"
old = subprocess.run(["rtk", "proxy", "git", "show", meta["base_commit"] + ":" + path], cwd=REPO, capture_output=True, text=True, check=True).stdout
assert old == (ART / "aae-fdn-before.rs.txt").read_text()
for suffix in ["src/tone_filter.rs", "bin/qa_aae_decay.rs", "tests/audit_neutral_decay.rs"]:
    path = "crates/sotf-plugins/crates/sotf-plugin-aae/" + suffix
    prior = subprocess.run(["rtk", "proxy", "git", "show", meta["base_commit"] + ":" + path], cwd=REPO, capture_output=True, text=True, check=True).stdout
    actual = ART / "aae-neutral-reference-before.rs.txt" if suffix == "tests/audit_neutral_decay.rs" else REPO / path
    assert prior == actual.read_text(), path

baseline = read("denoiser-small-clock-grid-baseline.json")
assert baseline["exit_code"] == 101
assert "small_grid_mapping failures=255" in baseline["stdout"]
assert baseline["stdout"].count("observed=6 expected=") == 10
new = read("denoiser-small-clock-grid-validation.json")
assert new["exit_code"] == 0 and totals(new["stdout"]) == (129, 0, 4)
assert "small_grid_mapping failures=0" in new["stdout"]
clocks = re.findall(r"small_bootstrap rate=([0-9.]+) low_latency=(false|true) observed=(\d+) expected=(\d+)", new["stdout"])
assert len(clocks) == 10
for rate, mode, observed, expected in clocks:
    import math
    reference = math.ceil(float(rate) * (5120.0 / 48000.0) / 256.0)
    assert int(observed) == int(expected) == reference
assert meta["plugin_statuses"] == {"1":"partial", "19":"partial", "40":"partial"}
print(json.dumps({"deterministic_evidence_verified":True,"aae_tests":129,"denoiser_tests":129,"denoiser_existing_ignores":4,"acceptance_timing_pending":True,"quality_and_native_scope_remain_open":True}))
