"""Validate UPC real-transform accuracy, conservative admission and full raw load evidence."""
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
validation=read("upc-real-fft-validation.json")
assert len(validation)==4 and all(r["exit_code"]==0 for r in validation)
output=validation[0]["stdout"]
counts=[tuple(map(int,m)) for m in re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored",output)]
assert tuple(sum(c[i] for c in counts) for i in range(3))==(266,0,7),counts
pattern=r"upc_real_cache cases=(\d+) checks=(\d+) failures=(\d+) dft_checks=(\d+) maximum_error=([0-9.e+-]+)"
baseline=read("upc-real-fft-baseline.json");assert baseline["exit_code"]==101
red=re.search(pattern,baseline["stdout"]);assert red and tuple(map(int,red.groups()[:4]))==(12,140,140,240)
focused=read("upc-real-fft-focused.json");assert focused["exit_code"]==0
green=re.search(pattern,focused["stdout"]);assert green and tuple(map(int,green.groups()[:4]))==(12,140,0,240) and float(green[5])<2e-5
m=re.search(r"upc_real_plan allocated_bytes=(\d+) reserve_bytes=(\d+) scratch_complex=(\d+)",output)
assert m and int(m[1])<=int(m[2]) and int(m[3])<=8*2048
assert "Physical callback misses: 0." in validation[2]["stdout"]
source=read("upc-real-fft-source.json")
assert source["hashes_unchanged_after_load"]
for name,sha in source["source_sha256"].items():assert hashlib.sha256((REPO/name).read_bytes()).hexdigest()==sha,name
for name,sha in source["baseline_artifact_sha256"].items():assert hashlib.sha256((ROOT/name).read_bytes()).hexdigest()==sha,name
plugin="crates/sotf-plugins/crates/sotf-plugin-convolution/src/lib/convolution_plugin.rs"
before=subprocess.run(["rtk","proxy","git","show",source["base_commit"]+":"+plugin],capture_output=True,text=True,check=True).stdout
after=(REPO/plugin).read_text()
def admission(s):
    a=s.index("    pub(super) fn estimated_ordinary_backend_bytes(")
    b=s.index("    pub fn new<",a)
    return re.sub(r"\s+","",re.sub(r"//[^\n]*","",s[a:b]))
assert admission(before)==admission(after),"admission arithmetic changed"
harness="crates/sotf-plugins/crates/sotf-plugin-convolution/bin/qa_convolution_load.rs"
assert subprocess.run(["rtk","proxy","git","show",source["base_commit"]+":"+harness],capture_output=True,check=True).stdout==(REPO/harness).read_bytes()
old=read("nupc-direct-head-load.json");new=read("upc-real-fft-load.json")
assert [json.loads(line) for line in (ROOT/"upc-real-fft-load.jsonl").read_text().splitlines() if line.strip()]==new["rows"]
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
    comparisons.append(dict(geometry=r["geometry"],old_misses=sum(p["physical_misses"] for p in [previous["warm"],*previous["steady"]]),new_misses=sum(p["physical_misses"] for p in [r["warm"],*r["steady"]]),old_retained_bytes=previous["construction_retained_requested_delta_bytes"],new_retained_bytes=r["construction_retained_requested_delta_bytes"]))

upc=[r for r in comparisons if not r["geometry"]["nupc"]]
nupc=[r for r in comparisons if r["geometry"]["nupc"]]
assert len(upc)==21 and len(nupc)==42
ratios=[r["new_retained_bytes"]/r["old_retained_bytes"] for r in upc]
print(json.dumps(dict(functional_verified=True,normal_tests_passed=266,normal_tests_ignored=7,cache_checks=140,dft_checks=240,maximum_dft_error=float(green[5]),real_plan_allocated_bytes=int(m[1]),real_plan_reserve_bytes=int(m[2]),scratch_complex=int(m[3]),admission_arithmetic_unchanged=True,load_harness_unchanged=True,load=new_summary,previous_load=old_summary,upc_old_misses=sum(r["old_misses"] for r in upc),upc_new_misses=sum(r["new_misses"] for r in upc),nupc_old_misses=sum(r["old_misses"] for r in nupc),nupc_new_misses=sum(r["new_misses"] for r in nupc),upc_retained_ratio_min=min(ratios),upc_retained_ratio_max=max(ratios),comparisons=comparisons,scope="Full local Plugin workload at unchanged admitted geometries. Historical timing is unmatched. No device deadline or new linear-EQ timing acceptance claim."),indent=2))
