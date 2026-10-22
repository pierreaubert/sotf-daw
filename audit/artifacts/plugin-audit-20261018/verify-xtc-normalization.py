"""Verify normalizer invariants and independently recompute every staged metric."""
import hashlib
import itertools
import json
import math
import re
from pathlib import Path
ROOT = Path(__file__).resolve().parent
baseline = json.loads((ROOT / "xtc-normalization-fixed-point-baseline.json").read_text())
assert baseline["exit_code"] == 101
assert "centered_coordinated_reference coefficient_words=4100 bit_differences=0" in baseline["stdout"]
assert "coefficients=28 failures=11" in baseline["stdout"]
assert "columns=20 failures=5" in baseline["stdout"]
reachable = json.loads((ROOT / "xtc-normalization-reachable-baseline.json").read_text())
assert reachable["exit_code"] == 101 and "columns=1944 failures=672" in reachable["stdout"]
validation = json.loads((ROOT / "xtc-normalization-validation.json").read_text())
assert len(validation) == 3 and all(r["exit_code"] == 0 for r in validation)
assert [r["command"][3] for r in validation] == ["test", "clippy", "run"]
output = validation[0]["stdout"]
counts = [tuple(map(int, m)) for m in re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored", output)]
assert sum(c[0] for c in counts) == 256 and sum(c[1] for c in counts) == 0 and sum(c[2] for c in counts) == 1, counts
for phrase in ["coefficients=28 failures=0", "columns=20 failures=0", "reachable_normalization columns=1944 failures=0", "preparation_stages conditions=16 frequencies=96 records=576 maximum_reconstruction_error=0", "rendered_pose normalization=false cases=120 maximum_mirror_error=0 maximum_partition_error=0", "rendered_pose normalization=true cases=120 maximum_mirror_error=0 maximum_partition_error=0", "rendered_pose_total cases=240", "lateral_pose_heap cases=60 allocations=0 frees=0 latency_samples=2048 reset_partition_error=0"]:
    assert phrase in output, phrase
assert "[ALL PASS] Xtc QA Complete." in validation[2]["stdout"]
nominal = json.loads((ROOT / "xtc-normalization-nominal-report.json").read_text())
assert nominal["exit_code"] == 0 and "Summary: 16/17 tests passed" in nominal["stdout"]
assert "measured 4.811, tolerance 0.000" in nominal["stdout"]
def objects(text):
    decoder = json.JSONDecoder()
    for line in text.splitlines():
        start = line.find("{")
        if start < 0: continue
        try: row, _ = decoder.raw_decode(line[start:])
        except json.JSONDecodeError: continue
        if isinstance(row,dict): yield row
stages = ["regularized_unbounded", "refined_unbounded", "inverse_limited", "normalized_capped", "identity_blended", "effort_limited"]
rows = [r for r in objects(output) if r.get("measurement") == "xtc_preparation_stage"]
expected = set()
for rate,fft,head,mode in itertools.product([48000,96000],[2048,4096],[0,1],["phase_only","explicit_delay"]):
    for bin in [0,100*fft//rate,200*fft//rate,1000*fft//rate,fft//4,fft//2]:
        for stage in stages: expected.add((rate,fft,head,mode,bin,stage))
key = lambda r: (r["rate"],r["fft_size"],r["head_model"],r["itd_modeling"],r["bin"],r["stage"])
assert len(rows) == 576 and len({key(r) for r in rows}) == 576 and {key(r) for r in rows} == expected
matrix = lambda values: [[complex(*value) for value in row] for row in values]
log = lambda value: math.log10(value) if value else -math.inf
maximum_metric_error = 0.0
maximum_power_error = 0.0
maximum_stored_error = 0.0
transfers = {}
by_key = {}
for row in rows:
    h,w = matrix(row["plant"]),matrix(row["filters"])
    t = [[h[ear][0]*w[0][cue]+h[ear][1]*w[1][cue] for cue in range(2)] for ear in range(2)]
    raw = [20*(log(abs(h[1-cue][cue]))-log(abs(t[1-cue][cue]))) for cue in range(2)]
    desired = [20*(log(abs(t[cue][cue]))-log(abs(h[cue][cue]))) for cue in range(2)]
    separation = [a+b for a,b in zip(raw,desired)]
    for field, values in [("raw_depth_db",raw),("desired_gain_db",desired),("separation_improvement_db",separation)]:
        for observed,value in zip(row[field],values):
            if math.isfinite(value):
                assert isinstance(observed,(int,float))
                error = abs(observed-value)
                maximum_metric_error = max(maximum_metric_error,error)
                assert error < 1e-6, (key(row),field,observed,value)
            else:
                assert observed == ("nan" if math.isnan(value) else "+inf" if value>0 else "-inf")
    for index,power in enumerate(row["row_powers"]):
        expected_power = sum(c.real*c.real+c.imag*c.imag for c in w[index])
        error = abs(power-expected_power)
        maximum_power_error = max(maximum_power_error,error)
        assert math.isclose(power,expected_power,rel_tol=1e-12,abs_tol=1e-12)
        if row["stage"] == "effort_limited": assert power <= row["ceiling"]**2 * 1.000002
    assert row["frequency_hz"] == row["bin"]*row["rate"]/row["fft_size"] and 0<=row["alpha"]<=1
    if row["stage"] == "effort_limited":
        stored = matrix(row["stored_filters"])
        maximum_stored_error = max(maximum_stored_error,max(abs(a-b) for arow,brow in zip(w,stored) for a,b in zip(arow,brow)))
    else: assert row["stored_filters"] is None
    transfers[key(row)] = t
    by_key[key(row)] = row
assert maximum_stored_error == 0
normalization_maximum_worsening = 0.0
for rate,fft,head,mode,bin,stage in expected:
    if stage != "normalized_capped": continue
    before = transfers[(rate,fft,head,mode,bin,"inverse_limited")]
    after = transfers[(rate,fft,head,mode,bin,stage)]
    for cue in range(2):
        worsening = abs(abs(after[cue][cue])-1)-abs(abs(before[cue][cue])-1)
        normalization_maximum_worsening = max(normalization_maximum_worsening,worsening)
        assert worsening < 2e-6
selected = [by_key[(48000,2048,0,"phase_only",4,stage)] for stage in stages]
assert selected[-1]["filters"] == selected[-2]["filters"]
def check_rendered(text):
    rows = [r for r in objects(text) if r.get("measurement")=="rendered_xtc_mismatch"]
    measured = [r for r in objects(text) if r.get("measurement")=="rendered_xtc_filter"]
    frequencies = [48000*b/2048 for b in [1,2,4,8,10,22,43,85,171,341]]
    expected_filters = set(itertools.product([False,True],frequencies))
    assert len(measured)==20 and {(r["spectral_normalization"],r["frequency_hz"]) for r in measured}==expected_filters
    filters = {(r["spectral_normalization"],r["frequency_hz"]):matrix(r["filters"]) for r in measured}
    for r in measured:
        assert (r["sample_rate"],r["fft_size"],r["total_frames"],r["measure_start_frame"],r["input_amplitude"],r["auto_gain_enabled"])==(48000,2048,24576,16384,0.001,False)
        assert r["filters"]==r["irregular_filters"]
    key = lambda r:(r["spectral_normalization"],r["frequency_hz"],r["head_x_m"],r["yaw_rad"],r["echo_gain"],r["cue"])
    expected=set(itertools.product([False,True],frequencies,[-0.1,-0.02,0.,0.02,0.1],[-0.3,0.,0.3],[0.,0.4],[0,1]))
    assert len(rows)==1200 and len({key(r) for r in rows})==1200 and {key(r) for r in rows}==expected
    maximum_error = 0.0
    for r in rows:
        frequency,x,yaw,echo=r["frequency_hz"],r["head_x_m"],r["yaw_rad"],r["echo_gain"]
        # Independently reconstruct the specified controlled point-source/image
        # field, not the plugin's head-shadow model or a measured human HRTF.
        h=[]
        for side in [-0.0875,0.0875]:
            ex=x+side*math.cos(yaw)
            ey=side*math.sin(yaw)
            row=[]
            for sx in [-1.,1.]:
                sy=math.sqrt(3.)
                direct=math.sqrt((sx-ex)**2+(sy-ey)**2)
                reflected=math.sqrt((sx-ex)**2+(sy+2.+ey)**2)
                a=-math.tau*frequency*direct/343.
                b=-math.tau*frequency*reflected/343.
                row.append(complex(2./direct*math.cos(a)+echo*2./reflected*math.cos(b),2./direct*math.sin(a)+echo*2./reflected*math.sin(b)))
            h.append(row)
        w=filters[(r["spectral_normalization"],frequency)]
        cue=r["cue"]
        t=[h[ear][0]*w[0][cue]+h[ear][1]*w[1][cue] for ear in range(2)]
        depth=20.*(log(abs(h[1-cue][cue]))-log(abs(t[1-cue])))
        gain=20.*(log(abs(t[cue]))-log(abs(h[cue][cue])))
        for field,value in [("raw_depth_db",depth),("desired_gain_db",gain),("separation_improvement_db",depth+gain)]:
            assert math.isfinite(r[field]) and math.isfinite(value)
            error=abs(r[field]-value)
            maximum_error=max(maximum_error,error)
            assert error<1e-6,(key(r),field,r[field],value)
    return {key(r):r for r in rows},maximum_error
mismatch,render_metric_error=check_rendered(output)
rendered_baseline=json.loads((ROOT/"xtc-normalization-rendered-baseline.json").read_text())
assert rendered_baseline["exit_code"]==0 and rendered_baseline["base_commit"]=="49aec9e" and rendered_baseline["worktree_removed"]
assert "3 passed; 0 failed" in rendered_baseline["stdout"]
assert hashlib.sha256((ROOT.parents[2]/"crates/sotf-plugins/crates/sotf-plugin-xtc/tests/audit_cancellation.rs").read_bytes()).hexdigest()==rendered_baseline["diagnostic_sha256"]
previous,baseline_render_metric_error=check_rendered(rendered_baseline["stdout"])
repair=json.loads((ROOT/"xtc-normalization-baseline-lock-resolution.json").read_text())
assert repair["package_keys_unchanged"] and len(repair["changed_packages"])==1
assert repair["changed_packages"][0]["package"][0]=="sotf-capture"
lock_change=repair["changed_packages"][0]
a,b=lock_change["before"],lock_change["after"]
assert {k:v for k,v in a.items() if k!="dependencies"}=={k:v for k,v in b.items() if k!="dependencies"}
assert set(b["dependencies"])-set(a["dependencies"])=={"rustix"} and set(a["dependencies"])<=set(b["dependencies"])
changes={mode:[r["separation_improvement_db"]-previous[k]["separation_improvement_db"] for k,r in mismatch.items() if k[0]==mode] for mode in [False,True]}
assert all(d==0 for d in changes[False]),"normalization-off baseline changed"
old_text=json.loads((ROOT/"xtc-coordinated-limits-validation.json").read_text())[0]["stdout"]
old_rows=[r for r in objects(old_text) if r.get("measurement")=="rendered_xtc_mismatch"]
assert len(old_rows)==360
for r in old_rows:
    k=(False,r["frequency_hz"],r["head_x_m"],r["yaw_rad"],r["echo_gain"],r["cue"])
    assert all(mismatch[k][f]==r[f] for f in ["raw_depth_db","desired_gain_db","separation_improvement_db"])
cohorts={str(mode):dict(cases=600,degraded=sum(r["separation_improvement_db"]<0 for k,r in mismatch.items() if k[0]==mode),worst_normalized_db=min(r["separation_improvement_db"] for k,r in mismatch.items() if k[0]==mode),minimum_change_from_baseline_db=min(changes[mode]),maximum_change_from_baseline_db=max(changes[mode]),nominal_rendered_93_75hz=mismatch[(mode,93.75,0.,0.,0.,0)],baseline_nominal_rendered_93_75hz=previous[(mode,93.75,0.,0.,0.,0)]) for mode in [False,True]}
change=json.loads((ROOT/"xtc-normalization-default-change.json").read_text())
folder=ROOT.parents[2]/"crates/sotf-plugins/crates/sotf-plugin-xtc/src/filters"
for name,field in [("centered_coordinated_limits_bits.txt","previous_fixture_sha256"),("centered_normalization_bits.txt","current_fixture_sha256")]:
    assert hashlib.sha256((folder/name).read_bytes()).hexdigest()==change[field]
assert change["frequency_blend_blocks_unchanged"]==3 and change["beta_function_unchanged"]
print(json.dumps(dict(passed_tests=256,existing_ignored=1,fixed_point_coefficients=28,controlled_gain_columns=20,reachable_prepared_columns=1944,stage_conditions=16,stage_frequency_cases=96,stage_records=576,maximum_independent_metric_error_db=maximum_metric_error,maximum_independent_power_error=maximum_power_error,maximum_stored_coefficient_error=maximum_stored_error,maximum_staged_normalization_worsening=normalization_maximum_worsening,nominal_selected_bin_stages=[dict(stage=r["stage"],frequency_hz=r["frequency_hz"],alpha=r["alpha"],raw_depth_db=r["raw_depth_db"],desired_gain_db=r["desired_gain_db"],separation_improvement_db=r["separation_improvement_db"],row_powers=r["row_powers"]) for r in selected],rendered_mismatch_cases=1200,rendered_normalization_cohorts=cohorts,rendered_pose_cases=240,historical_off_rows_preserved=360,maximum_rendered_metric_error_db=render_metric_error,maximum_baseline_rendered_metric_error_db=baseline_render_metric_error,default_coefficient_change=change,nominal_report="16/17; selected93.75Hz cancellation4.811dB remains below5dB",scope="Stage replay is a shared-model causal comparison. Counterfactual unbounded refinement can choose a different candidate. Measured/native robustness, device/update deadlines, reconstructed true peaks and listening remain unproven."),indent=2))
