"""Independently check the retained denoiser recurrence and recorded A/B evidence."""
import hashlib
import itertools
import json
import math
import re
from pathlib import Path
ROOT=Path(__file__).resolve().parent
REPO=ROOT.parents[2]
old=json.loads((ROOT/"denoiser-fast-adaptation-baseline.json").read_text())
new=json.loads((ROOT/"denoiser-fast-adaptation-candidate.json").read_text())
assert len(old)==2 and old[0]["exit_code"]==101 and old[1]["exit_code"]==0
assert "fast_adaptation_reference cases=120 failures=33" in old[0]["stdout"]
assert new["exit_code"]==0 and "fast_adaptation_reference cases=120 failures=0" in new["stdout"]
source=json.loads((ROOT/"denoiser-fast-adaptation-source.json").read_text())
for name,sha in source["sources"].items():assert hashlib.sha256((REPO/name).read_bytes()).hexdigest()==sha
selection=json.loads((ROOT/"denoiser-fast-adaptation-selection.json").read_text())
assert len(selection["sources"])==6
for record in selection["sources"]:
    assert hashlib.sha256((REPO/record["file"]).read_bytes()).hexdigest()==record["sha256"]
    assert hashlib.sha256((REPO/record["provenance_manifest"]).read_bytes()).hexdigest()==record["provenance_manifest_sha256"]
def objects(output):return [json.loads(l) for l in output.splitlines() if l.startswith("{")]
key=lambda r:(r["source"],r["condition"],r["low_latency"])
def checked(output):
    rows=[r for r in objects(output) if r.get("measurement")=="recorded_estimator"]
    expected=set(itertools.product(["speech-1272","speech-1462","speech-174","phoneme-s","violin","cymbal"],["clean","stationary","step"],[False,True]))
    assert len(rows)==36 and {key(r) for r in rows}==expected
    for r in rows:
        assert r["latency_frames"]==(512 if r["low_latency"] else 2048)
        assert r["aligned_after_source_energy"]>=0
        assert [w["label"] for w in r["windows"]]==["onset","early","body","full"]
        assert [(w["start"],w["end"]) for w in r["windows"]]==[(0,4800),(4800,24000),(24000,r["frames"]),(0,r["frames"])]
        for w in r["windows"]:
            for mode in ["input","output"]:
                m=w[mode];c,e,d,x=(m[k] for k in ["clean_energy","output_energy","dot","error_energy"])
                assert all(math.isfinite(v) for v in m.values()) and c>0 and e>=0 and x>=0
                assert abs(m["projection"]-d/c)<1e-12
                assert abs(m["sdr_db"]-10*math.log10(c/max(x,1e-30)))<1e-10
                assert abs(x-(c+e-2*d))<1e-10*max(c,e,1.)
        if r["condition"]!="clean":assert abs(r["windows"][-1]["input"]["sdr_db"])<1e-5
    return {key(r):r for r in rows}
a,b=checked(old[1]["stdout"]),checked(new["stdout"])
changes=[]
for k,r in b.items():
    for before,after in zip(a[k]["windows"],r["windows"]):
        assert before["input"]==after["input"]
        changes.append(dict(source=k[0],condition=k[1],low_latency=k[2],window=after["label"],sdr_delta_db=after["output"]["sdr_db"]-before["output"]["sdr_db"],projection_delta=after["output"]["projection"]-before["output"]["projection"]))
full=[r["sdr_delta_db"] for r in changes if r["window"]=="full"]
# This is a recurrence correction, not a predeclared quality gate. Retain every
# signed delta, including tiny regressions, and the still-failed vowel retention.
quality=[r for r in objects(new["stdout"]) if r.get("measurement")=="recorded_denoiser_onset"]
assert len(quality)==2 and all(r["vowel_300_450ms_clean_projection"]<0.13 for r in quality)
validation=json.loads((ROOT/"denoiser-fast-adaptation-validation.json").read_text())
assert len(validation)==4 and all(r["exit_code"]==0 for r in validation)
counts=[tuple(map(int,m)) for m in re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored",validation[0]["stdout"])]
assert sum(r[0] for r in counts)==127 and sum(r[1] for r in counts)==0 and sum(r[2] for r in counts)==4
assert "[ALL PASS] Denoiser QA Complete." in validation[2]["stdout"]
assert len(re.findall(r"ch fft=\d+ block=\d+ modes=",validation[2]["stdout"]))==48
assert checked(validation[0]["stdout"])==b
print(json.dumps(dict(recurrence_reference_cases=120,baseline_failures=33,candidate_failures=0,passed_tests=sum(r[0] for r in counts),existing_ignored=sum(r[2] for r in counts),recorded_conditions=36,window_comparisons=len(changes),full_sdr_delta_min_db=min(full),full_sdr_delta_max_db=max(full),default_legacy_acceleration=math.log(.45)/math.log(.9),default_corrected_acceleration=math.log(.81)/math.log(.9),recorded_original_onset=quality,signed_changes=changes,scope="Exact two-update fast-adaptation recurrence plus controlled development recordings and existing gates; not resolution of vowel loss, held-out quality or native-device timing."),indent=2))
