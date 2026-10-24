"""Finite cosine-support weighted-L2 feasibility; numerical, not interval proof."""
import importlib.util
import json
import math
from pathlib import Path

ART = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("frozen", ART / "frozen-prototype.py")
model = importlib.util.module_from_spec(spec)
spec.loader.exec_module(model)
SQRT2 = math.sqrt(2)


def dot(a, b):
    return math.fsum(x * y for x, y in zip(a, b))


class Objective:
    def __init__(self, rate, bands, size=8192, dimensions=512):
        self.rate, self.bands = rate, bands
        self.size, self.dimensions = size, dimensions
        self.half = size // 2
        self.count = self.half + 1
        self.targets = [model.target(max(1.0, j * rate / size), rate, bands)
                        for j in range(self.count)]
        self.weights = [1 / value**2 for value in self.targets]
        # Trapezoidal cosine Gram is M*I for these modes. Full endpoint
        # weights add a PSD term, so B'B/L >= M/L*I. Weighting preserves
        # Q >= min(1/target^2)*M/L*I; this bounds residual error in the dual.
        self.eigenvalue_lower_bound = min(self.weights) * self.half / self.count
        self.rhs = self.adjoint([1 / t for t in self.targets])

    def basis(self, frequency):
        return [1.0] + [SQRT2 * math.cos(math.tau * frequency * k / self.rate)
                        for k in range(1, self.dimensions)]

    def synthesize(self, x):
        impulse = [0.0] * self.size
        impulse[0] = x[0]
        for k, value in enumerate(x[1:], 1):
            impulse[k] = impulse[-k] = value / SQRT2
        return [z.real for z in model.fft(impulse)[:self.count]]

    def adjoint(self, y):
        spectrum = model.fft(y + y[-2:0:-1])
        # Half the even-extension FFT omits half of each endpoint.
        return [((spectrum[k].real + y[0] + (-1)**k * y[-1]) / 2)
                * (1.0 if k == 0 else SQRT2) / self.count
                for k in range(self.dimensions)]

    def normal(self, x):
        return self.adjoint([w * value for w, value in zip(self.weights, self.synthesize(x))])

    def objective(self, x):
        return math.fsum((value / t - 1)**2
                         for value, t in zip(self.synthesize(x), self.targets)) / self.count

    def solve(self, rhs):
        x = [0.0] * self.dimensions
        residual = list(rhs)
        direction = list(residual)
        rr = dot(residual, residual)
        initial = math.sqrt(rr)
        iterations = 0
        for iterations in range(1, 201):
            qd = self.normal(direction)
            curvature = dot(direction, qd)
            assert curvature > 0
            alpha = rr / curvature
            x = [a + alpha * b for a, b in zip(x, direction)]
            residual = [a - alpha * b for a, b in zip(residual, qd)]
            new_rr = dot(residual, residual)
            if math.sqrt(new_rr) < 2e-13 * max(1.0, initial):
                break
            direction = [a + new_rr / rr * b for a, b in zip(residual, direction)]
            rr = new_rr
        else:
            raise AssertionError("CG did not converge within fixed 200-iteration cap")
        actual = [a - b for a, b in zip(self.normal(x), rhs)]
        norm = math.sqrt(dot(actual, actual))
        assert norm < 1e-10 * max(1.0, initial)
        return x, dict(iterations=iterations, normal_residual_norm=norm,
                       relative_normal_residual=norm / initial)


def analyze(rate):
    bands = [("Peak", 500, 2, 9), ("Peak", 2000, 1.5, -6), ("Lowshelf", 150, 1, 3)]
    problem = Objective(rate, bands)
    baseline = model.design(rate, 1024, bands)
    bx = [baseline[0]] + [SQRT2 * h for h in baseline[1:]]
    baseline_metrics = model.metrics(baseline, rate, bands)
    baseline_mse = problem.objective(bx)
    assert abs(baseline_mse - baseline_metrics["rms_relative_error"]**2) < 1e-15
    # Independent direct cosine checks on deterministic modes and grid rows.
    test = [math.sin(k * .731) / (1 + k) for k in range(problem.dimensions)]
    synthesis = problem.synthesize(test)
    direct_error = max(abs(synthesis[j] - dot(problem.basis(j * rate / problem.size), test))
                       for j in [0, 1, 7, 43, 511, 1023, 2048, 4096])
    y = [math.cos(j * .171) for j in range(problem.count)]
    adjoint = problem.adjoint(y)
    adjoint_error = max(abs(adjoint[k] - math.fsum(y[j] * (1.0 if k == 0 else
                         SQRT2 * math.cos(math.pi * j * k / problem.half))
                         for j in range(problem.count)) / problem.count)
                        for k in [0, 1, 2, 31, 127, 511])
    assert direct_error < 1e-10 and adjoint_error < 1e-11
    optimum, convergence = problem.solve(problem.rhs)
    center = next(center for shape, center, q, gain in bands if shape == "Peak")
    a = problem.basis(center)
    direction, direction_convergence = problem.solve(a)
    center_target = model.target(center, rate, bands)
    lower = center_target * 10**(-.05 / 20)
    upper = center_target * 10**(.05 / 20)
    predicted = dot(a, optimum)
    denominator = dot(a, direction)
    assert denominator > 0
    optimum_mse = problem.objective(optimum)
    branches = []
    for low, high in [(lower, upper), (-upper, -lower)]:
        boundary = min(high, max(low, predicted))
        multiplier = (boundary - predicted) / denominator
        constrained = [x + multiplier * z for x, z in zip(optimum, direction)]
        qx = problem.normal(constrained)
        dual_rhs = [r + multiplier * ai for r, ai in zip(problem.rhs, a)]
        residual = [q - r for q, r in zip(qx, dual_rhs)]
        allowance = dot(residual, residual) / problem.eigenvalue_lower_bound
        # L=f(x)-2*lambda*(a'x-c), minimized over unrestricted x.
        # lambda's sign chooses the nearest edge of this interval. Subtract
        # the residual allowance from this approximate dual minimum.
        primal_mse = problem.objective(constrained)
        cancelled_dual = 1 - 2 * dot(dual_rhs, constrained) + dot(constrained, qx) + 2 * multiplier * boundary
        # Evaluate the same Lagrangian through residual squares, avoiding
        # cancellation between O(1) terms when the MSE is much smaller.
        approximate_dual = primal_mse - 2 * multiplier * (dot(a, constrained) - boundary)
        roundoff_allowance = 512 * math.ulp(1.0) * (1 + abs(primal_mse)
                                                  + abs(2 * multiplier * boundary))
        dual_lower = approximate_dual - allowance - roundoff_allowance
        formula_mse = optimum_mse + (boundary - predicted)**2 / denominator
        assert abs(primal_mse - formula_mse) < 1e-11
        assert abs(primal_mse - approximate_dual) < 1e-11
        assert (multiplier >= 0 if predicted < low else multiplier <= 0 if predicted > high else multiplier == 0)
        coefficients = [constrained[0]] + [x / SQRT2 for x in constrained[1:]]
        branches.append(dict(signed_interval=[low, high], chosen_boundary=boundary,
                             multiplier=multiplier, constraint_residual=dot(a, constrained)-boundary,
                             normal_residual_norm=math.sqrt(dot(residual, residual)),
                             residual_allowance_mse=allowance, dual_lower_mse=dual_lower,
                             floating_roundoff_safety_allowance_mse=roundoff_allowance,
                             cancelled_dual_evaluation_difference=cancelled_dual-approximate_dual,
                             primal_mse=primal_mse, rms_relative_error=math.sqrt(primal_mse),
                             formula_mse=formula_mse,
                             metrics=model.metrics(coefficients, rate, bands), coefficients=coefficients))
    best = min(branches, key=lambda row: row["primal_mse"])
    lower_bound = min(row["dual_lower_mse"] for row in branches)
    return dict(rate=rate, taps=1024, independent_cosine_dimensions=512, grid_points=problem.count,
                natural_constraint_frequency=center, db_tolerance=.05,
                baseline=baseline_metrics, baseline_mse=baseline_mse,
                eigenvalue_lower_bound=problem.eigenvalue_lower_bound,
                direct_synthesis_max_error=direct_error, direct_adjoint_max_error=adjoint_error,
                unconstrained=dict(convergence=convergence, mse=optimum_mse,
                                   rms_relative_error=math.sqrt(optimum_mse), signed_center_response=predicted,
                                   center_error_db=20 * math.log10(abs(predicted) / center_target)),
                inverse_constraint_convergence=direction_convergence,
                constraint_representer_norm_squared=denominator, branches=branches,
                constrained_rms_lower_bound=math.sqrt(max(0, lower_bound)),
                baseline_rms_nonregression_impossible=lower_bound > baseline_mse,
                best_constrained_rms=best["rms_relative_error"],
                best_constrained_rms_ratio_to_baseline=best["rms_relative_error"] / baseline_metrics["rms_relative_error"])


def main():
    result = dict(production_modified=False, all_original_gates_preserved=True,
                  scope="Single natural configured band-center constraint versus frozen uniform-grid RMS only; no four-probe infeasibility claim",
                  proof_status="Floating-point numerical dual certificate with residual allowance, not interval arithmetic",
                  objective="mean((B*x/target - 1)^2), 4097 uniformly spaced full-spectrum points including endpoints",
                  rows=[analyze(rate) for rate in [48000, 96000]])
    (ART / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({**{k:v for k,v in result.items() if k != "rows"}, "rows":[{k:v for k,v in row.items() if k not in ["branches"]} for row in result["rows"]]},indent=2))


if __name__ == "__main__":
    main()
