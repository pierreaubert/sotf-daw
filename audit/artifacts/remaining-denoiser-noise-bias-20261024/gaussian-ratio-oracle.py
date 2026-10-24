#!/usr/bin/env python3
"""Exact Gaussian-H0 ratio/sum oracle; never an empirical calibration."""
from fractions import Fraction as F
from math import comb, factorial, nextafter, inf
from pathlib import Path
import json

def ih(x,n):
    # Exact Irwin-Hall CDF at rational x, including cancellation in integers.
    if x<=0:return F(0)
    if x>=n:return F(1)
    p,q=x.numerator,x.denominator
    value=sum((-1)**j*comb(n,j)*(p-j*q)**n for j in range(p//q+1))
    return F(value,q**n*factorial(n))
assert ih(F(1,2),1)==F(1,2)
assert ih(F(1,2),2)==F(1,8)
assert ih(F(1),2)==F(1,2)
assert ih(F(1),3)==F(1,6)
assert ih(F(2),3)==F(5,6)
rows=[]
for size,n in [(2048,83),(512,21)]:
    hop=size//2
    alpha=F(hop,1000*48000*3600)
    low,high=F(0),F(n,2)
    for _ in range(48):
        middle=(low+high)/2
        if ih(middle,n)<=alpha:low=middle
        else:high=middle
    threshold=1-2*low/n
    tail=ih(low,n)
    assert tail<=alpha and ih(high,n)>alpha
    assert ih(F(n,2),n)==F(1,2)
    c=nextafter(float(threshold),inf)
    power=[]
    for scale in [10,100,10000]:
        # If every T exceeds c, its mean exceeds c: a rigorous lower bound
        # under the independent common-scale Gaussian H1, not exact power.
        prob_single=1-(1+c)/(scale*(1-c)+1+c)
        power.append(dict(variance_scale=scale,all_bins_exceed_lower_bound=prob_single**n))
    rows.append(dict(fft_size=size,hop=hop,bins=n,hourly_familywise_budget=.001,per_frame_budget=float(alpha),mean_t_threshold=c,exact_tail_at_rational_threshold=float(tail),rational_threshold=str(threshold),h1_lower_bounds=power,scope="real white Gaussian only; proper independent interior bins and single disjoint-window raw reference"))
Path(__file__).with_name("gaussian-ratio-oracle.json").write_text(json.dumps(dict(cases=rows),indent=2)+"\n")
print(json.dumps(rows,indent=2))
