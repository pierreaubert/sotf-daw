"""Recheck the saved dual witness with scalar cosine sums, without CG or FFT."""
import importlib.util
import json
import math
from pathlib import Path

ART = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("frozen", ART / "frozen-prototype.py")
model = importlib.util.module_from_spec(spec)
spec.loader.exec_module(model)
result = json.loads((ART / "result.json").read_text())
bands = [("Peak", 500, 2, 9), ("Peak", 2000, 1.5, -6), ("Lowshelf", 150, 1, 3)]
rows = []
for row in result["rows"]:
    rate, count = row["rate"], row["grid_points"]
    half = count - 1
    targets = [model.target(max(1.0, j * rate / (2 * half)), rate, bands)
               for j in range(count)]
    assert abs(row["eigenvalue_lower_bound"] - min(1 / t**2 for t in targets) * half / count) < 1e-15
    a = [1.0] + [math.sqrt(2) * math.cos(math.tau * 500 * k / rate) for k in range(1, 512)]
    witnesses = []
    for branch in row["branches"]:
        h = branch["coefficients"]
        # Each frequency is evaluated directly. No FFT or solver imported.
        values = [h[0] + 2 * math.fsum(h[k] * math.cos(math.pi * j * k / half)
                                     for k in range(1, len(h))) for j in range(count)]
        errors = [v / t - 1 for v, t in zip(values, targets)]
        mse = math.fsum(e * e for e in errors) / count
        weighted = [e / t for e, t in zip(errors, targets)]
        gradient = [math.fsum(weighted) / count] + [math.sqrt(2) * math.fsum(
            weighted[j] * math.cos(math.pi * j * k / half) for j in range(count)) / count
            for k in range(1, 512)]
        residual = [g - branch["multiplier"] * basis for g, basis in zip(gradient, a)]
        residual_norm = math.sqrt(math.fsum(v * v for v in residual))
        center = model.response(h, 500, rate)
        constraint_residual = center - branch["chosen_boundary"]
        lower, upper = branch["signed_interval"]
        multiplier = branch["multiplier"]
        assert (multiplier >= 0 and branch["chosen_boundary"] == lower
                or multiplier <= 0 and branch["chosen_boundary"] == upper
                or multiplier == 0 and lower <= branch["chosen_boundary"] <= upper)
        dual = mse - 2 * multiplier * constraint_residual
        bound = dual - residual_norm**2 / row["eigenvalue_lower_bound"] - branch["floating_roundoff_safety_allowance_mse"]
        assert abs(mse - branch["primal_mse"]) < 1e-12
        assert abs(constraint_residual) < 1e-10
        assert residual_norm < 1e-9
        assert abs(bound - branch["dual_lower_mse"]) < 1e-12
        witnesses.append(dict(direct_mse=mse, scalar_normal_residual_norm=residual_norm,
                              scalar_constraint_residual=constraint_residual,
                              scalar_dual_lower_mse=bound))
    lower_bound = min(w["scalar_dual_lower_mse"] for w in witnesses)
    impossible = lower_bound > row["baseline_mse"]
    assert impossible == row["baseline_rms_nonregression_impossible"]
    rows.append(dict(rate=rate, witnesses=witnesses,
                     scalar_constrained_rms_lower_bound=math.sqrt(max(0, lower_bound)),
                     baseline_rms=row["baseline"]["rms_relative_error"],
                     rms_nonregression_impossible_for_single_constraint=impossible))
out = dict(method="Independent scalar cosine synthesis and normal-gradient sums; no FFT or CG",
           proof_status="Floating numerical certificate with residual and roundoff safety allowances, not interval arithmetic",
           rows=rows)
(ART / "verification.json").write_text(json.dumps(out, indent=2) + "\n")
print(json.dumps(out, indent=2))
