#!/usr/bin/env python3
"""Scalar tracking reference, not a production estimator or an MMSE claim.

Algorithm source: Gerkmann/Hendriks primary author reference v1.0,
https://fiona.uni-hamburg.de/11888f86/noisepowproposed.m.
See THIRD_PARTY_NOTICES.md for its BSD-style copyright/license notice.
Physical-clock conversion is declared here; the original reference uses
32 ms frames and 16 ms hops with alpha-mean=.9, alpha-PSD=.8.
"""
import json
import math
from pathlib import Path

rows=[]
for hop in [256,768,1024]:
    dt=hop/48000
    alpha_mean=.9**(dt/.016)
    alpha_psd=.8**(dt/.016)
    noise=1.
    mean_p=.5
    maximum_during_step=0.
    cap_frames=0
    trajectory=[]
    for frame in range(math.ceil(1./dt)):
        t=(frame+1)*dt
        observed=10000. if .25<t<.75 else 1.
        xi=10**(15/10)
        log_odds=-math.log1p(xi)+xi/(1+xi)*observed/noise
        # Stable logistic evaluation is equivalent to the reference capped GLR
        # throughout this bounded range; p rounds to one for huge odds.
        e=math.exp(min(log_odds,200.))
        p=e/(1+e)
        mean_p=alpha_mean*mean_p+(1-alpha_mean)*p
        if mean_p>.99:
            p=min(p,.99)
            cap_frames+=1
        estimate=p*noise+(1-p)*observed
        noise=alpha_psd*noise+(1-alpha_psd)*estimate
        if observed==10000.:maximum_during_step=max(maximum_during_step,noise/observed)
        trajectory.append(dict(time=t,observed_power=observed,noise_psd=noise,p=p,mean_p=mean_p))
    assert cap_frames==0
    assert maximum_during_step<.001
    rows.append(dict(hop=hop,alpha_mean=alpha_mean,alpha_psd=alpha_psd,cap_frames=cap_frames,maximum_noise_to_true_psd_during_step=maximum_during_step,trajectory=trajectory))
Path(__file__).with_name("fixed-prior-reference.json").write_text(json.dumps(rows,indent=2)+"\n")
print("Fixed-prior SPP reference: short 40 dB mean-power step remains untracked before the anti-stall cap at all three clocks; no production port selected.")
