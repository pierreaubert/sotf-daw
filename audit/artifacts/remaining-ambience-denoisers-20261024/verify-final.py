"""Verify current DSP fixes, preserved failures, complete quality and timing scope."""
from pathlib import Path
import hashlib
import json
import math
import re
import subprocess

ART = Path(__file__).resolve().parent
REPO = ART.parents[2]
def read(name): return json.loads((ART / name).read_text())
def totals(text):
    rows = [tuple(map(int, x)) for x in re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored", text)]
    return tuple(sum(x[i] for x in rows) for i in range(3))
def previous(path):
    return subprocess.run(["rtk","proxy","git","show","b481bd8:"+path],cwd=REPO,capture_output=True,text=True,check=True).stdout
def json_rows(text):
    rows = []
    for line in text.splitlines():
        try: value = json.loads(line)
        except ValueError: continue
        if isinstance(value,dict): rows.append(value)
    return rows

meta = read("checkpoint.json")
for path, digest in meta["source_sha256"].items():
    assert hashlib.sha256((REPO / path).read_bytes()).hexdigest() == digest, path
assert read("strict-clippy.json")["exit_code"] == 0
assert read("denoiser-optional-qa.json")["exit_code"] == 0
assert totals(read("aae-density-functional.json")["stdout"]) == (131,0,0)
assert totals(read("denoiser-final-validation.json")["stdout"]) == (129,0,4)
assert totals(read("speech-temporal-reference-validation.json")["stdout"]) == (219,0,3)
assert totals(read("speech-temporal-recorded-quality.json")["stdout"]) == (1,0,0)

# The neutral model changes only its independently derived grid, not its
# excitation, dense matrix, complete-tail loop or numerical/fit assertions.
neutral_path = "crates/sotf-plugins/crates/sotf-plugin-aae/tests/audit_neutral_decay.rs"
old_neutral = (ART / "aae-neutral-reference-before.rs.txt").read_text()
assert old_neutral == previous(neutral_path)
current = (REPO / neutral_path).read_text()
old_test = old_neutral[old_neutral.index("#[test]"):].replace("Lossless::new(rate, room);", "Lossless::new(rate, room, rt60);")
assert current[current.index("#[test]"):] == old_test
for suffix in ["src/early_reflections.rs","src/tone_filter.rs","src/hadamard.rs","bin/qa_aae_decay.rs"]:
    path = "crates/sotf-plugins/crates/sotf-plugin-aae/"+suffix
    assert (REPO / path).read_text() == previous(path), path
quality = read("aae-density-quality.json")
assert len(quality) == 2 and all(x["exit_code"] == 0 for x in quality)
new_rows = json_rows(quality[1]["stdout"])
old_rows = [json.loads(x) for x in (REPO / "audit/artifacts/plugin-audit-20261018/aae-radial-damping-final-corners.jsonl").read_text().splitlines() if x.strip()]
key = lambda row: tuple(row.get(k) for k in ["rate","room","rt60","bass","treble","modulation","excitation","drive_frames","neutral_control"])
a = {key(x):x for x in old_rows if x.get("kind")=="decay"}
b = {key(x):x for x in new_rows if x.get("kind")=="decay"}
assert len(a) == len(b) == 208 and a.keys() == b.keys()
assert sum(not x["passed"] for x in a.values()) == 12
for k, row in b.items():
    assert row["frames"] == a[k]["frames"]
    assert row["passed"] and row["finite"] and row["target_ok"] and row["fit_ok"]
    assert row["fits"][0]["r2"] > .98
assert new_rows[-1]["rows"] == 208 and new_rows[-1]["failed"] == 0

# Reject the spectral bootstrap experiment under all original/predeclared gates.
rejected = read("denoiser-spectral-prior-analysis.json")
assert rejected["candidate_rejected"] and not rejected["functional_gate_passed"]
assert not rejected["vowel_guard_passed"] and rejected["recorded_full_sdr_guard_passed"] and rejected["noise_guard_passed"]
assert read("denoiser-spectral-prior-functional.json")["exit_code"] == 101
assert "test_noise_reduction_reduces_energy" in read("denoiser-spectral-prior-functional.json")["stderr"]
mcra = "crates/sotf-plugins/crates/sotf-plugin-denoiser/src/mcra.rs"
assert (REPO / mcra).read_text() == previous(mcra)
assert "bootstrap_spectral_prior" not in (REPO / "crates/sotf-plugins/crates/sotf-plugin-denoiser/src/multi_resolution.rs").read_text()
old_rows = json_rows(read("denoiser-small-clock-grid-validation.json")["stdout"])
new_rows = json_rows(read("denoiser-final-validation.json")["stdout"])
# The main-estimator recorded input/output measurements return exactly.
for measurement in ["recorded_denoiser_onset","recorded_estimator","independent_noise_channel"]:
    old = [x for x in old_rows if x.get("measurement")==measurement]
    new = [x for x in new_rows if x.get("measurement")==measurement]
    sort = lambda rows: sorted(rows,key=lambda x:json.dumps(x,sort_keys=True))
    assert sort(old) == sort(new), measurement

speech = read("speech-temporal-analysis.json")
assert speech["cases"] == 36 and speech["exact_controls"] == 18 and speech["changed_cases"] == 18
assert speech["predeclared_quality_guard_passed"] and speech["mono42_quality_fields_exact"]
assert len(speech["rows"]) == 36
for row in speech["rows"]:
    assert row["ordinary_sdr_delta_db"] >= -.25 and row["si_sdr_delta_db"] >= -.25
    if row["regime"].endswith("control"):
        assert row["ordinary_sdr_delta_db"] == row["si_sdr_delta_db"] == 0
assert read("speech-energy-crossing-baseline.json")["exit_code"] == 101
assert "relative_detector_delta=2.001" in read("speech-energy-crossing-baseline.json")["stdout"]

trials = read("aae-paired-timing.json")
assert len(trials) == 4 and [x["variant"] for x in trials] == ["baseline","candidate","candidate","baseline"]
assert all(x["exit_code"] == 0 for x in trials)
builds = read("aae-timing-builds.json")
assert len(builds) == 2 and all(x["exit_code"] == 0 for x in builds)
source = read("aae-paired-timing-source.json")
fdn = REPO / "crates/sotf-plugins/crates/sotf-plugin-aae/src/fdn.rs"
assert hashlib.sha256(fdn.read_bytes()).hexdigest() == source["candidate_source_sha256"]
assert hashlib.sha256((ART / "aae-fdn-before.rs.txt").read_bytes()).hexdigest() == source["baseline_source_sha256"]
raw = read("aae-density-full-timing.json")
rows = json_rows(raw["stdout"])
timing = [x for x in rows if x.get("kind")=="timing"]
summary = rows[-1]
assert len(timing) == summary["cases"] == 64
wall = cpu = missing = callbacks = 0
cohorts = {}
for row in timing:
    assert row["observation_seconds"] == 5 and row["spatial_prelude"] and row["buffer_mode"] == "full_stream"
    assert len(row["samples"]) == row["blocks"]
    assert math.isclose(row["deadline_ns"],row["block_frames"]*1e9/row["rate"],rel_tol=1e-12)
    wall_count = sum(pair[0]>row["deadline_ns"] for pair in row["samples"])
    cpu_count = sum(pair[1] is not None and pair[1]>row["deadline_ns"] for pair in row["samples"])
    missing_count = sum(pair[1] is None for pair in row["samples"])
    assert wall_count == row["wall_deadline_misses"]
    assert cpu_count == row["cpu_deadline_misses"]
    assert missing_count == row["missing_cpu_samples"]
    wall += wall_count; cpu += cpu_count; missing += missing_count; callbacks += len(row["samples"])
    cohorts.setdefault(row["condition_index"],set()).add(row["audio_sha256"])
assert len(cohorts) == 16 and all(len(values)==1 for values in cohorts.values())
assert wall == summary["wall_deadline_misses"] == 5
assert cpu == summary["cpu_deadline_misses"] == 0
assert missing == summary["missing_cpu_samples"] == summary["missing_usage_cases"] == 0
assert raw["exit_code"] == 1
assert meta["plugin_statuses"] == {"1":"partial","19":"partial","40":"partial"}
print(json.dumps({"verified":True,"aae_functional":131,"aae_decay_cases":208,"aae_prior_failures_fixed":12,"denoiser_functional":129,"speech_functional_plus_recorded_test":220,"recorded_crossing_cases":36,"legacy_abba_passed":True,"full_timing_callbacks":callbacks,"full_wall_misses":wall,"full_cpu_misses":cpu,"spectral_bootstrap_rejected_and_restored":True,"all_three_plugins_remain_partial":True}))
