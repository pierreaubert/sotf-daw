"""Validate Distributed UPC tail work, accuracy, admission and complete raw load evidence."""
import difflib
import hashlib
import itertools
import json
import math
import re
import subprocess
from pathlib import Path
ROOT=Path(__file__).resolve().parent
REPO=ROOT.parents[2]
read=lambda name:json.loads((ROOT/name).read_text())
validation=read("upc-distributed-validation.json")
assert len(validation)==4 and all(r["exit_code"]==0 for r in validation)
output=validation[0]["stdout"]
counts=[tuple(map(int,m)) for m in re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored",output)]
assert tuple(sum(c[i] for c in counts) for i in range(3))==(268,0,7),counts
baseline=read("upc-distributed-baseline.json");assert baseline["exit_code"]==101
pattern=r"upc_early_tail frames=128 maximum_error=([0-9.e+-]+)"
red=re.search(pattern,baseline["stdout"]);assert red and float(red[1])>.3
focused=read("upc-distributed-focused.json");assert focused["exit_code"]==0
green=re.search(pattern,focused["stdout"]);assert green and float(green[1])<2e-5
assert "upc_partial_tail_recovery cases=4 bit_exact=true" in output
assert re.search(pattern,output) and float(re.search(pattern,output)[1])<2e-5
cache=re.search(r"upc_real_cache cases=(\d+) checks=(\d+) failures=(\d+) dft_checks=(\d+) maximum_error=([0-9.e+-]+)",output)
assert cache and tuple(map(int,cache.groups()[:4]))==(12,140,0,240) and float(cache[5])<2e-5
assert "Physical callback misses: 0." in validation[2]["stdout"]
restored=read("upc-distributed-restored-validation.json")
assert len(restored)==2 and all(r["exit_code"]==0 for r in restored)
restored_counts=[tuple(map(int,m)) for m in re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored",restored[0]["stdout"])]
assert tuple(sum(c[i] for c in restored_counts) for i in range(3))==(105,0,4)
source=read("upc-distributed-source.json")
assert source["hashes_unchanged_after_load"]
assert source["candidate_rejected"]
for name,sha in source["source_sha256"].items():
    snapshot=source["candidate_snapshot_paths"].get(name,name)
    assert hashlib.sha256((REPO/snapshot).read_bytes()).hexdigest()==sha,name
for name,sha in source["restored_baseline_sha256"].items():
    baseline_bytes=subprocess.run(["rtk","proxy","git","show",source["base_commit"]+":"+name],capture_output=True,check=True).stdout
    assert hashlib.sha256(baseline_bytes).hexdigest()==sha,name
    # Documentation may append the recorded rejection; DSP sources are exact.
    if name in source["candidate_snapshot_paths"]:assert (REPO/name).read_bytes()==baseline_bytes,name
patch=""
for name,snapshot in source["candidate_snapshot_paths"].items():
    baseline=subprocess.run(["rtk","proxy","git","show",source["base_commit"]+":"+name],capture_output=True,text=True,check=True).stdout
    patch+="".join(difflib.unified_diff(baseline.splitlines(True),(REPO/snapshot).read_text().splitlines(True),fromfile="a/"+name,tofile="b/"+name,n=0))
assert (ROOT/"upc-distributed-candidate.patch").read_text()==patch
assert hashlib.sha256(patch.encode()).hexdigest()==source["candidate_patch_sha256"]
for name,sha in source["baseline_artifact_sha256"].items():assert hashlib.sha256((ROOT/name).read_bytes()).hexdigest()==sha,name
plugin="crates/sotf-plugins/crates/sotf-plugin-convolution/src/lib/convolution_plugin.rs"
before=subprocess.run(["rtk","proxy","git","show",source["base_commit"]+":"+plugin],capture_output=True,text=True,check=True).stdout
after=(REPO/source["candidate_snapshot_paths"][plugin]).read_text()
def admission(s):
    a=s.index("    pub(super) fn estimated_ordinary_backend_bytes(")
    b=s.index("    pub fn new<",a)
    return re.sub(r"\s+","",re.sub(r"//[^\n]*","",s[a:b]))
assert admission(before)==admission(after),"admission arithmetic changed"
harness="crates/sotf-plugins/crates/sotf-plugin-convolution/bin/qa_convolution_load.rs"
assert subprocess.run(["rtk","proxy","git","show",source["base_commit"]+":"+harness],capture_output=True,check=True).stdout==(REPO/harness).read_bytes()
old=read("upc-real-fft-load.json");new=read("upc-distributed-load.json")
assert [json.loads(line) for line in (ROOT/"upc-distributed-load.jsonl").read_text().splitlines() if line.strip()]==new["rows"]
key=lambda r:tuple(r["geometry"][k] for k in ["rate_hz","output_channels","ir_channels","true_stereo","nupc","head"])
expected={(rate,out,ir,matrix,nupc,head) for rate,(out,ir,matrix),(nupc,head) in itertools.product([48000,96000,192000],[(2,1,False),(2,2,False),(12,1,False),(12,12,False),(32,1,False),(32,32,False),(2,4,True)],[(False,False),(True,False),(True,True)])}
def load(data):
    rows=[r for r in data["rows"] if r["event"]=="case_result"]
    assert len(rows)==63 and {key(r) for r in rows}==expected
    assert not any(r["event"]=="case_error" for r in data["rows"])
    failed=misses=callbacks=0
    for r in rows:
        assert r["retained_delta_within_estimate"] and r["construction_retained_requested_delta_bytes"]<=r["estimate_bytes"]
        phases=[r["warm"],*r["steady"]];assert len(phases)==3
        for p in phases:
            rate=r["geometry"]["rate_hz"];frames=p["callback_frames"]
            assert math.isclose(p["physical_budget_us"],frames/rate*1e6,rel_tol=1e-12)
            assert math.isclose(p["reported_budget_us"],max(frames,p["reported_quantum_frames"])/rate*1e6,rel_tol=1e-12)
            assert p["allocations"]==p["frees"]==0 and p["finite"]
            assert 0<=p["reported_misses"]<=p["physical_misses"]<=p["callbacks"]
            assert 0<=p["p50_us"]<=p["p95_us"]<=p["p99_us"]<=p["max_us"]
            callbacks+=p["callbacks"];misses+=p["physical_misses"]
        failed+=int(any(p["physical_misses"] or p["reported_misses"] for p in phases))
    summary=[r for r in data["rows"] if r["event"]=="summary"];assert len(summary)==1;summary=summary[0]
    assert summary["cases"]==63 and summary["errors"]==0 and summary["failed_cases"]==failed and summary["physical_misses"]==misses and summary["timing_margin"]==1.0
    assert summary["pass"]==(failed==0) and data["exit_code"]==(1 if failed else 0)
    return {key(r):r for r in rows},dict(cases=63,callbacks=callbacks,failed_cases=failed,physical_misses=misses,deadline_pass=summary["pass"])
a,old_summary=load(old);b,new_summary=load(new);comparisons=[]
for k,r in b.items():
    previous=a[k]
    for field in ["ir_frames","estimate_bytes","latency_frames","next_frame_rejection"]:assert r[field]==previous[field],(k,field)
    for left,right in zip([r["warm"],*r["steady"]],[previous["warm"],*previous["steady"]]):
        for field in ["callback_frames","callbacks","reported_quantum_frames","physical_budget_us","reported_budget_us"]:assert left[field]==right[field],(k,field)
    phases=[]
    for before_phase,after_phase in zip([previous["warm"],*previous["steady"]],[r["warm"],*r["steady"]]):
        phases.append(dict(callback_frames=after_phase["callback_frames"],callbacks=after_phase["callbacks"],budget_us=after_phase["physical_budget_us"],old_p99_us=before_phase["p99_us"],new_p99_us=after_phase["p99_us"],old_max_us=before_phase["max_us"],new_max_us=after_phase["max_us"],old_misses=before_phase["physical_misses"],new_misses=after_phase["physical_misses"]))
    comparisons.append(dict(geometry=r["geometry"],old_misses=sum(p["old_misses"] for p in phases),new_misses=sum(p["new_misses"] for p in phases),old_retained_bytes=previous["construction_retained_requested_delta_bytes"],new_retained_bytes=r["construction_retained_requested_delta_bytes"],phases=phases))


upc=[r for r in comparisons if not r["geometry"]["nupc"]]
nupc=[r for r in comparisons if r["geometry"]["nupc"]]
assert len(upc)==21 and len(nupc)==42
ratios=[r["new_retained_bytes"]/r["old_retained_bytes"] for r in upc]
print(json.dumps(dict(candidate_functional_verified=True,candidate_rejected=True,production_dsp_restored_exactly=True,restored_tests_passed=105,restored_tests_ignored=4,restored_strict_clippy_passed=True,normal_tests_passed=268,normal_tests_ignored=7,cache_checks=140,dft_checks=240,maximum_dft_error=float(cache[5]),early_tail_maximum_error=float(green[1]),admission_arithmetic_unchanged=True,load_harness_unchanged=True,load=new_summary,previous_load=old_summary,upc_old_misses=sum(r["old_misses"] for r in upc),upc_new_misses=sum(r["new_misses"] for r in upc),nupc_old_misses=sum(r["old_misses"] for r in nupc),nupc_new_misses=sum(r["new_misses"] for r in nupc),upc_geometries_fewer_misses=sum(r["new_misses"]<r["old_misses"] for r in upc),upc_geometries_equal_misses=sum(r["new_misses"]==r["old_misses"] for r in upc),upc_geometries_more_misses=sum(r["new_misses"]>r["old_misses"] for r in upc),upc_retained_ratio_min=min(ratios),upc_retained_ratio_max=max(ratios),comparisons=comparisons,scope="Full local Plugin workload at unchanged admitted geometries. Historical timing is unmatched. No device deadline or new linear-EQ timing acceptance claim."),indent=2))
