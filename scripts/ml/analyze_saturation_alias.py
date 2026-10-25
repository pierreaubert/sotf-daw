"""Independent continuous-tanh Fourier check of native folded-harmonic probes."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np


def reference(drive, fundamental, period=4096, points=65536):
    phase = 2 * np.pi * np.arange(points) / points
    curve = np.tanh(0.7 * drive * np.sin(phase)) / np.tanh(drive)
    # Independently integrate the continuous curve's sine-series coefficients.
    # This is not a sampled base-rate or learned-model waveform reference.
    coefficients = {
        h: float(2 * np.mean(curve * np.sin(h * phase))) for h in range(1, 32, 2)
    }
    folded = {}
    for harmonic, coefficient in coefficients.items():
        if harmonic * fundamental <= period / 2:
            continue
        signed = (harmonic * fundamental + period // 2) % period - period // 2
        bin = abs(signed)
        if bin == fundamental:
            continue
        folded[bin] = folded.get(bin, 0) + np.sign(signed) * coefficient
    ratio = sum(x * x for x in folded.values()) / coefficients[1] ** 2
    return float(10 * np.log10(max(ratio, 1e-30))), sorted(folded)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--native-log", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    raw = args.native_log.read_bytes()
    line = next(
        x[x.index("{") :]
        for x in raw.decode().splitlines()
        if '{"alias_parameter_sweep":' in x
    )
    data = json.loads(line)
    results = []
    for case in data["alias_parameter_sweep"]:
        expected, bins = reference(case["drive"], case["fundamental_bin"])
        observed = case["learned_folded_to_fundamental_db"]
        error = abs(observed - expected)
        if bins != sorted(case["alias_bins"]) or error > 0.15:
            raise RuntimeError(
                "Native analytic fixture disagrees with independent continuous-curve Fourier probe"
            )
        results.append(
            dict(
                case,
                continuous_tanh_folded_probe_db=expected,
                native_reference_error_db=error,
                learned_minus_native_adaa_probe_db=observed
                - case["native_adaa_folded_to_fundamental_db"],
            )
        )
    report = {
        "native_log_sha256": hashlib.sha256(raw).hexdigest(),
        "cases": results,
        "deployment_admitted": False,
        "scope": "Untrained analytic tanh fixtures at48kHz. Independent65536-point continuous-curve quadrature checks selected folded odd harmonics3..31, not total alias power. No measured hardware, oversampled trained-model or perceptual admission.",
    }
    with args.output.open("x") as f:
        f.write(json.dumps(report, indent=2, allow_nan=False) + "\n")
    print(
        json.dumps(
            {
                "cases": len(results),
                "maximum_native_reference_error_db": max(
                    x["native_reference_error_db"] for x in results
                ),
                "deployment_admitted": False,
            }
        )
    )


if __name__ == "__main__":
    main()
