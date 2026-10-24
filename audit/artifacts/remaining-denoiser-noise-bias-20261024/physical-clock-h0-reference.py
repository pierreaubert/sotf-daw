#!/usr/bin/env python3
"""Counterfactual known-H0 smoothing, not a full nonlinear MCRA model."""
import json
import math
from pathlib import Path
import struct
HERE=Path(__file__).resolve().parent
groups=json.loads((HERE/"analysis.json").read_text())["groups"]
f32=lambda value:struct.unpack("f",struct.pack("f",value))[0]
out=[]
for n in [2048,512]:
    hop=n//2
    dt=hop/48000
    bootstrap=math.ceil(5120/hop)
    prefix=[0.]
    for k in range(n):prefix.append(prefix[-1]+math.sin(math.pi*k/n)**2)
    def power(end,channel):
        valid=prefix[n]-prefix[max(0,n-end)]
        if channel==1:return f32(.01)**2/3*valid
        low=max(0,12001-(end-n))
        high=min(n,36000-(end-n))
        high_energy=prefix[high]-prefix[low] if high>low else 0.
        return f32(.001)**2/3*valid+(f32(.1)**2-f32(.001)**2)/3*high_energy
    for channel in [0,1]:
        for fast in [False,True]:
            alpha=f32(f32(.9)*f32(.9)) if fast else f32(.9)
            noise=0.
            counter=0
            next_time=0
            times=[.2,.3,.45,.6,.9]
            for end in range(hop,48001,hop):
                target=power(end,channel)
                if counter<bootstrap:
                    noise+=target
                    counter+=1
                    if counter==bootstrap:noise/=bootstrap
                else:noise=alpha*noise+(1-alpha)*target
                center=(end-hop)/48000
                if next_time<len(times) and center>=times[next_time]:
                    time=times[next_time]
                    actual=next(g for g in groups if g["low_latency"]==(n==512) and g["channel"]==channel and g["time"]==time)
                    out.append(dict(fft_size=n,channel=channel,quiet_fast=fast,alpha=alpha,physical_tau_seconds=-dt/math.log(alpha),requested_source_time=time,window_end_frame=end,source_window_center=center,window_end_time=end/48000,center_age_after_fall=center-.75,end_age_after_fall=end/48000-.75,h0_mean_noise_psd=noise,expected_psd=target,h0_noise_to_truth_ratio=noise/target,measured_mcra_ratio=actual["estimated_ratio_mean"]))
                    next_time+=1
(HERE/"physical-clock-h0-reference.json").write_text(json.dumps(out,indent=2)+"\n")
print("Physical-clock known-H0 reference replayed; downward ratios are not independently classified as bugs.")
