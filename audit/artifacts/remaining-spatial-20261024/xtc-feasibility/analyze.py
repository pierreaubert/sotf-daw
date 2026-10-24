"""Independent scalar/modal feasibility of the frozen XTC LF coefficients."""

import cmath
import hashlib
import json
import math
from pathlib import Path

ROOT = Path(__file__).parent


def unpack(matrix):
    return [[complex(*entry) for entry in row] for row in matrix]


def norm(vector):
    return math.sqrt(sum(abs(value) ** 2 for value in vector))


def response(row, column):
    return sum(a * b for a, b in zip(row, column))


def optimal_unit_direction_metrics(desired, leakage, ratio, error):
    """Exact symmetric-row-ball robust-depth boundary, independent of inversion."""
    b = norm(leakage)
    a = norm(desired)
    parallel = abs(sum(x * y.conjugate() for x, y in zip(desired, leakage))) / b
    perpendicular = abs(desired[0] * leakage[1] - desired[1] * leakage[0]) / b
    q = ratio * b - parallel
    assert q > 0  # The LF case: demanded ratio exceeds row correlation ratio.
    threshold = (1 + ratio) * error
    if threshold > perpendicular:
        return {"feasible_direction": False}
    denominator = perpendicular**2 + q**2
    t = (-q * threshold + perpendicular * math.sqrt(denominator - threshold**2)) / denominator
    amplitude = parallel * t + perpendicular * math.sqrt(max(0, 1 - t*t))
    return {
        "feasible_direction": True,
        "parallel_fraction": t,
        "nominal_desired_per_unit_column": amplitude,
        "guaranteed_desired_per_unit_column": amplitude - error,
        "guaranteed_leakage_per_unit_column": b * t + error,
        "boundary_ratio": (amplitude - error) / (b*t + error),
        "pythagorean_error": abs(a*a - parallel*parallel - perpendicular*perpendicular),
    }


stage_run = json.loads((ROOT / "baseline-stages.json").read_text())
assert stage_run["exit_code"] == 0
records = [json.loads(line) for line in stage_run["stdout"].splitlines() if line.startswith('{')]
rows = [row for row in records if row.get("measurement") == "xtc_preparation_stage"]
maximum_modal_error = 0.0
for row in rows:
    if row["stage"] != "regularized_unbounded":
        continue
    plant = unpack(row["plant"])
    d, c = plant[0]
    assert plant[1] == [c, d]
    # The Hadamard eigenvectors diagonalize a complex symmetric two-ear plant.
    plus, minus = d + c, d - c
    gp = plus.conjugate() / (abs(plus)**2 + row["beta"])
    gm = minus.conjugate() / (abs(minus)**2 + row["beta"])
    oracle = [[(gp+gm)/2, (gp-gm)/2], [(gp-gm)/2, (gp+gm)/2]]
    actual = unpack(row["filters"])
    maximum_modal_error = max(maximum_modal_error, max(abs(oracle[i][j]-actual[i][j]) for i in range(2) for j in range(2)))
assert maximum_modal_error <= 1e-6

selected = {row["stage"]: row for row in rows if row["rate"] == 48000 and row["fft_size"] == 2048
            and row["head_model"] == 0 and row["itd_modeling"] == "phase_only" and row["bin"] == 4}
plant = unpack(selected["effort_limited"]["plant"])
desired, leakage = plant
ratio = 10**(5/20)  # The unchanged declared nominal LF threshold.
perpendicular = abs(desired[0]*leakage[1] - desired[1]*leakage[0]) / norm(leakage)
critical_error = perpendicular / (1+ratio)
equal_row_radius = norm([a-b for a,b in zip(desired, leakage)]) / 2
gain_floor = 10**(selected["effort_limited"]["desired_gain_db"][0]/20)
ceiling = selected["effort_limited"]["ceiling"]

# A caller-explicit controlled point-source field, not a human/room calibration.
frequency = 93.75
k = math.tau*frequency/343
a = 0.0875
near = math.hypot(1-a, math.sqrt(3))
far = math.hypot(1+a, math.sqrt(3))
point_row = [2/near, 2/far*cmath.exp(-1j*k*(far-near))]
model_offset = norm([x-y for x,y in zip(point_row, desired)])
regions = []
for name, lateral, yaw, echo in [("small_explicit_region", .005, math.pi/180, 0), ("wide_explicit_region", .1, .3, .4)]:
    displacement = lateral + 2*a*math.sin(yaw/2)
    minimum_range = near-displacement
    image_range = math.hypot(1-a, math.sqrt(3)+2)-displacement
    # |d(2 exp(-ikr)/r)/dr| <= 2/r^2 + 2k/r. The image term is bounded
    # in amplitude, so this covers every pose/echo in the declared region.
    direct_bound = (2/minimum_range**2+2*k/minimum_range)*displacement
    reflection_bound = 2*echo/image_range
    row_error = model_offset + math.sqrt(2)*(direct_bound+reflection_bound)
    metrics = optimal_unit_direction_metrics(desired, leakage, ratio, row_error)
    if metrics["feasible_direction"]:
        metrics["maximum_guaranteed_desired_with_row_budget"] = ceiling*metrics["guaranteed_desired_per_unit_column"]
        metrics["baseline_nominal_gain_floor_feasible"] = metrics["maximum_guaranteed_desired_with_row_budget"] >= gain_floor
        assert abs(metrics["boundary_ratio"]-ratio) <= 1e-12
    maximum_residual = (abs(leakage[0]) - row_error) / ratio
    raw_metrics = {"feasible_direction": False}
    if maximum_residual > 0:
        combined_ratio = max(ratio, gain_floor / maximum_residual)
        raw_metrics = optimal_unit_direction_metrics(desired, leakage, combined_ratio, row_error)
        if raw_metrics["feasible_direction"]:
            scale = gain_floor / raw_metrics["guaranteed_desired_per_unit_column"]
            raw_metrics["required_row_power"] = scale**2
            raw_metrics["required_nominal_desired_amplitude"] = scale*raw_metrics["nominal_desired_per_unit_column"]
            raw_metrics["gain_and_power_feasible"] = scale**2 <= ceiling**2 and raw_metrics["required_nominal_desired_amplitude"] <= 1.25
        raw_metrics["effective_required_absolute_ratio"] = combined_ratio
    regions.append({"name":name,"lateral_radius_m":lateral,"yaw_limit_rad":yaw,"echo_max":echo,
        "model_offset_from_declared_point_field":model_offset,"continuous_row_error_radius":row_error,
        "contains_equal_ear_rows_in_overapproximation":row_error >= equal_row_radius,
        "absolute_separation_only":metrics,"conservative_independent_amplitude_raw_certificate":raw_metrics})

result = {
    "baseline_stage_artifact_sha256": hashlib.sha256((ROOT/"baseline-stages.json").read_bytes()).hexdigest(),
    "modal_reference_cases":sum(row["stage"]=="regularized_unbounded" for row in rows),
    "maximum_modal_coefficient_error":maximum_modal_error,
    "selected_frequency_hz":frequency,
    "stages":{name:{key:row[key] for key in ["raw_depth_db","desired_gain_db","row_powers"]} for name,row in selected.items()},
    "existing_row_power_ceiling":ceiling**2,
    "baseline_nominal_desired_gain":gain_floor,
    "maximum_equal_row_L2_error_radius_for_5dB_absolute_separation_any_direction":critical_error,
    "equal_ear_rank_one_witness_radius":equal_row_radius,
    "regions":regions,
    "metric_correction":"The retained nominal5dB test is raw residual attenuation relative to passthrough. The initial feasibility report described absolute desired/undesired separation only; it did not certify that retained raw gate. Both constraints are separate here, with unchanged original physical regions and gain/power bounds.",
    "raw_certificate_scope":"The independent numerator/denominator amplitude comparison is sufficient but conservative; it discards the same-row ratio correlation. Exact correlated raw-ratio feasibility is separately tested in raw-ratio-reference.json and final-reference.json. No conservative failure here establishes physical or original-contract impossibility.",
    "scope":"Exact scalar row-ball geometry and continuous controlled point-field bound; overapproximation infeasibility is not physical-set impossibility or measured provenance.",
}
(ROOT/"analysis.json").write_text(json.dumps(result,indent=2)+"\n")
print(json.dumps(result,indent=2))
