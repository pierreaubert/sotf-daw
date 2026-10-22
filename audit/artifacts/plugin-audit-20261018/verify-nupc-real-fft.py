"""Verify NUPC half spectra, unchanged workload geometry and complete failing load data."""
import hashlib
import itertools
import json
import math
import re
from pathlib import Path
ROOT=Path(__file__).resolve().parent
REPO=ROOT.parents[2]
read=lambda name:json.loads((ROOT/name).read_text())
old=read("nupc-real-fft-baseline.json")
assert old["exit_code"]==101 and "nupc_half_spectra checks=132 failures=120" in old["stdout"]
assert "nupc_real_direct_reference cases=54" in old["stdout"]
focused=read("nupc-real-fft-final-focused.json")
assert focused["exit_code"]==0
assert "nupc_half_spectra checks=132 failures=0 legacy_complex_values=87360 actual_complex_values=43812" in focused["stdout"]
for cases,name in [(54,"nupc_real_direct_reference"),(4,"nupc_large_sparse_reference")]:
    match=re.search(name+r" cases=(\d+) maximum_error=([0-9.e+-]+)",focused["stdout"])
    assert match and int(match[1])==cases and float(match[2])<2e-5
validation=read("nupc-real-fft-validation.json")
consumers=read("nupc-real-fft-consumer-validation.json")
assert len(validation)==3 and len(consumers)==3 and all(r["exit_code"]==0 for r in validation+consumers)
counts=lambda output:[tuple(map(int,m)) for m in re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored",output)]
assert sum(r[0] for r in counts(validation[0]["stdout"]))==115
assert sum(r[1] for r in counts(validation[0]["stdout"]))==0
assert sum(r[2] for r in counts(validation[0]["stdout"]))==4
assert sum(r[0] for r in counts(consumers[1]["stdout"]))==145
assert sum(r[1] for r in counts(consumers[1]["stdout"]))==0
assert sum(r[2] for r in counts(consumers[1]["stdout"]))==2
assert "Physical callback misses: 0." in validation[2]["stdout"]
source=read("nupc-real-fft-source.json")
assert source["admission_arithmetic_unchanged"] and all(source["historical_audio_functions_equal"].values())
for name,sha in source["source_sha256"].items():
    assert hashlib.sha256((REPO/name).read_bytes()).hexdigest()==sha,name
assert all(r["same"] for r in read("nupc-real-fft-historical-baseline-check.json"))
load=read("nupc-real-fft-load.json")
previous=read("convolution-maximum-load.json")
key=lambda r:tuple(r["geometry"][k] for k in ["rate_hz","output_channels","ir_channels","true_stereo","nupc","head"])
expected={(rate,out,ir,matrix,nupc,head) for rate,(out,ir,matrix),(nupc,head) in itertools.product([48000,96000,192000],[(2,1,False),(2,2,False),(12,1,False),(12,12,False),(32,1,False),(32,32,False),(2,4,True)],[(False,False),(True,False),(True,True)])}
old_rows={key(r):r for r in previous["rows"] if r["event"]=="case_result"}
new_rows={key(r):r for r in load["rows"] if r["event"]=="case_result"}
assert len(new_rows)==63 and new_rows.keys()==old_rows.keys()==expected
assert not any(r["event"]=="case_error" for r in load["rows"])
failed=misses=callbacks=0
comparisons=[]
for k,row in new_rows.items():
    before=old_rows[k]
    for field in ["ir_frames","estimate_bytes","latency_frames","next_frame_rejection"]:
        assert row[field]==before[field],(k,field)
    assert row["retained_delta_within_estimate"] and row["construction_retained_requested_delta_bytes"]<=row["estimate_bytes"]
    assert row["construction_peak_requested_delta_bytes"]>=row["construction_retained_requested_delta_bytes"]
    phases=[row["warm"],*row["steady"]];old_phases=[before["warm"],*before["steady"]]
    case_misses=0
    assert len(phases)==3
    for phase,old_phase in zip(phases,old_phases):
        for field in ["callback_frames","callbacks","reported_quantum_frames","physical_budget_us","reported_budget_us"]:
            assert phase[field]==old_phase[field],(k,field)
        frames=phase["callback_frames"];rate=k[0]
        assert math.isclose(phase["physical_budget_us"],frames/rate*1e6,rel_tol=1e-12)
        assert math.isclose(phase["reported_budget_us"],max(frames,phase["reported_quantum_frames"])/rate*1e6,rel_tol=1e-12)
        assert phase["allocations"]==phase["frees"]==0 and phase["finite"]
        assert phase["reported_misses"]<=phase["physical_misses"]<=phase["callbacks"]
        assert 0<=phase["p50_us"]<=phase["p95_us"]<=phase["p99_us"]<=phase["max_us"]
        callbacks+=phase["callbacks"];case_misses+=phase["physical_misses"]
    failed+=int(any(p["physical_misses"] or p["reported_misses"] for p in phases));misses+=case_misses
    comparisons.append(dict(geometry=row["geometry"],ir_frames=row["ir_frames"],old_misses=sum(p["physical_misses"] for p in old_phases),new_misses=case_misses,old_retained_bytes=before["construction_retained_requested_delta_bytes"],new_retained_bytes=row["construction_retained_requested_delta_bytes"],retained_ratio=row["construction_retained_requested_delta_bytes"]/before["construction_retained_requested_delta_bytes"]))
summary=[r for r in load["rows"] if r["event"]=="summary"]
assert len(summary)==1
summary=summary[0]
assert summary["cases"]==63 and summary["errors"]==0 and summary["failed_cases"]==failed and summary["physical_misses"]==misses and summary["timing_margin"]==1.0
assert summary["pass"]==(failed==0)
assert load["exit_code"]==(1 if failed else 0)
nupc=[r for r in comparisons if r["geometry"]["nupc"]]
print(json.dumps(dict(functional_verified=True,deadline_pass=summary["pass"],shared_convolution_full_suite_passes=115,final_fft_reference_tests=3,linear_eq_passes=145,existing_ignored_shared_convolution=4,existing_ignored_linear_eq=2,stored_complex_values_before=87360,stored_complex_values_after=43812,load_geometries=63,load_callbacks=callbacks,failed_cases=failed,physical_misses=misses,historical_failed_cases=[r for r in previous["rows"] if r["event"]=="summary"][0]["failed_cases"],historical_physical_misses=[r for r in previous["rows"] if r["event"]=="summary"][0]["physical_misses"],nupc_geometries_fewer_misses=sum(r["new_misses"]<r["old_misses"] for r in nupc),nupc_geometries_equal_misses=sum(r["new_misses"]==r["old_misses"] for r in nupc),nupc_geometries_more_misses=sum(r["new_misses"]>r["old_misses"] for r in nupc),nupc_retained_ratio_min=min(r["retained_ratio"] for r in nupc),nupc_retained_ratio_max=max(r["retained_ratio"] for r in nupc),comparisons=comparisons,scope="Same admitted IR/partition/callback geometry and strict raw deadlines. Historical timing is an unmatched observation; no native-device, RSS or aggregate memory compliance claim. Retain all misses."),indent=2))
