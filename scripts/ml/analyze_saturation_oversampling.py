"""Full-spectrum alias diagnostics for clock-correct native neural oversampling."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np


def harmonic_spectrum(drive, points=65536):
    phase = 2 * np.pi * np.arange(points) / points
    return np.fft.fft(np.tanh(0.7 * drive * np.sin(phase)) / np.tanh(drive)) / points


def ideal_fold(drive, fundamental, factor=1, period=4096, points=65536):
    coefficients = harmonic_spectrum(drive, points)
    harmonic = np.r_[np.arange(points // 2), np.arange(-points // 2, 0)]
    internal = np.zeros(period * factor, dtype=complex)
    np.add.at(internal, (harmonic * fundamental) % (period * factor), coefficients)
    # Exact ideal band limitation after sampling the continuous curve at the
    # internal clock. Aliases already folded into the base passband remain.
    bins = np.arange(period * factor)
    signed = np.where(bins <= period * factor // 2, bins, bins - period * factor)
    keep = abs(signed) <= period // 2
    output = np.zeros(period, dtype=complex)
    np.add.at(output, signed[keep] % period, internal[keep])
    return abs(output) ** 2


def ratio(power, fundamental):
    n = len(power)
    physical = np.minimum(np.arange(n), n - np.arange(n))
    legitimate = (physical % fundamental) == 0
    alias = power[~legitimate].sum()
    return float(
        10
        * np.log10(
            max(float(alias / (power[fundamental] + power[-fundamental])), 1e-30)
        )
    )


def extract(raw, key):
    line = next(
        x[x.index("{") :] for x in raw.decode().splitlines() if '"' + key + '":' in x
    )
    return json.loads(line)[key]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--native-log", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    raw = args.native_log.read_bytes()
    direct = extract(raw, "alias_parameter_sweep")
    wrapped = extract(raw, "oversampled_alias_parameter_sweep")
    direct_by_setting = {(x["drive"], x["fundamental_bin"]): x for x in direct}
    baseline = []
    for case in direct:
        power = ideal_fold(case["drive"], case["fundamental_bin"])
        coarser = ideal_fold(case["drive"], case["fundamental_bin"], points=32768)
        if np.max(abs(power - coarser)) > 1e-10:
            raise RuntimeError("Continuous-curve folded reference did not converge")
        expected = ratio(power, case["fundamental_bin"])
        error = abs(expected - case["learned_total_nonharmonic_to_fundamental_db"])
        if error > 0.15:
            raise RuntimeError(
                "Direct native full-spectrum probe disagrees with converged reference"
            )
        baseline.append(
            dict(
                case,
                independent_total_nonharmonic_db=expected,
                reference_error_db=error,
            )
        )
    cases = []
    for case in wrapped:
        drive, bin = case["drive"], case["fundamental_bin"]
        if (
            case["capture_sample_rate"] != case["sample_rate"] * case["factor"]
            or case["added_model_lookahead_samples"] != 0
        ):
            raise RuntimeError("Candidate capture clock or latency metadata mismatch")
        ideal = ideal_fold(drive, bin, case["factor"])
        coarser = ideal_fold(drive, bin, case["factor"], points=32768)
        if np.max(abs(ideal - coarser)) > 1e-10:
            raise RuntimeError("Oversampled ideal reference did not converge")
        expected = ratio(ideal, bin)
        original = direct_by_setting[(drive, bin)]
        analytic_fundamental = abs(harmonic_spectrum(drive)[1]) ** 2
        cases.append(
            dict(
                case,
                ideal_internal_sampling_and_lowpass_nonharmonic_db=expected,
                actual_minus_ideal_nonharmonic_db=case[
                    "total_nonharmonic_to_fundamental_db"
                ]
                - expected,
                reduction_from_direct_neural_db=original[
                    "learned_total_nonharmonic_to_fundamental_db"
                ]
                - case["total_nonharmonic_to_fundamental_db"],
                difference_from_base_native_adaa_db=case[
                    "total_nonharmonic_to_fundamental_db"
                ]
                - original["native_adaa_total_nonharmonic_to_fundamental_db"],
                fundamental_power_error_db=float(
                    10 * np.log10(case["fundamental_power"] / analytic_fundamental)
                ),
            )
        )
    report = {
        "native_log_sha256": hashlib.sha256(raw).hexdigest(),
        "direct": baseline,
        "oversampled": cases,
        "deployment_admitted": False,
        "scope": "Untrained memoryless analytic fixtures only. Native full nonharmonic spectrum excludes DC and legitimate in-band harmonic bins. Independent continuous-curve65536/32768-point convergence and ideal internal-sampling/lowpass references include harmonics beyond31. Actual FFT resampler filters are not ideal. No measured-hardware, finite-drain, perceptual or production admission.",
    }
    with args.output.open("x") as f:
        f.write(json.dumps(report, indent=2, allow_nan=False) + "\n")
    print(
        json.dumps(
            {
                "direct_cases": len(baseline),
                "oversampled_cases": len(cases),
                "latencies": sorted({x["latency_samples"] for x in cases}),
                "deployment_admitted": False,
            }
        )
    )


if __name__ == "__main__":
    main()
