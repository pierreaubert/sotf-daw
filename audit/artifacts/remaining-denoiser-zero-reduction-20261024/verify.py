"""Verify zero-depth semantics; --frozen explicitly checks the archived checkpoint."""
from pathlib import Path
import hashlib
import json
import re
import subprocess
import sys

ART = Path(__file__).resolve().parent
REPO = ART.parents[2]
FROZEN = "--frozen" in sys.argv[1:]
def read(name): return json.loads((ART / name).read_text())
def totals(text):
    rows = [tuple(map(int,x)) for x in re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored",text)]
    return tuple(sum(row[i] for row in rows) for i in range(3))
def objects(text):
    rows = []
    for line in text.splitlines():
        try: value = json.loads(line)
        except ValueError: continue
        if isinstance(value,dict): rows.append(value)
    return rows
meta = read("checkpoint.json")
for path,digest in meta["source_sha256"].items():
    snapshot = REPO / meta["frozen_source_paths"][path]
    assert hashlib.sha256(snapshot.read_bytes()).hexdigest() == digest,path
    if not FROZEN: assert hashlib.sha256((REPO / path).read_bytes()).hexdigest() == digest,path
for suffix in ["mcra.rs","wiener/consts.rs","polyphonic.rs","multi_resolution.rs","fft.rs","noise_profile.rs","params/consts.rs"]:
    path = "crates/sotf-plugins/crates/sotf-plugin-denoiser/src/"+suffix
    before = subprocess.run(["rtk","proxy","git","show","78eb434:"+path],cwd=REPO,capture_output=True,text=True,check=True).stdout
    actual = (REPO / meta["frozen_source_paths"][path]).read_text()
    assert actual == before,path
baseline = read("baseline.json")
assert baseline["exit_code"] == 101
assert "zero_reduction_summary cases=48 failures=48" in baseline["stdout"]
assert "a_published_captured_profile_cannot_override_zero_attenuation" in baseline["stderr"]
validation = read("validation.json")
assert validation["exit_code"] == 0 and totals(validation["stdout"]) == (134,0,4)
match = re.search(r"zero_reduction_summary cases=(\d+) failures=(\d+) maximum_error=([0-9.e+-]+)",validation["stdout"])
assert match and int(match[1])==48 and int(match[2])==0 and float(match[3])<2e-6
assert len(re.findall(r"zero_reduction_profile low=(?:true|false) maximum_error=",validation["stdout"])) == 2
assert "zero_depth_cold_callbacks_live_transitions_and_reset_have_no_heap_work ... ok" in validation["stdout"]
match = re.search(r"zero_reduction_clock cases=(\d+) maximum_error=([0-9.e+-]+) exact_endpoints=true",validation["stdout"])
assert match and int(match[1])==16 and float(match[2])<2e-6
components = [json.loads(line.split("COMPONENT ",1)[1]) for line in validation["stdout"].splitlines() if "COMPONENT " in line]
assert len(components)==720
assert {row["variant"] for row in components} == {"default","instantaneous_snr","no_gaussian_smoothing","instantaneous_no_gaussian","formant"}
for row in components:
    if row["gain_frame_active"]:
        assert abs(row["observed_gain_power"]-row["mixed_periodogram"])/(1+row["mixed_periodogram"])<2e-6
assert "estimator_invariant=true" in validation["stdout"]
old = read("../remaining-ambience-denoisers-20261024/denoiser-final-validation.json")
key = lambda value: json.dumps(value,sort_keys=True)
for measurement,count in [("recorded_estimator",36),("recorded_denoiser_onset",2),("independent_noise_channel",10)]:
    before = [row for row in objects(old["stdout"]) if row.get("measurement")==measurement]
    after = [row for row in objects(validation["stdout"]) if row.get("measurement")==measurement]
    assert len(before)==len(after)==count
    assert sorted(before,key=key)==sorted(after,key=key),measurement
assert read("clippy.json")["exit_code"]==0
assert read("optional-qa.json")["exit_code"]==0
assert len(meta["optional_profile_rows"])==48
assert sum(row["definite_max_over_budget"] for row in meta["optional_profile_rows"])==meta["nonexclusive_optional_profile_over_budget_rows"]==5
assert not meta["deadline_acceptance_claimed"]
print(json.dumps({"verified":True,"source_scope":"frozen checkpoint" if FROZEN else "current worktree","tests_passed":134,"existing_ignored":4,"unity_cases":48,"captured_profile_cases":2,"physical_clock_cases":16,"positive_default_recorded_rows_exact":36,"component_records":720,"nonexclusive_optional_profile_over_budget_rows":5,"deadline_acceptance":False,"denoiser_remains_partial":True}))
