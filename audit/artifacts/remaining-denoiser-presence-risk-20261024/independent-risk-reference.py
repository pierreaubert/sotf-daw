#!/usr/bin/env python3
"""Derive a reference optimum from the quadratic risk coefficients."""
import json
from pathlib import Path

rows = []
for clean_power in [0., 1e-8, .01, .1, 1., 10., 1e5]:
    for multiplier in [1., 2., 10., 100., 1e4]:
        previous = -1.
        for p in [0., .1, .5, .9, 1.]:
            # Independent f64 optimum from polynomial coefficients A*g^2+B*g+D.
            a = clean_power + 1. + (multiplier - 1.) * (1. - p)
            b = -2. * clean_power
            optimum = -b / (2. * a)
            def risk(g):
                return clean_power*(1.-g)**2 + g*g + (multiplier-1.)*(1.-p)*g*g
            assert abs(2.*a*optimum+b) < 1e-9
            assert 0. <= optimum <= 1.
            assert clean_power/(clean_power+multiplier) <= optimum+1e-15
            assert optimum <= clean_power/(clean_power+1.)+1e-15
            assert optimum >= previous
            previous = optimum
            for delta in [-.1, -.001, .001, .1]:
                assert risk(optimum) <= risk(optimum+delta)+1e-10
            if p == 0.:
                assert optimum == clean_power/(clean_power+multiplier)
            if p == 1.:
                assert optimum == clean_power/(clean_power+1.)
            rows.append(dict(xi=clean_power, multiplier=multiplier, p=p, gain=optimum, risk=risk(optimum)))
for p in [0., .1, .5, .9, 1.]:
    for c in [1., 2., 10., 100., 1e4]:
        q = sorted((r for r in rows if r["p"] == p and r["multiplier"] == c), key=lambda r:r["xi"])
        assert all(a["gain"] <= b["gain"] for a,b in zip(q,q[1:]))
    for xi in [0., 1e-8, .01, .1, 1., 10., 1e5]:
        q = sorted((r for r in rows if r["p"] == p and r["xi"] == xi), key=lambda r:r["multiplier"])
        assert all(a["gain"] >= b["gain"] for a,b in zip(q,q[1:]))
out=Path(__file__).with_name("independent-risk-reference.json")
out.write_text(json.dumps(dict(cases=len(rows),rows=rows),indent=2)+"\n")
print(f"Independent quadratic-risk optimum, endpoint, bounds and p-monotonicity: {len(rows)} cases pass.")
