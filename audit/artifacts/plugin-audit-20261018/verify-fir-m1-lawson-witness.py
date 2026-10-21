"""Independently check the saved finite-grid FIR approximation witnesses.

Uses only Python scalar math, not production biquads, the fitter, or its FFT.
A witness annihilates all centered N-tap linear FIRs. Its target inner product
bounds the achievable relative error. This does not prove infeasibility at the
four isolated FIR-M1 probes or for a different support/phase/latency contract.
"""
import json
import math
from pathlib import Path

ROOT = Path(__file__).resolve().parent
raw = json.loads((ROOT / "fir-m1-lawson-witness-experiment.json").read_text())
witnesses = [json.loads(line.split(" ", 1)[1]) for line in raw["stderr"].splitlines() if line.startswith("lawson_certificate ")]
assert len(witnesses) == 2
rows = []
for rate, witness in zip([48000, 96000], witnesses):
    target, dual = witness["target"], witness["dual"]
    assert len(target) == len(dual) == 4097
    size = 2 * (len(target) - 1)
    scale = max(map(abs, dual))
    dual = [v / scale for v in dual]
    weights = [1] + [2] * (len(target) - 2) + [1]
    def prototype(f):
        product = 1.0
        for shape, center, q, gain in [("Peak", 500, 2, 9), ("Peak", 2000, 1.5, -6), ("Lowshelf", 150, 1, 3)]:
            ratio = math.tan(math.pi*f/rate) / math.tan(math.pi*center/rate)
            r2 = ratio*ratio
            a = 10 ** (gain/40)
            if shape == "Peak":
                product *= math.sqrt(((1-r2)**2+(a*ratio/q)**2)/((1-r2)**2+(ratio/(a*q))**2))
            else:
                product *= a*math.sqrt(((a-r2)**2+2*a*r2)/((1-a*r2)**2+2*a*r2))
        return product
    independent = [prototype(max(1, k*rate/size)) for k in range(len(target))]
    target_error = max(abs(a-b)/b for a,b in zip(target, independent))
    assert target_error < 1e-8
    moments = [math.fsum(w*y*math.cos(math.tau*k*lag/size) for k,(w,y) in enumerate(zip(weights,dual)))/size for lag in range(witness["taps"]//2)]
    assert max(map(abs, moments)) < 1e-12
    numerator = abs(math.fsum(w*y*t for w,y,t in zip(weights,dual,independent))/size)
    denominator = math.fsum(w*abs(y)*t for w,y,t in zip(weights,dual,independent))/size
    mean_target = math.fsum(w*t for w,t in zip(weights,independent))/size
    leakage = mean_target*(abs(moments[0])+2*math.fsum(map(abs,moments[1:])))
    lower = max(0, (numerator-leakage)/(denominator+leakage))
    if rate == 96000:
        assert lower > 10**(0.05/20)-1
    rows.append(dict(rate=rate,taps=witness["taps"],grid_points=len(target),max_target_relative_difference=target_error,max_annihilated_moment=max(map(abs,moments)),leakage_allowance=leakage,relative_error_lower_bound=lower,conservative_db_error_lower_bound=20*math.log10(1+lower),required_relative_bound_for_005_db=10**(0.05/20)-1))
print(json.dumps(dict(method="Independent scalar prototype and cosine sums; floating point numerical witness, not interval arithmetic proof",rows=rows),indent=2))
