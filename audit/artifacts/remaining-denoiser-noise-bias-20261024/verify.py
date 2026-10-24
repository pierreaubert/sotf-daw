#!/usr/bin/env python3
"""Verify the frozen no-go, independent references and exact restoration."""
from fractions import Fraction
import hashlib
import json
from math import comb, factorial, ceil, floor
from pathlib import Path
import re
import subprocess
import sys

HERE=Path(__file__).resolve().parent
ROOT=HERE.parents[2]
FROZEN="--frozen" in sys.argv
load=lambda name:json.loads((HERE/name).read_text())
base="21806c0"
crate="crates/sotf-plugins/crates/sotf-plugin-denoiser"
def git(args):return subprocess.check_output(["rtk","proxy","git"]+args,cwd=ROOT)
def digest(data):return hashlib.sha256(data).hexdigest()
def extract(text,marker):return [json.JSONDecoder().raw_decode(text[m.end():])[0] for m in re.finditer(marker,text)]

restoration=load("restoration.json")
assert restoration["base_commit"]==base
assert restoration["active_entire_crate_matches"] and restoration["index_entire_crate_matches"]
assert restoration["tracked_file_count"]==46 and not restoration["untracked_crate_files"]
for path,sha in restoration["active_file_hashes"].items():
    assert digest(git(["show",base+":"+path]))==sha,path
if not FROZEN:
    subprocess.run(["rtk","proxy","git","diff",base,"--exit-code","--",crate],cwd=ROOT,check=True,stdout=subprocess.PIPE)
    subprocess.run(["rtk","proxy","git","diff","--cached",base,"--exit-code","--",crate],cwd=ROOT,check=True,stdout=subprocess.PIPE)
    assert not git(["ls-files","--others","--exclude-standard","-z","--",crate])
    for path,sha in restoration["active_file_hashes"].items():assert digest((ROOT/path).read_bytes())==sha,path
for name in ["noise_bias_tests.rs","noise_change_diagnostics.rs"]:
    if not FROZEN:assert not (ROOT/crate/"src"/name).exists()

bias_source=load("measured-source-hashes.json")
common_source=load("common-change-measured-source-hashes.json")
for path,sha in common_source["hashes"].items():
    suffix=path.split("sotf-plugin-denoiser/",1)[1]
    assert digest((HERE/"measured-source"/suffix).read_bytes())==sha,path
registration=git(["show",base+":"+crate+"/src/lib.rs"])
first=registration+b"\n#[cfg(test)]\nmod noise_bias_tests;\n"
assert (HERE/"bias-only-registration.rs").read_bytes()==first
assert digest(first)==bias_source["hashes"][crate+"/src/lib.rs"]
assert (HERE/"measured-source/src/lib.rs").read_bytes()==first+b"\n#[cfg(test)]\nmod noise_change_diagnostics;\n"
assert (HERE/"diagnostic-source.rs").read_bytes()==(HERE/"measured-source/src/noise_bias_tests.rs").read_bytes()
for path,sha in bias_source["hashes"].items():
    if path.endswith("/src/lib.rs"):continue
    data=(HERE/"diagnostic-source.rs").read_bytes() if path.endswith("/src/noise_bias_tests.rs") else git(["show",base+":"+path])
    assert digest(data)==sha,path

bias=load("validation.json")
assert bias["exit_code"]==0 and len(extract(bias["stdout"],"NOISE_BIAS "))==1280
assert load("analysis.json")["records"]==1280
assert len(load("analysis.json")["groups"])==20
ratio=load("ratio-validation.json")
assert ratio["exit_code"]==0
rows=extract(ratio["stdout"],"COMMON_CHANGE ")
assert len(rows)==1424
assert "raw_reference_lifecycle bins=21,83 fixed_lag=4 reset=true grouping_exact=true gain_independent=true heap_allocations=0 heap_frees=0" in ratio["stdout"]
assert "qa" in ratio["command"]
for row in rows:
    assert not row["production_rebase"]
    assert all(event["frame"]-event["reference_frame"]==4 for event in row["events"])
assert load("ratio-validation-initial-compile.json")["exit_code"]==101
analysis=load("ratio-analysis.json")
assert analysis["decision"]=="NO-GO for production state rebase"
assert not analysis["production_rebase_go"] and not analysis["production_changed"]
assert analysis["false_control_reports"]==221 and analysis["false_control_events"]==1257
assert len(analysis["positive_guards"])==6
assert all(g["timely_detections"]==64 and g["off_change_events"]==0 and g["passed"] for g in analysis["positive_guards"])
assert all(s["events"]==0 for s in analysis["summary"] if s["condition"].startswith("stationary_"))

for case in load("gaussian-ratio-oracle.json")["cases"]:
    n=case["bins"]
    c=Fraction(case["rational_threshold"])
    x=n*(1-c)/2
    p,q=x.numerator,x.denominator
    tail=Fraction(sum((-1)**j*comb(n,j)*(p-j*q)**n for j in range(p//q+1)),q**n*factorial(n))
    assert tail<=Fraction(case["hop"],1000*48000*3600)
    assert case["mean_t_threshold"]>=float(c)
    size=case["fft_size"]
    bins=list(range(ceil(100*size/48000),floor(4000*size/48000)+1,2))
    assert len(bins)==n and all(a+b not in [0,1,size-1,size] for a in bins for b in bins)
    assert all(abs(a-b)!=1 for a in bins for b in bins)
    assert all(r["threshold"]==case["mean_t_threshold"] for r in rows if r["fft_size"]==size)
assert len(load("analytic-reference.json"))==2
assert len(load("selection-bias-reference.json"))==10
assert len(load("physical-clock-h0-reference.json"))==40
assert all(r["cap_frames"]==0 and r["maximum_noise_to_true_psd_during_step"]<.001 for r in load("fixed-prior-reference.json"))
assert all(r["exit_code"]==0 for r in load("independent-replay.json"))
assert len(load("independent-replay.json"))==7
baseline=HERE.parent/"remaining-denoiser-snr-risk-20261024"
assert json.loads((baseline/"restored-validation.json").read_text())["exit_code"]==0
assert json.loads((baseline/"restored-clippy.json").read_text())["exit_code"]==0
print("NO-GO retained: 1280 PSD rows,1424 score reports,1257 wanted false events; exact Gaussian oracle/lifecycle and all positive controls preserved; " + ("frozen" if FROZEN else "active and indexed") + " shipping source matches validated 21806c0.")
