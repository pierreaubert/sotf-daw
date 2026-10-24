#!/usr/bin/env python3
"""Check white-uniform-noise STFT moments independently of production FFTs."""
import cmath
import json
import math
from pathlib import Path
out=[]
for n in [512,2048]:
    first=math.ceil(100*n/48000)
    last=math.floor(4000*n/48000)
    k=last-first+1
    w2=[math.sin(math.pi*i/n)**2 for i in range(n)]
    energy=sum(w2)
    fourth=sum(v*v for v in w2)
    assert abs(energy-n/2)<1e-10
    assert abs(fourth-3*n/8)<1e-10
    # Direct f64 covariance DFT of squared window; no production transform.
    covariance={d:sum(v*cmath.exp(-2j*math.pi*d*i/n) for i,v in enumerate(w2)) for d in range(2*last+1)}
    gaussian=sum(abs(covariance[abs(a-b)])**2+abs(covariance[a+b])**2 for a in range(first,last+1) for b in range(first,last+1))/(k*k)
    # Unit variance uniform noise has fourth cumulant -6/5.
    direct=(gaussian-(6/5)*fourth)/(energy*energy)
    closed=(3*k-1)/(2*k*k)-9/(5*n)
    assert abs(direct-closed)<1e-12
    out.append(dict(fft_size=n,first_bin=first,last_bin=last,bins=k,window_energy=energy,window_fourth_sum=fourth,normalized_band_mean_variance=direct,closed_form_variance=closed,relative_sd_single=math.sqrt(direct),relative_sd_64_seeds_3_disjoint_windows=math.sqrt(direct/192)))
Path(__file__).with_name("analytic-reference.json").write_text(json.dumps(out,indent=2)+"\n")
print(json.dumps(out,indent=2))
