#!/usr/bin/env python3
"""Solve the declared quadratic risk independently in f64."""
import json
from pathlib import Path
rows=[]
for xi in [0., 1e-8, .01, .1, 1., 10., 100., 1e5]:
    for c in [1., 2., 10., 100., 1e4]:
        # Assume unit noise variance; xi is source power. Extra noise risk is
        # weighted by the modeled noise fraction, not a speech event posterior.
        noise_fraction = 1. / (xi+1.)
        a = xi + 1. + (c-1.)*noise_fraction
        b = -2.*xi
        optimum = -b/(2.*a)
        risk=lambda g:xi*(1.-g)**2+g*g+(c-1.)*noise_fraction*g*g
        assert abs(2.*a*optimum+b)<1e-9
        assert xi/(xi+c) <= optimum+1e-15 <= xi/(xi+1.)+2e-15
        for delta in [-.1,-.001,.001,.1]:
            assert risk(optimum)<=risk(optimum+delta)+1e-10
        if c==1.:assert optimum==xi/(xi+1.)
        rows.append(dict(xi=xi,multiplier=c,gain=optimum,risk=risk(optimum)))
for c in [1.,2.,10.,100.,1e4]:
    gains=[r["gain"] for r in rows if r["multiplier"]==c]
    assert all(a<=b for a,b in zip(gains,gains[1:]))
for xi in [0.,1e-8,.01,.1,1.,10.,100.,1e5]:
    gains=[r["gain"] for r in rows if r["xi"]==xi]
    assert all(a>=b for a,b in zip(gains,gains[1:]))
Path(__file__).with_name("independent-risk-reference.json").write_text(json.dumps(dict(cases=len(rows),rows=rows),indent=2)+"\n")
print(f"Independent noise-fraction risk reference: {len(rows)} optimum/limit/monotonic cases pass.")
