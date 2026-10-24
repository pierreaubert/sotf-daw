#!/usr/bin/env python3
"""Verify preserved rejection evidence and exact source restoration."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
FROZEN="--frozen" in sys.argv
HERE=Path(__file__).resolve().parent
ROOT=HERE.parents[2]
a=json.loads((HERE/"analysis.json").read_text())
assert not a["declared_quality_guards_passed"]
assert len(a["vowel_guard"])==2 and not any(r["passed"] for r in a["vowel_guard"])
assert len(a["development_sdr"])==36 and sum(not r["passed"] for r in a["development_sdr"])==1
assert all(r["passed"] for r in a["settled_noise_guard"])
assert len(a["added_noise_step_guard"])==12 and sum(not r["passed"] for r in a["added_noise_step_guard"])==4
assert a["functional_exit"]==101
assert json.loads((HERE/"quality-validation.json").read_text())["exit_code"]==0
for path,digest in json.loads((HERE/"candidate-source-hashes.json").read_text()).items():
    suffix=path.split("sotf-plugin-denoiser/",1)[1]
    assert hashlib.sha256((HERE/"candidate-source"/suffix).read_bytes()).hexdigest()==digest
    source=ROOT/path
    if suffix=="src/decision_directed.rs":
        if not FROZEN:assert not source.exists()
    else:
        expected=subprocess.check_output(["rtk","proxy","git","show","21806c0:"+path],cwd=ROOT)
        assert (HERE/"restored-source"/suffix).read_bytes()==expected
        if not FROZEN:assert source.read_bytes()==expected,source
for name in ["restored-validation", "restored-clippy"]:
    assert json.loads((HERE/(name+".json")).read_text())["exit_code"]==0,name
def measurements(path):
    raw=json.loads(path.read_text())["stdout"]
    result=[]
    for line in raw.splitlines():
        offset=line.find("{")
        if offset<0:continue
        try:row=json.JSONDecoder().raw_decode(line[offset:])[0]
        except ValueError:continue
        if isinstance(row,dict) and row.get("measurement") in {"recorded_estimator","recorded_denoiser_onset","independent_noise_channel"}:result.append(row)
    return sorted(result,key=lambda row:json.dumps(row,sort_keys=True))
before=measurements(HERE.parent/"remaining-denoiser-zero-reduction-20261024"/"validation.json")
after=measurements(HERE/"restored-validation.json")
assert len(before)==48 and after==before
print("Rejected SNR-risk evidence preserved; three independent quality guards fail; " + ("frozen restored checkpoint" if FROZEN else "current production source") + " matches 21806c0 with passing tests and strict Clippy.")
