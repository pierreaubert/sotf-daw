"""Verify the current spatial increment without turning remaining scope green."""
from pathlib import Path
import collections
import hashlib
import json
import re
import subprocess

ART = Path(__file__).resolve().parent
REPO = ART.parents[2]
def read(name):
    return json.loads((ART / name).read_text())
def total(text):
    rows = [tuple(map(int, x)) for x in re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored", text)]
    return tuple(sum(row[i] for row in rows) for i in range(3))
source = read("spatial-source.json")
for path, sha in source["source_sha256"].items():
    assert hashlib.sha256((REPO/path).read_bytes()).hexdigest() == sha, path
fixture = REPO/"crates/sotf-plugins/crates/sotf-plugin-binaural/tests/fixtures/kemar/mit_kemar_normal_pinna.sofa"
assert hashlib.sha256(fixture.read_bytes()).hexdigest() == "e7035994f5fd754058424c061380ee92b1d5ed58fccef2887a4266916616acdf"
assert read("binaural-directional-baseline.json")["exit_code"] == 101
assert "0 passed; 2 failed" in read("binaural-directional-baseline.json")["stdout"]
assert read("binaural-directional-fixed.json")["exit_code"] == 0
assert "cases=60 maximum_error=1.6391277e-7" in read("binaural-directional-fixed.json")["stderr"]
assert read("upmixer-coherence-baseline.json")["exit_code"] == 101
assert "cold coherence round=0" in read("upmixer-coherence-baseline.json")["stderr"]
upmixer = read("upmixer-validation.json")
assert len(upmixer) == 2 and all(x["exit_code"] == 0 for x in upmixer)
assert total(upmixer[0]["stdout"]) == (229,0,3)
measured = read("measured-final-validation.json")
assert len(measured) == 4 and all(x["exit_code"] == 0 for x in measured)
assert total(measured[0]["stdout"]) == (172,0,4)
assert total(measured[2]["stdout"]) == (263,0,1)
assert "complex_values=688" in measured[0]["stdout"]
assert "directions=43" in measured[0]["stdout"]
assert "cases=9 failures=8" in read("xtc-measured-pose-baseline.json")["stdout"]
assert "cases=9 failures=0" in read("xtc-measured-pose-fixed.json")["stdout"]
assert "sampled_cues=432 conservatively_certified_columns=8" in measured[2]["stdout"]
assert "cues=140 degraded=4" in measured[2]["stdout"]
for mode in ["false","true"]:
    assert "rendered_xtc_summary normalization="+mode+" cases=600 degraded=146" in measured[2]["stdout"]
final = read("spatial-final-verification.json")
assert len(final) == 3 and all(x["exit_code"] == 0 for x in final)
assert total(final[0]["stdout"]) == (133,0,4)
assert "cues=140 degraded=4" in final[1]["stdout"]
# Synthetic inverse/filter/gain code is unchanged by the resource-pose fix.
for path in (REPO/"crates/sotf-plugins/crates/sotf-plugin-xtc/src/filters").glob("*.rs"):
    relative = str(path.relative_to(REPO))
    prior = subprocess.run(["rtk","proxy","git","show","b481bd8:"+relative], cwd=REPO,capture_output=True,check=True).stdout
    assert path.read_bytes() == prior, relative
flat = read("upmixer-clap-flat-baseline.json")
assert flat["exit_code"] == 1 and "Could not open bundle" in flat["stderr"]
bundle = read("upmixer-clap-bundle-validation.json")
assert bundle["install_migrates_flat_to_bundle"]
assert all(x["exit_code"] == 0 for x in bundle["commands"])
clap = read("upmixer-clap-functional-validation.json")
assert clap["exit_code"] == 0 and clap["library_sha256"] == bundle["library_sha256"]
statuses = collections.Counter()
def walk(value):
    if isinstance(value,dict):
        if "status" in value and isinstance(value["status"],dict): statuses[value["status"].get("code")]+=1
        for child in value.values(): walk(child)
    elif isinstance(value,list):
        for child in value: walk(child)
walk(json.loads(clap["stdout"]))
assert statuses == {"success":15,"skipped":5}
motion = read("binaural-measured-motion.json")
assert motion["exit_code"] == 0
assert read("binaural-measured-motion-source.json")["hashes_unchanged_after_run"]
rows = [line.split("MEASURED_MOTION ",1)[1] for line in motion["stdout"].splitlines() if "MEASURED_MOTION channels=" in line]
assert len(rows) == 6
motion_rows = []
for line in rows:
    header, durations = line.split(" durations_ns=",1)
    times = json.loads(durations.split(" scope=",1)[0])
    assert len(times) == 128
    fields = {k:int(v) for k,v in re.findall(r"(\w+)=(\d+)", header)}
    assert fields["final_generation"] > fields["baseline_generation"]
    assert 0 < fields["publication_observed_ns"] <= fields["adoption_observed_ns"] <= fields["numerical_audio_observed_ns"]
    assert sum(t > 128/44100*1e9 for t in times) == fields["physical_callback_misses"] == 0
    motion_rows.append(fields)
assert "baseline_generation" not in read("binaural-measured-motion-startup-contaminated.json")["stdout"]
# Recompute retained summary values from raw command output rather than labels.
quality_line = re.search(r"MEASURED_KEMAR_HELD_OUT_SUMMARY directions=43 maximum_band_ild_error_db=(\[[^\]]+\]) maximum_wrapped_interaural_phase_error_rad=(\[[^\]]+\])", measured[0]["stdout"])
assert quality_line
ild_errors, phase_errors = map(json.loads, quality_line.groups())
assert ild_errors == [1.2642868940204028, 1.5801768539843568]
assert phase_errors == [2.947626039425671, 2.9478853950050214]
center_before = float(re.search(r"RECORDED_CENTER_SUMMARY cases=20 maximum_surround_center_ratio=([0-9.e+-]+)", read("upmixer-recorded-center.json")["stdout"])[1])
center_after = float(re.search(r"RECORDED_CENTER_SUMMARY cases=20 maximum_surround_center_ratio=([0-9.e+-]+)", read("upmixer-coherence-candidate-validation.json")[-1]["stdout"])[1])
assert center_after < center_before < 0.05
measured_xtc = re.search(r"MEASURED_KEMAR_XTC_SUMMARY cues=140 degraded=(\d+) worst_separation_improvement_db=([0-9.e+-]+)", final[1]["stdout"])
assert measured_xtc and int(measured_xtc[1]) == 4
bundle_binary = Path(bundle["bundle"])/"Contents/MacOS/sotf_upmixer"
assert hashlib.sha256(bundle_binary.read_bytes()).hexdigest() == bundle["library_sha256"]
assert list(json.loads(clap["stdout"])["plugin-tests"]) == ["org.spinorama.sotf.upmixer"]
summary = {
    "current_source_verified":True,
    "all_three_plugins_remain_partial":True,
    "binaural_tests":[172,0,4], "upmixer_tests":[229,0,3],"xtc_tests":[263,0,1],
    "strict_clippy_passed":True,
    "native_clap":{"success":15,"unsupported_skips":5,"scan_time_evaluated":False},
    "recorded_center_max_ratio_before":center_before,"recorded_center_max_ratio_after":center_after,
    "measured_binaural_quality":{"held_out_directions":43,"legacy_max_band_ild_error_db":1.2642868940204028,"directional_max_band_ild_error_db":1.5801768539843568,"legacy_max_wrapped_ipd_error_rad":2.947626039425671,"directional_max_wrapped_ipd_error_rad":2.9478853950050214,"quality_improvement_established":False},
    "measured_xtc":{"pose_lookup_failures_before":8,"pose_lookup_failures_after":0,"cases":9,"rendered_cues":140,"degraded_cues":4,"worst_separation_improvement_db":float(measured_xtc[2])},
    "default_synthetic_xtc_filter_code_unchanged":True,
    "motion":{"trials":6,"callbacks":768,"physical_callback_misses":0,"heap_events":0,"startup_contaminated_trial_accepted":False,"rows":motion_rows,"scope":"Paced software output with held-pose reference; not hardware or audibility acceptance."},
    "remaining":["Binaural enclosing mesh, donor/null/delay analysis, measured continuous-motion/listening and native/max-dataset validation","Upmixer mixed-programme/downmix/onset/listening, external DAW PDC, non-macOS native and loaded worst-layout timing","XTC existing LF5dB and146/600mismatch failures, calibrated/measured robustness, native/head-tracking/listening"],
}
(ART/"spatial-summary.json").write_text(json.dumps(summary,indent=2)+"\n")
print(json.dumps({k:v for k,v in summary.items() if k not in ["motion","remaining"]},indent=2))
