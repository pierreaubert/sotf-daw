#!/usr/bin/env python3
"""Verify the preserved presence diagnosis independently of current sources."""
import json
from pathlib import Path
import re
HERE=Path(__file__).resolve().parent
raw=json.loads((HERE/"presence-diagnostics.json").read_text())
assert raw["exit_code"]==0
baseline=json.loads((HERE.parent/"remaining-denoiser-zero-reduction-20261024"/"validation.json").read_text())["stdout"]
def extract(text,marker):
    return [json.JSONDecoder().raw_decode(text[m.end():])[0] for m in re.finditer(marker,text)]
voiced=lambda text:sorted(extract(text,r"(?<!NOISE_)COMPONENT "),key=lambda row:json.dumps(row,sort_keys=True))
assert len(voiced(raw["stdout"]))==720 and voiced(raw["stdout"])==voiced(baseline)
noise=extract(raw["stdout"],"NOISE_COMPONENT ")
assert len(noise)==144
active=[r for r in noise if r["gain_frame_active"]]
assert len(active)==128
maximum=max(abs(r["observed_power"]-r["true_noise_periodogram"])/(1+r["true_noise_periodogram"]) for r in active)
assert maximum<2e-6
assert all(r["observed_power"] is None for r in noise if not r["gain_frame_active"])
rows=extract(raw["stdout"],"NOISE_PRESENCE ")
assert len(rows)==36
for t in [.6,.9]:
    row=next(r for r in rows if not r["low_latency"] and r["channel"]==0 and r["requested_source_time"]==t)
    assert row["power_weighted_p"]>.99 and row["bins_above_half"]==row["bins"]
for name in ["presence-diagnostics-initial-postgain-read", "presence-diagnostics-initial-bootstrap-read"]:
    assert json.loads((HERE/(name+".json")).read_text())["exit_code"]==101
assert json.loads((HERE/"independent-risk-reference.json").read_text())["cases"]==175
print(f"Presence hypothesis not selected: original 720 traces exact; 128 independent noise DFT checks pass (max {maximum:.9g}); noise-only false-presence evidence and failed trace reads preserved.")
