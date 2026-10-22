"""Check the complete 108-row linear EQ workload after shared NUPC changes."""
import hashlib
import itertools
import json
import math
from pathlib import Path
ROOT=Path(__file__).resolve().parent
REPO=ROOT.parents[2]
old=json.loads((ROOT/"linear-phase-eq-load.json").read_text())
new=json.loads((ROOT/"nupc-coalesced-linear-eq-load.json").read_text())
source=json.loads((ROOT/"nupc-coalesced-source.json").read_text())
name="crates/sotf-plugins/crates/sotf-plugin-linear-phase-eq/bin/qa_dynamic_linear_phase_eq.rs"
assert source["linear_eq_harness_unchanged_from_historical"]
assert hashlib.sha256((REPO/name).read_bytes()).hexdigest()==old["source_sha256"][name]==source["source_sha256"][name]
rows=[r["row"] for r in new["records"] if "row" in r]
key=lambda r:(r["rate_hz"],r["channels"],r["stages"],r["phase"],r["callback_frames"])
expected=set(itertools.product([48000,96000,192000],[2,8,12],[1,2,10],["linear","minimum"],[128,512]))
assert len(rows)==108 and {key(r) for r in rows}==expected
before={key(r):r for r in old["rows"]};assert set(before)==expected
failed=misses=reported=callbacks=0
changes=[]
phases=["steady_before","priming_or_acceptance","blend","steady_after"]
for r in rows:
    a=before[key(r)]
    for field in ["taps_per_stage","physical_budget_us","reported_budget_us","reported_quantum_frames"]:assert r[field]==a[field]
    assert r["taps_per_stage"]==8192
    # The harness uses Duration, whose nanosecond rounding is retained exactly
    # in the old/new comparison above. Check the physical formula within 1 ns.
    assert math.isclose(r["physical_budget_us"],r["callback_frames"]/r["rate_hz"]*1e6,rel_tol=0,abs_tol=.001)
    assert math.isclose(r["reported_budget_us"],max(r["callback_frames"],r["reported_quantum_frames"])/r["rate_hz"]*1e6,rel_tol=0,abs_tol=.001)
    assert r["allocations"]==r["frees"]==0 and r["finite_output"] and r["replay_bound_ok"] and r["settled"]
    assert r["accepted_generation"]==a["accepted_generation"]==1
    row_misses=row_reported=0
    for phase in phases:
        p=r[phase]
        assert p["callbacks"]==a[phase]["callbacks"]
        assert 0<=p["reported_misses"]<=p["physical_misses"]<=p["callbacks"]
        assert 0<=p["p50_us"]<=p["p95_us"]<=p["p99_us"]<=p["max_us"]
        row_misses+=p["physical_misses"];row_reported+=p["reported_misses"];callbacks+=p["callbacks"]
    misses+=row_misses;reported+=row_reported;failed+=int(row_misses>0 or row_reported>0)
    changes.append(dict(rate_hz=r["rate_hz"],channels=r["channels"],stages=r["stages"],phase=r["phase"],callback_frames=r["callback_frames"],old_misses=sum(a[p]["physical_misses"] for p in phases),new_misses=row_misses))
summary=[r["summary"] for r in new["records"] if "summary" in r];assert len(summary)==1
summary=summary[0]
assert summary["rows"]==108 and summary["failed_rows"]==failed and summary["physical_misses"]==misses and summary["reported_misses"]==reported and summary["deadline_margin"]==1.0
assert new["exit_code"]==(1 if failed else 0)
print(json.dumps(dict(functional_verified=True,deadline_pass=failed==0,rows=108,callbacks=callbacks,failed_rows=failed,physical_misses=misses,reported_misses=reported,historical_summary=old["summary"],fewer_misses=sum(r["new_misses"]<r["old_misses"] for r in changes),equal_misses=sum(r["new_misses"]==r["old_misses"] for r in changes),more_misses=sum(r["new_misses"]>r["old_misses"] for r in changes),comparisons=changes,scope="Unchanged 108-row harness, configurations, callbacks and deadline margin. Historical measurements precede both shared NUPC changes and may include other intervening implementation changes; no causal speedup, native-device or FIR-M1 acceptance claim."),indent=2))
