#!/usr/bin/env python3
"""Summarize fixed-seed PSD observations without calibrating a posterior."""
import json
from pathlib import Path
import re
import statistics
HERE=Path(__file__).resolve().parent
raw=json.loads((HERE/"validation.json").read_text())["stdout"]
rows=[json.JSONDecoder().raw_decode(raw[m.end():])[0] for m in re.finditer("NOISE_BIAS ",raw)]
assert len(rows)==1280
out=[]
for low in [False,True]:
    for channel in [0,1]:
        for time in [.2,.3,.45,.6,.9]:
            q=[r for r in rows if r["low_latency"]==low and r["channel"]==channel and r["requested_source_time"]==time]
            assert len(q)==64
            out.append(dict(low_latency=low,channel=channel,time=time,seeds=64,observed_ratio_mean=statistics.mean(r["observed_mean_ratio"] for r in q),observed_ratio_sd=statistics.stdev(r["observed_mean_ratio"] for r in q),estimated_ratio_mean=statistics.mean(r["estimated_mean_ratio"] for r in q),estimated_ratio_sd=statistics.stdev(r["estimated_mean_ratio"] for r in q),estimated_ratio_seed_median=statistics.median(r["estimated_mean_ratio"] for r in q),mean_p=statistics.mean(r["mean_p"] for r in q),power_weighted_p_mean=statistics.mean(r["power_weighted_p"] for r in q)))
(HERE/"analysis.json").write_text(json.dumps(dict(records=1280,groups=out,native_acceptance=False,heldout_acceptance=False),indent=2)+"\n")
print("Fixed 1280-row noise-PSD analysis replayed; no population/posterior acceptance.")
