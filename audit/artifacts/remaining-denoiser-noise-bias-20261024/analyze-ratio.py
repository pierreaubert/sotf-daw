#!/usr/bin/env python3
"""Apply the fixed common-change diagnostic go/no-go without retuning."""
import json
import math
from pathlib import Path
import re
HERE=Path(__file__).resolve().parent
run=json.loads((HERE/"ratio-validation.json").read_text())
raw=run["stdout"]
rows=[json.JSONDecoder().raw_decode(raw[m.end():])[0] for m in re.finditer("COMMON_CHANGE ",raw)]
assert len(rows)==1424,len(rows)
false_rows=[]
positive=[]
stationary_or_wanted=lambda r:r["condition"].startswith("stationary_") or r["condition"].startswith("wanted_") or r["condition"]=="recorded_wanted"
for r in rows:
    if stationary_or_wanted(r) and r["events"]:
        false_rows.append(dict(condition=r["condition"],source=r.get("source"),ordinal=r.get("ordinal"),low_latency=r["low_latency"],events=r["events"],fixed_manifest_regions=r.get("fixed_manifest_regions")))
for condition in ["gaussian_step_40db","uniform_step_40db","ar95_step_40db"]:
    for low in [False,True]:
        q=[r for r in rows if r["condition"]==condition and r["low_latency"]==low]
        assert len(q)==64
        accepted=0
        off_change=0
        for r in q:
            n=r["fft_size"]
            deadline=.25+2*n/48000
            accepted+=any(.25<=e["window_end_time"]<=deadline for e in r["events"])
            for e in r["events"]:
                # A four-hop reference is 2N old. A complete compared window
                # can remain affected until three N after either level edge.
                t=e["window_end_time"]
                if not any(edge<=t<=edge+3*n/48000 for edge in [.25,.75]):off_change+=1
        positive.append(dict(condition=condition,low_latency=low,seeds=64,timely_detections=accepted,required=math.ceil(.95*64),off_change_events=off_change,passed=accepted>=math.ceil(.95*64) and off_change==0))
summary=[]
for condition in sorted(set(r["condition"] for r in rows)):
    for low in [False,True]:
        q=[r for r in rows if r["condition"]==condition and r["low_latency"]==low]
        summary.append(dict(condition=condition,low_latency=low,reports=len(q),comparisons=sum(r["comparisons"] for r in q),events=sum(len(r["events"]) for r in q),reports_with_events=sum(bool(r["events"]) for r in q)))
exact_checks_pass=run["exit_code"]==0 and "raw_reference_lifecycle bins=21,83 fixed_lag=4 reset=true grouping_exact=true gain_independent=true" in raw
production_go=exact_checks_pass and not false_rows and all(r["passed"] for r in positive)
result=dict(decision="GO to separately predeclared production candidate only" if production_go else "NO-GO for production state rebase",production_rebase_go=production_go,production_changed=False,functional_exit=run["exit_code"],exact_reference_checks_pass=exact_checks_pass,false_control_reports=len(false_rows),false_control_events=sum(len(r["events"]) for r in false_rows),false_rows=false_rows,positive_guards=positive,summary=summary,heldout_acceptance=False,native_acceptance=False,limits="Exact Irwin-Hall tails apply only to the stated real-white-Gaussian reference. Actual uniform/colored/voiced/unvoiced fixtures are empirical and finite. The score is not an H1-shape goodness-of-fit or speech-identity proof.")
(HERE/"ratio-analysis.json").write_text(json.dumps(result,indent=2)+"\n")
print(json.dumps(dict(decision=result["decision"],false_control_reports=result["false_control_reports"],false_control_events=result["false_control_events"],positive_guards=positive,summary=summary),indent=2))
