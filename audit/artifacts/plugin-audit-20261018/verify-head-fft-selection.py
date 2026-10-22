"""Validate FFT-friendly direct-head tail selection, waveform references and full strict load evidence."""
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
validation=read("head-fft-selection-validation.json")
assert len(validation)==4 and all(r["exit_code"]==0 for r in validation)
output=validation[0]["stdout"]
counts=[tuple(map(int,m)) for m in re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored",output)]
assert tuple(sum(c[i] for c in counts) for i in range(3))==(271,0,7),counts
pattern=r"head_fft_selection cases=(\d+) levels=(\d+) failures=(\d+)"
baseline=read("head-fft-selection-baseline.json");assert baseline["exit_code"]==101
red=re.search(pattern,baseline["stdout"]);assert red and tuple(map(int,red.groups()))==(962,4817,476)
focused=read("head-fft-selection-focused.json");assert focused["exit_code"]==0
green=re.search(pattern,focused["stdout"]);assert green and tuple(map(int,green.groups()))==(962,5260,0)
assert re.search(pattern,output).groups()==green.groups()
assert "head_fft_example head=257 blocks=[257, 514, 1028, 2056] first_delay=0" in baseline["stdout"]
assert "head_fft_example head=257 blocks=[256, 512, 1024, 2048] first_delay=1" in output
errors={}
for name,cases in [("nupc_coalesced_unmerged",45),("nupc_real_direct_reference",54),("nupc_large_sparse_reference",4),("nupc_phase_f64",36)]:
    m=re.search(name+r" cases=(\d+) maximum_error=([0-9.e+-]+)",output)
    assert m and int(m[1])==cases and float(m[2])<2e-5
    errors[name]=float(m[2])
assert "Physical callback misses: 0." in validation[2]["stdout"]
source=read("head-fft-selection-source.json")
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
def body(text,signature):
    a=text.index(signature);b=text.index("{",a);depth=1;c=b+1
    while depth:
        if text[c]=="{":depth+=1
        elif text[c]=="}":depth-=1
        c+=1
    return text[a:c]
for signature in ["fn process_stream(", "pub(super) fn build_ir_state(", "pub(super) fn apply_ir_state(", "fn reset(&mut self)"]:
    assert body(before,signature)==body(after,signature),signature
matched_source=read("head-fft-selection-matched-source.json")
assert matched_source["hashes_unchanged_after_load"]
for name,sha in matched_source["source_sha256"].items():
    assert hashlib.sha256((REPO/matched_source["snapshot_paths"].get(name,name)).read_bytes()).hexdigest()==sha,name
engine="crates/sotf-plugins/crates/plugins-spatial/src/nupc/nupc_engine.rs"
engine_before=subprocess.run(["rtk","proxy","git","show",source["base_commit"]+":"+engine],capture_output=True,text=True,check=True).stdout
engine_after=(REPO/engine).read_text()
for signature in ["pub fn new(ir: &[f32], min_block: usize) -> Self", "pub fn instantiate(&self)", "pub fn instantiate_with_phase(", "pub fn process_sample(", "pub fn reset(&mut self)"]:
    assert body(engine_before,signature)==body(engine_after,signature),signature
coalesced="crates/sotf-plugins/crates/plugins-spatial/src/nupc/coalesced_tests.rs"
coalesced_before=subprocess.run(["rtk","proxy","git","show",source["base_commit"]+":"+coalesced],capture_output=True,text=True,check=True).stdout
coalesced_after=(REPO/coalesced).read_text()
for signature in ["fn unmerged(","fn merged_levels_match_separate_delayed_groups_through_reset_and_complete_tail("]:
    assert body(coalesced_before,signature)==body(coalesced_after,signature),signature
harness="crates/sotf-plugins/crates/sotf-plugin-convolution/bin/qa_convolution_load.rs"
harness_before=subprocess.run(["rtk","proxy","git","show",source["base_commit"]+":"+harness],capture_output=True,text=True,check=True).stdout
harness_after=(REPO/harness).read_text()
for signature in ["fn measure(","fn preflight(","fn fixture(","fn main()"]:
    assert body(harness_before,signature)==body(harness_after,signature),signature
assert (REPO/matched_source["snapshot_paths"][harness]).read_text()==harness_before
assert body((REPO/matched_source["snapshot_paths"][engine]).read_text(),"pub fn new_with_head(")==body(engine_after,"pub fn new_with_head(")
old=read("nupc-phase-load.json");new=read("head-fft-selection-load.json");matched=read("head-fft-selection-matched-load.json")
for data,name in [(new,"head-fft-selection-load.jsonl"),(matched,"head-fft-selection-matched-load.jsonl")]:
    assert [json.loads(line) for line in (ROOT/name).read_text().splitlines() if line.strip()]==data["rows"]
key=lambda r:tuple(r["geometry"][k] for k in ["rate_hz","output_channels","ir_channels","true_stereo","nupc","head"])
expected={(rate,out,ir,matrix,nupc,head) for rate,(out,ir,matrix),(nupc,head) in itertools.product([48000,96000,192000],[(2,1,False),(2,2,False),(12,1,False),(12,12,False),(32,1,False),(32,32,False),(2,4,True)],[(False,False),(True,False),(True,True)])}
def largest(length,base):
    if length==0:return 1
    period=base;coverage=4*base
    while coverage<length:
        period*=2;coverage+=4*period
    return period
def load(data,expanded=False):
    rows=[r for r in data["rows"] if r["event"]=="case_result"]
    assert len(rows)==63 and {key(r) for r in rows}==expected
    assert not any(r["event"]=="case_error" for r in data["rows"])
    failed=misses=callbacks=0
    for r in rows:
        assert r["retained_delta_within_estimate"] and r["construction_retained_requested_delta_bytes"]<=r["estimate_bytes"]
        length=r["ir_frames"];g=r["geometry"]
        head=min(257,length) if g["head"] else 0
        base=min(1024,head) if head else 1024
        legacy=largest(length-head,base) if g["nupc"] else 1024
        actual_base=1<<(base.bit_length()-1) if head else base
        actual=largest(length-head,actual_base) if g["nupc"] else 1024
        period=max(legacy,actual) if expanded else legacy
        if expanded:
            assert r["legacy_largest_period_frames"]==legacy
            assert r["actual_largest_period_frames"]==actual
            assert r["measured_largest_period_frames"]==period
        assert r["warm"]["callback_frames"]==128 and r["warm"]["callbacks"]==(length+r["latency_frames"]+37+127)//128
        assert [p["callback_frames"] for p in r["steady"]]==[128,512]
        for phase in r["steady"]:
            assert phase["callbacks"]==max(256,(4*period+phase["callback_frames"]-1)//phase["callback_frames"])
            if expanded:assert phase["callbacks"]*phase["callback_frames"]>=4*actual
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
a,old_summary=load(old);b,new_summary=load(new,expanded=True);c,matched_summary=load(matched)
def compare(previous_rows,current_rows,growth):
    result=[]
    for k,r in current_rows.items():
        previous=previous_rows[k]
        for field in ["ir_frames","estimate_bytes","latency_frames","next_frame_rejection"]:assert r[field]==previous[field],(k,field)
        phases=[]
        for i,(left,right) in enumerate(zip([r["warm"],*r["steady"]],[previous["warm"],*previous["steady"]])):
            for field in ["callback_frames","reported_quantum_frames","physical_budget_us","reported_budget_us"]:assert left[field]==right[field],(k,field)
            if growth and i>0:assert left["callbacks"]>=right["callbacks"]
            else:assert left["callbacks"]==right["callbacks"]
            phases.append(dict(callback_frames=left["callback_frames"],old_callbacks=right["callbacks"],new_callbacks=left["callbacks"],old_misses=right["physical_misses"],new_misses=left["physical_misses"]))
        result.append(dict(geometry=r["geometry"],old_misses=sum(p["old_misses"] for p in phases),new_misses=sum(p["new_misses"] for p in phases),old_retained_bytes=previous["construction_retained_requested_delta_bytes"],new_retained_bytes=r["construction_retained_requested_delta_bytes"],phases=phases))
    return result
horizon=read("head-fft-selection-horizon.json")
assert len(horizon["rows"])==63 and horizon["extended_geometries"]==12 and horizon["additional_callbacks"]==30600
assert horizon["matched_callbacks"]==matched_summary["callbacks"]==532086
assert horizon["expected_expanded_callbacks"]==new_summary["callbacks"]==562686
for report in horizon["rows"]:
    k=key(report);r=b[k];previous=c[k]
    for name,field in [("legacy_period_frames","legacy_largest_period_frames"),("actual_period_frames","actual_largest_period_frames"),("measured_period_frames","measured_largest_period_frames")]:assert report[name]==r[field]
    for p,current,old_phase in zip(report["phases"],r["steady"],previous["steady"]):
        assert p["callback_frames"]==current["callback_frames"]==old_phase["callback_frames"]
        assert p["matched_callbacks"]==old_phase["callbacks"] and p["expanded_callbacks"]==current["callbacks"]
        assert math.isclose(p["matched_actual_periods"],old_phase["callbacks"]*old_phase["callback_frames"]/r["actual_largest_period_frames"],rel_tol=1e-15)
comparisons=compare(a,b,True);matched_comparisons=compare(a,c,False)
upc=[r for r in comparisons if not r["geometry"]["nupc"]]
nupc=[r for r in comparisons if r["geometry"]["nupc"]]
assert len(upc)==21 and len(nupc)==42
heads=[r for r in nupc if r["geometry"]["head"]]
matched_heads=[r for r in matched_comparisons if r["geometry"]["head"]]
assert len(heads)==len(matched_heads)==21
assert sum(r["new_misses"] for r in matched_heads)<sum(r["old_misses"] for r in matched_heads)
ratios=[r["new_retained_bytes"]/r["old_retained_bytes"] for r in matched_heads]
expanded_rows=sum(any(p["new_callbacks"]>p["old_callbacks"] for p in r["phases"]) for r in comparisons)
print(json.dumps(dict(functional_verified=True,normal_tests_passed=271,normal_tests_ignored=7,head_selection_cases=962,old_levels=4817,new_levels=5260,maximum_errors=errors,admission_arithmetic_unchanged=True,load_measurement_budgets_and_matrix_unchanged=True,actual_partition_horizon_verified=True,extended_horizon_geometries=expanded_rows,additional_callbacks=30600,load=new_summary,matched_historical_horizon_load=matched_summary,previous_load=old_summary,matched_head_old_misses=sum(r["old_misses"] for r in matched_heads),matched_head_new_misses=sum(r["new_misses"] for r in matched_heads),expanded_head_misses=sum(r["new_misses"] for r in heads),matched_head_geometries_fewer_misses=sum(r["new_misses"]<r["old_misses"] for r in matched_heads),matched_head_geometries_equal_misses=sum(r["new_misses"]==r["old_misses"] for r in matched_heads),matched_head_geometries_more_misses=sum(r["new_misses"]>r["old_misses"] for r in matched_heads),matched_head_retained_ratio_min=min(ratios),matched_head_retained_ratio_max=max(ratios),comparisons=comparisons,matched_comparisons=matched_comparisons,scope="All admitted geometries and strict budgets retained. Historical matched timings are unmatched local observations; the final horizon is extended to at least four actual prepared periods and never shorter than historical duration. No causal/native-device or new linear-EQ capacity acceptance."),indent=2))
