#!/usr/bin/env python3
"""Compare every predeclared quality/noise guard without changing fixtures."""
import json
from pathlib import Path
import re
HERE=Path(__file__).resolve().parent
BASE=HERE.parent/"remaining-denoiser-zero-reduction-20261024"
def read(name):
    return json.loads(name.read_text())["stdout"]
def records(raw):
    out=[]
    for line in raw.splitlines():
        offset=line.find("{")
        if offset<0:continue
        try:row=json.JSONDecoder().raw_decode(line[offset:])[0]
        except ValueError:continue
        if isinstance(row,dict):out.append(row)
    return out
raw=read(HERE/"validation.json")
if (HERE/"quality-validation.json").exists():
    raw += "\n"+read(HERE/"quality-validation.json")
a=records(raw);b=records(read(BASE/"validation.json"))
pick=lambda rows,m:[r for r in rows if r.get("measurement")==m]
key=lambda r:(r["source"],r["condition"],r["low_latency"])
old={key(r):r for r in pick(b,"recorded_estimator")}
sdr=[]
for r in pick(a,"recorded_estimator"):
    prior=old[key(r)]
    assert [(w["label"],w["input"]) for w in r["windows"]]==[(w["label"],w["input"]) for w in prior["windows"]]
    full=lambda q:next(w["output"]["sdr_db"] for w in q["windows"] if w["label"]=="full")
    delta=full(r)-full(prior)
    sdr.append(dict(source=r["source"],condition=r["condition"],low_latency=r["low_latency"],baseline=full(prior),candidate=full(r),delta_db=delta,passed=delta>=-.25))
assert len(sdr)==36
vowels=[dict(low_latency=r["low_latency"],projection=r["vowel_300_450ms_clean_projection"],passed=r["vowel_300_450ms_clean_projection"]>=.5) for r in pick(a,"recorded_denoiser_onset")]
assert len(vowels)==2
old_noise={r["low_latency"]:r for r in pick(b,"independent_noise_channel") if r["start_frame"]==40800}
noise=[]
for r in pick(a,"independent_noise_channel"):
    if r["start_frame"]!=40800:continue
    delta=r["gain_db"]-old_noise[r["low_latency"]]["gain_db"]
    noise.append(dict(low_latency=r["low_latency"],attenuation_loss_db=delta,passed=delta<=1.))
assert len(noise)==2
render=lambda raw:[json.JSONDecoder().raw_decode(raw[m.end():])[0] for m in re.finditer("NOISE_RENDER ",raw)]
rkey=lambda r:(r["low_latency"],r["channel"],r["start_frame"],r["end_frame"])
old_render={rkey(r):r for r in render(read(HERE/"baseline-noise-step.json"))}
step=[]
for r in render(raw):
    delta=r["gain_db"]-old_render[rkey(r)]["gain_db"]
    step.append(dict(**r,baseline_gain_db=old_render[rkey(r)]["gain_db"],attenuation_loss_db=delta,passed=delta<=1.))
assert len(step)==12
all_guards=all(r["passed"] for group in [vowels,sdr,noise,step] for r in group)
result=dict(declared_quality_guards_passed=all_guards,vowel_guard=vowels,development_sdr=sdr,settled_noise_guard=noise,added_noise_step_guard=step,functional_exit=json.loads((HERE/"validation.json").read_text())["exit_code"],independent_checks=[x for x in raw.splitlines() if any(k in x for k in ["clean_power_reference cases=","clean_power_control_invariance configurations=","noise_fraction_risk_reference cases=","component_trace_summary records=","noise_presence_trace_summary records="])],native_deadline_acceptance=False,heldout_acceptance=False)
(HERE/"analysis.json").write_text(json.dumps(result,indent=2)+"\n")
print(json.dumps(dict(all_quality_guards=all_guards,vowels=vowels,sdr_range=[min(r["delta_db"] for r in sdr),max(r["delta_db"] for r in sdr)],sdr_failures=sum(not r["passed"] for r in sdr),noise=noise,noise_step_failures=[r for r in step if not r["passed"]]),indent=2))
