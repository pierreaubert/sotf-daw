"""Verify coalesced NUPC correctness and complete unchanged-workload load evidence."""
import hashlib
import itertools
import json
import math
import re
from pathlib import Path
ROOT=Path(__file__).resolve().parent
REPO=ROOT.parents[2]
read=lambda name:json.loads((ROOT/name).read_text())
baseline=read("nupc-coalesced-baseline.json")
assert baseline["exit_code"]==101
assert "nupc_coalesced_coverage cases=120 failures=60 original_levels=437 actual_levels=437" in baseline["stdout"]
assert "nupc_coalesced_unmerged cases=45 maximum_error=0e0" in baseline["stdout"]
validation=read("nupc-coalesced-validation.json")
assert len(validation)==4 and all(r["exit_code"]==0 for r in validation)
output=validation[0]["stdout"]
counts=[tuple(map(int,m)) for m in re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored",output)]
assert tuple(sum(c[i] for c in counts) for i in range(3))==(263,0,6)
assert "nupc_coalesced_coverage cases=120 failures=0 original_levels=437 actual_levels=248" in output
errors={}
for name,cases in [("nupc_coalesced_unmerged",45),("nupc_real_direct_reference",54),("nupc_large_sparse_reference",4)]:
    m=re.search(name+r" cases=(\d+) maximum_error=([0-9.e+-]+)",output)
    assert m and int(m[1])==cases and float(m[2])<2e-5
    errors[name]=float(m[2])
assert "nupc_half_spectra checks=108 failures=0" in output
assert "Physical callback misses: 0." in validation[2]["stdout"]
source=read("nupc-coalesced-source.json")
assert all(source["unchanged_from_real_fft_baseline"].values())
for name,sha in source["source_sha256"].items():assert hashlib.sha256((REPO/name).read_bytes()).hexdigest()==sha,name
for name,sha in source["baseline_artifact_sha256"].items():assert hashlib.sha256((ROOT/name).read_bytes()).hexdigest()==sha,name
previous=read("nupc-real-fft-load.json")
current=read("nupc-coalesced-load.json")
key=lambda r:tuple(r["geometry"][k] for k in ["rate_hz","output_channels","ir_channels","true_stereo","nupc","head"])
expected={(rate,out,ir,matrix,nupc,head) for rate,(out,ir,matrix),(nupc,head) in itertools.product([48000,96000,192000],[(2,1,False),(2,2,False),(12,1,False),(12,12,False),(32,1,False),(32,32,False),(2,4,True)],[(False,False),(True,False),(True,True)])}
def load_rows(data):
    results=[r for r in data["rows"] if r["event"]=="case_result"]
    assert len(results)==63 and {key(r) for r in results}==expected
    assert not any(r["event"]=="case_error" for r in data["rows"])
    failed=misses=callbacks=0
    for row in results:
        assert row["retained_delta_within_estimate"] and row["construction_retained_requested_delta_bytes"]<=row["estimate_bytes"]
        phases=[row["warm"],*row["steady"]]
        assert len(phases)==3
        for p in phases:
            rate=row["geometry"]["rate_hz"];frames=p["callback_frames"]
            assert math.isclose(p["physical_budget_us"],frames/rate*1e6,rel_tol=1e-12)
            assert math.isclose(p["reported_budget_us"],max(frames,p["reported_quantum_frames"])/rate*1e6,rel_tol=1e-12)
            assert p["allocations"]==p["frees"]==0 and p["finite"]
            assert 0<=p["reported_misses"]<=p["physical_misses"]<=p["callbacks"]
            assert 0<=p["p50_us"]<=p["p95_us"]<=p["p99_us"]<=p["max_us"]
            callbacks+=p["callbacks"];misses+=p["physical_misses"]
        failed+=int(any(p["physical_misses"] or p["reported_misses"] for p in phases))
    summary=[r for r in data["rows"] if r["event"]=="summary"]
    assert len(summary)==1;summary=summary[0]
    assert summary["cases"]==63 and summary["errors"]==0 and summary["failed_cases"]==failed and summary["physical_misses"]==misses and summary["timing_margin"]==1.0
    assert summary["pass"]==(failed==0) and data["exit_code"]==(1 if failed else 0)
    return {key(r):r for r in results},dict(cases=63,callbacks=callbacks,failed_cases=failed,physical_misses=misses,deadline_pass=summary["pass"])
a,old=load_rows(previous);b,new=load_rows(current)
comparisons=[]
for k,row in b.items():
    before=a[k]
    for field in ["ir_frames","estimate_bytes","latency_frames","next_frame_rejection"]:assert row[field]==before[field],(k,field)
    for left,right in zip([row["warm"],*row["steady"]],[before["warm"],*before["steady"]]):
        for field in ["callback_frames","callbacks","reported_quantum_frames","physical_budget_us","reported_budget_us"]:assert left[field]==right[field],(k,field)
    comparisons.append(dict(geometry=row["geometry"],old_misses=sum(p["physical_misses"] for p in [before["warm"],*before["steady"]]),new_misses=sum(p["physical_misses"] for p in [row["warm"],*row["steady"]]),old_retained_bytes=before["construction_retained_requested_delta_bytes"],new_retained_bytes=row["construction_retained_requested_delta_bytes"],retained_ratio=row["construction_retained_requested_delta_bytes"]/before["construction_retained_requested_delta_bytes"]))
nupc=[r for r in comparisons if r["geometry"]["nupc"]]
print(json.dumps(dict(functional_verified=True,passed_tests=263,existing_ignored=6,structural_cases=120,baseline_structural_failures=60,candidate_structural_failures=0,original_levels=437,coalesced_levels=248,maximum_errors=errors,load=new,previous_load=old,nupc_fewer_misses=sum(r["new_misses"]<r["old_misses"] for r in nupc),nupc_equal_misses=sum(r["new_misses"]==r["old_misses"] for r in nupc),nupc_more_misses=sum(r["new_misses"]>r["old_misses"] for r in nupc),retained_ratio_min=min(r["retained_ratio"] for r in nupc),retained_ratio_max=max(r["retained_ratio"] for r in nupc),comparisons=comparisons,scope="Same public partition plan, admitted IRs, callbacks, horizons and raw deadline margin. Historical timings are unmatched observations; preserve all failures, with no native-device/RSS/aggregate memory acceptance claim."),indent=2))
