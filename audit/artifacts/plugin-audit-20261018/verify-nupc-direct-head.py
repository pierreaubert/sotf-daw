"""Verify direct-head accuracy, the declared paired guard and complete load evidence."""
import hashlib
import itertools
import json
import math
import re
import statistics
from pathlib import Path
ROOT=Path(__file__).resolve().parent
REPO=ROOT.parents[2]
read=lambda name:json.loads((ROOT/name).read_text())
validation=read("nupc-direct-head-validation.json")
assert len(validation)==4 and all(r["exit_code"]==0 for r in validation)
output=validation[0]["stdout"]
counts=[tuple(map(int,m)) for m in re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored",output)]
assert tuple(sum(c[i] for c in counts) for i in range(3))==(264,0,7)
errors={}
for name,cases in [("direct_head_f64",18),("nupc_coalesced_unmerged",45),("nupc_real_direct_reference",54),("nupc_large_sparse_reference",4)]:
    m=re.search(name+r" cases=(\d+) maximum_error=([0-9.e+-]+)",output)
    assert m and int(m[1])==cases and float(m[2])<2e-5
    errors[name]=float(m[2])
initial=read("nupc-direct-head-initial-validation.json")
assert initial[0]["exit_code"]==0 and initial[1]["exit_code"]==101 and "manual_slice_fill" in initial[1]["stderr"]
assert "Physical callback misses: 0." in validation[2]["stdout"]
profile=read("nupc-direct-head-final-profile.json");assert profile["exit_code"]==0
pattern=r"direct_head_cost taps=(\d+) trial=(\d+) candidate=(true|false) channels=(\d+) frames=(\d+) calls=(\d+) checksum=([^ ]+) ns=(\[[^\n]+\])"
rows=[]
for m in re.finditer(pattern,profile["stdout"]):
    ns=json.loads(m[8]);assert len(ns)==int(m[6])==32 and all(isinstance(n,int) and n>0 for n in ns)
    assert int(m[4])==32 and int(m[5])==128 and math.isfinite(float(m[7]))
    rows.append(dict(taps=int(m[1]),trial=int(m[2]),candidate=m[3]=="true",p50_ns=statistics.median(ns)))
assert len(rows)==12 and {(r["taps"],r["trial"]) for r in rows}==set(itertools.product([32,257,512],range(4)))
paired=[]
for taps in [32,257,512]:
    rs=sorted((r for r in rows if r["taps"]==taps),key=lambda r:r["trial"])
    assert [r["candidate"] for r in rs]==[False,True,True,False]
    median=lambda flag:statistics.median(r["p50_ns"] for r in rs if r["candidate"]==flag)
    ratio=median(True)/median(False)
    if taps in [257,512]:assert ratio<=.9
    paired.append(dict(taps=taps,legacy_p50_ns=median(False),candidate_p50_ns=median(True),ratio=ratio))
assembly=read("nupc-direct-head-final-assembly.json")
assert assembly["exit_code"]==0 and assembly["packed_arithmetic"]
assert hashlib.sha256((ROOT/"nupc-direct-head-final-dot-disassembly.txt").read_bytes()).hexdigest()==assembly["disassembly_sha256"]
disassembly=(ROOT/"nupc-direct-head-final-dot-disassembly.txt").read_text()
observed=[l.strip() for l in disassembly.splitlines() if re.search(r"\b(fmul|fadd|fmla)(?:\.4s\b|\s+v[^\n]*\.4s)",l)]
assert observed==assembly["packed_arithmetic"] and observed
source=read("nupc-direct-head-source.json")
assert all(source["unchanged_from_coalesced_baseline"].values())
for name,sha in source["source_sha256"].items():assert hashlib.sha256((REPO/name).read_bytes()).hexdigest()==sha,name
for name,sha in source["baseline_artifact_sha256"].items():assert hashlib.sha256((ROOT/name).read_bytes()).hexdigest()==sha,name
old=read("nupc-coalesced-load.json");new=read("nupc-direct-head-load.json")
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
heads=[r for r in comparisons if r["geometry"]["head"]]
print(json.dumps(dict(functional_verified=True,normal_tests_passed=264,normal_tests_ignored=7,existing_ignored=6,explicit_cost_diagnostic_ignored_by_default=1,explicit_cost_diagnostic_executed=True,maximum_errors=errors,paired_cost_guard_passed=True,paired_cost=paired,load=new_summary,previous_load=old_summary,head_geometries_fewer_misses=sum(r["new_misses"]<r["old_misses"] for r in heads),head_geometries_equal_misses=sum(r["new_misses"]==r["old_misses"] for r in heads),head_geometries_more_misses=sum(r["new_misses"]>r["old_misses"] for r in heads),comparisons=comparisons,scope="Paired isolated heads-only wall timing plus unchanged full Plugin workload. Full-load historical timings are unmatched. No native-device deadline, arbitrary-architecture speedup or new linear-EQ timing acceptance claim."),indent=2))
