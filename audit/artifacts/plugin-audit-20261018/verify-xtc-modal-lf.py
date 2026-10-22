"""Independently verify modal LF decisions, gain budgets and declared A/B guards."""
import contextlib
import hashlib
import io
import itertools
import json
import math
import re
import runpy
import struct
from pathlib import Path
ROOT=Path(__file__).resolve().parent
# Revalidate the immutable prior experiment and reuse its independent point-field
# reconstruction, not any production Rust design helper.
with contextlib.redirect_stdout(io.StringIO()):
    baseline=runpy.run_path(str(ROOT/"verify-xtc-normalization.py"))
objects=baseline["objects"]
matrix=baseline["matrix"]
check_rendered=baseline["check_rendered"]
f32=lambda v:struct.unpack("<f",struct.pack("<f",v))[0]
legacy=json.loads((ROOT/"xtc-modal-lf-baseline.json").read_text())
assert len(legacy)==2 and all(r["exit_code"]==101 for r in legacy)
assert "centered_normalization_reference coefficient_words=4100 bit_differences=0" in legacy[0]["stdout"]
assert "healthy_lf_blend cases=7 failures=4" in legacy[0]["stdout"]
assert "bounded_lf_blend row_checks=24 failures=12" in legacy[0]["stdout"]
assert "Summary: 16/17 tests passed" in legacy[1]["stdout"]
validation=json.loads((ROOT/"xtc-modal-lf-validation.json").read_text())
assert len(validation)==3 and all(r["exit_code"]==0 for r in validation)
output=validation[0]["stdout"]
counts=[tuple(map(int,m)) for m in re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored",output)]
assert sum(c[0] for c in counts)==261 and sum(c[1] for c in counts)==0 and sum(c[2] for c in counts)==1,counts
for phrase in ["healthy_lf_blend cases=7 failures=0", "bounded_lf_blend row_checks=24 failures=0", "mode_gain_oracle cases=24", "collapsed_ear_mode_retains_legacy_low_frequency_rescue", "preparation_stages conditions=16 frequencies=96 records=672 maximum_reconstruction_error=0", "rendered_pose_total cases=240", "lateral_pose_heap cases=60 allocations=0 frees=0 latency_samples=2048 reset_partition_error=0"]:
    assert phrase in output,phrase
assert "[ALL PASS] Xtc QA Complete." in validation[2]["stdout"]
rows=[r for r in objects(output) if r.get("measurement")=="xtc_preparation_stage"]
stages=["regularized_unbounded","refined_unbounded","inverse_limited","normalized_capped","prepared_effort_limited","identity_blended","effort_limited"]
expected=set()
for rate,fft,head,mode in itertools.product([48000,96000],[2048,4096],[0,1],["phase_only","explicit_delay"]):
    for bin in [0,100*fft//rate,200*fft//rate,1000*fft//rate,fft//4,fft//2]:
        for stage in stages:expected.add((rate,fft,head,mode,bin,stage))
key=lambda r:(r["rate"],r["fft_size"],r["head_model"],r["itd_modeling"],r["bin"],r["stage"])
assert len(rows)==672 and len({key(r) for r in rows})==672 and {key(r) for r in rows}==expected
by_key={key(r):r for r in rows}
log=lambda v:math.log10(v) if v else -math.inf
metric_error=power_error=stored_error=mode_error=weight_error=prepared_error=blend_error=0.0
for r in rows:
    h,w=matrix(r["plant"]),matrix(r["filters"])
    t=[[h[e][0]*w[0][i]+h[e][1]*w[1][i] for i in range(2)] for e in range(2)]
    raw=[20*(log(abs(h[1-i][i]))-log(abs(t[1-i][i]))) for i in range(2)]
    gain=[20*(log(abs(t[i][i]))-log(abs(h[i][i]))) for i in range(2)]
    for field,values in [("raw_depth_db",raw),("desired_gain_db",gain),("separation_improvement_db",[a+b for a,b in zip(raw,gain)])]:
        for actual,value in zip(r[field],values):
            if math.isfinite(value):
                error=abs(actual-value);metric_error=max(metric_error,error);assert error<1e-6
            else:assert actual==("nan" if math.isnan(value) else "+inf" if value>0 else "-inf")
    for index,power in enumerate(r["row_powers"]):
        expected_power=sum(v.real*v.real+v.imag*v.imag for v in w[index])
        power_error=max(power_error,abs(power-expected_power));assert math.isclose(power,expected_power,rel_tol=1e-12,abs_tol=1e-12)
        if r["stage"] in ["prepared_effort_limited","identity_blended","effort_limited"]:
            assert power<=r["ceiling"]**2*1.000002
    assert r["frequency_hz"]==r["bin"]*r["rate"]/r["fft_size"]
    if r["stage"]=="effort_limited":
        stored=matrix(r["stored_filters"])
        stored_error=max(stored_error,max(abs(a-b) for ar,br in zip(w,stored) for a,b in zip(ar,br)))
    else:assert r["stored_filters"] is None
assert stored_error==0
for k in expected:
    if k[-1]!="prepared_effort_limited":continue
    r=by_key[k];h=matrix(r["plant"]);p=matrix(r["filters"])
    before=by_key[k[:-1]+("normalized_capped",)]
    w=matrix(before["filters"])
    largest=max(before["row_powers"])
    scale=min(1.,r["ceiling"]/math.sqrt(largest)) if largest else 1.
    for i in range(2):
        for j in range(2):
            predicted=complex(f32(w[i][j].real*scale),f32(w[i][j].imag*scale))
            prepared_error=max(prepared_error,abs(predicted-p[i][j]));assert abs(predicted-p[i][j])<1e-6
    t=[[h[e][0]*p[0][i]+h[e][1]*p[1][i] for i in range(2)] for e in range(2)]
    # Independent Hermitian Gram eigenvalues, rather than production's
    # determinant/product calculation of the small singular value.
    a=sum(abs(t[e][0])**2 for e in range(2))
    d=sum(abs(t[e][1])**2 for e in range(2))
    cross=sum(t[e][0].conjugate()*t[e][1] for e in range(2))
    minimum_squared=max(0.,0.5*(a+d-math.hypot(a-d,2*abs(cross))))
    expected_mode=min(1.,math.sqrt(minimum_squared))
    mode_error=max(mode_error,abs(r["mode_gain"]-expected_mode));assert abs(r["mode_gain"]-expected_mode)<2e-7
    low=1/(1+math.exp((100-r["frequency_hz"])/30))
    high=1/(1+math.exp((r["frequency_hz"]-12000)/1500))
    predicted_alpha=(low+(1-low)*r["mode_gain"])*high
    weight_error=max(weight_error,abs(r["alpha"]-predicted_alpha));assert abs(r["alpha"]-predicted_alpha)<2e-7
    b=matrix(by_key[k[:-1]+("identity_blended",)]["filters"])
    for i in range(2):
        for j in range(2):
            re_part=f32(p[i][j].real*r["alpha"]);im_part=f32(p[i][j].imag*r["alpha"])
            if i==j:re_part=f32(re_part+f32(1-r["alpha"]));im_part=f32(im_part+0.)
            error=abs(complex(re_part,im_part)-b[i][j]);blend_error=max(blend_error,error);assert error<1e-6
current,render_error=check_rendered(output)
previous=baseline["mismatch"]
assert set(current)==set(previous)
cohorts={}
guard_failures=[]
for mode in [False,True]:
    new=[r for k,r in current.items() if k[0]==mode];old=[r for k,r in previous.items() if k[0]==mode]
    degraded=sum(r["separation_improvement_db"]<0 for r in new)
    old_degraded=sum(r["separation_improvement_db"]<0 for r in old)
    worst=min(r["separation_improvement_db"] for r in new);old_worst=min(r["separation_improvement_db"] for r in old)
    if degraded>old_degraded:
        guard_failures.append(dict(mode=mode,guard="degraded_count",actual=degraded,maximum=old_degraded))
    if worst<old_worst-1e-6:
        guard_failures.append(dict(mode=mode,guard="worst_separation",actual=worst,minimum=old_worst-1e-6))
    differences=[r["separation_improvement_db"]-previous[k]["separation_improvement_db"] for k,r in current.items() if k[0]==mode]
    newly_degraded=[dict(key=list(k),baseline_separation_db=previous[k]["separation_improvement_db"],candidate_separation_db=r["separation_improvement_db"]) for k,r in current.items() if k[0]==mode and r["separation_improvement_db"]<0<=previous[k]["separation_improvement_db"]]
    cohorts[str(mode)]=dict(newly_degraded=newly_degraded,cases=600,degraded=degraded,baseline_degraded=old_degraded,worst_normalized_db=worst,baseline_worst_normalized_db=old_worst,minimum_change_db=min(differences),maximum_change_db=max(differences),nominal_rendered_93_75hz=current[(mode,93.75,0.,0.,0.,0)])
change=json.loads((ROOT/"xtc-modal-lf-default-change.json").read_text())
assert all(change["unchanged_functions"].values())
folder=ROOT.parents[2]/"crates/sotf-plugins/crates/sotf-plugin-xtc"
assert hashlib.sha256((folder/"tests/audit_cancellation.rs").read_bytes()).hexdigest()==change["rendered_diagnostic_sha256"]
for name,field in [("centered_normalization_bits.txt","previous_fixture_sha256"),("xtc-modal-lf-rejected-fixture.txt","current_fixture_sha256")]:
    assert hashlib.sha256(((folder/"src/filters"/name) if field=="previous_fixture_sha256" else ROOT/"xtc-modal-lf-rejected-fixture.txt").read_bytes()).hexdigest()==change[field]
selected=[by_key[(48000,2048,0,"phase_only",4,s)] for s in stages]
assert selected[-1]["raw_depth_db"][0]>=5
print(json.dumps(dict(accepted=not guard_failures,candidate_status="rejected" if guard_failures else "accepted",guard_failures=guard_failures,passed_tests=261,existing_ignored=1,modal_oracle_cases=24,stage_records=672,stage_frequency_cases=96,maximum_metric_error_db=metric_error,maximum_power_error=power_error,maximum_stored_error=stored_error,maximum_prepared_budget_error=prepared_error,maximum_independent_mode_error=mode_error,maximum_independent_weight_error=weight_error,maximum_blend_error=blend_error,nominal_selected_bin=[dict(stage=r["stage"],frequency_hz=r["frequency_hz"],mode_gain=r["mode_gain"],alpha=r["alpha"],raw_depth_db=r["raw_depth_db"],desired_gain_db=r["desired_gain_db"],row_powers=r["row_powers"]) for r in selected],rendered_cohorts=cohorts,maximum_independent_render_metric_error_db=render_error,default_coefficient_change=change,scope="Retained nominal analytical targets and declared controlled-mismatch guards; measured HRTF/room/native/load/listening and true-peak acceptance remain unproven."),indent=2))

if guard_failures:
    raise SystemExit(1)
