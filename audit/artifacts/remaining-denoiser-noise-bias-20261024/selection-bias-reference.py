#!/usr/bin/env python3
"""An independent one-step selection-bias example, not a full MCRA model."""
import math
import json
from pathlib import Path

def simpson(f,a,b):
    n=10000
    h=(b-a)/n
    return h/3*(f(a)+f(b)+sum((4 if i%2 else 2)*f(a+i*h) for i in range(1,n)))
rows=[]
for fast in [False,True]:
    # Frozen pre-frame p=0, alpha_s=.9, alpha_p=.7. The binary indicator
    # gives post-frame p=0 or .3. Fast alpha^2 applies only to the quiet arm.
    alpha0=.9**2 if fast else .9
    alpha1=.9+.1*.3
    w0=1-alpha0
    w1=1-alpha1
    for tau in [.1,.5,1.,2.,5.]:
        # True/previous noise PSD both one, fresh independent exponential
        # normalized periodogram gamma. Indicator I=1 iff gamma>tau.
        expected=1+(w1-w0)*tau*math.exp(-tau)
        f=lambda x,w:(1+w*(x-1))*math.exp(-x)
        integrated=simpson(lambda x:f(x,w0),0,tau)+simpson(lambda x:f(x,w1),tau,40)
        assert abs(expected-integrated)<1e-8
        rows.append(dict(fast=fast,threshold=tau,quiet_update_weight=w0,indicator_update_weight=w1,expected_next_noise_ratio=expected,numerical_integral=integrated,bias_db=10*math.log10(expected)))
Path(__file__).with_name("selection-bias-reference.json").write_text(json.dumps(rows,indent=2)+"\n")
print(f"Independent single-step adaptive-averaging selection bias: {len(rows)} quadrature cases pass.")
