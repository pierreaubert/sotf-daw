"""Post-hoc noise-lead calibration diagnostic; no quality-driven parameter search."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from expander_mask_native import reference


def rms(x):
    return float(np.sqrt(np.mean(np.asarray(x, float) ** 2)))


def reopen(file):
    raw = Path(file["path"]).read_bytes()
    if hashlib.sha256(raw).hexdigest() != file["sha256"]:
        raise ValueError("Frozen native artifact identity changed")
    return np.frombuffer(raw, dtype="<f4")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    source = json.loads(args.report.read_text())
    report = {
        "source_report_sha256": hashlib.sha256(args.report.read_bytes()).hexdigest(),
        "scope": "Post-hoc diagnostic, not preregistered quality admission. Scalar control calibrated only on source time 50–200 ms known noise lead, then frozen for the full waveform. Classical/dry interpolation or attenuation uses actual native outputs. No SNR-based alpha selection.",
        "deployment_admitted": False,
        "cases": [],
    }
    for case in source["cases"]:
        channels, rate = case["channels"], case["sample_rate"]
        clean, noise = [
            reopen(case[name + "_file"]).reshape(-1, channels)
            for name in ["clean", "noise"]
        ]
        if not np.any(noise):
            report["cases"].append(
                {
                    "case": case["case"],
                    "calibrated": False,
                    "reason": "no noise for calibration",
                }
            )
            continue
        arrays = {}
        for mode in ["classical", "unity", "learned"]:
            metadata = case["methods"][mode]["metadata"]
            output = reopen(metadata["files"]["output"]).reshape(-1, channels)
            gains = reopen(metadata["files"]["gain_trace_output"]).reshape(
                -1, channels, 513
            )
            residual = reference(noise, metadata["analysis_window"], gains, len(output))
            desired = reference(clean, metadata["analysis_window"], gains, len(output))
            arrays[mode] = (output, desired, residual)
        begin, end = 1024 + int(0.05 * rate), 1024 + int(0.20 * rate)
        classical = arrays["classical"][2][begin:end]
        dry = arrays["unity"][2][begin:end]
        target = rms(arrays["learned"][2][begin:end]) ** 2
        classical_energy = rms(classical) ** 2
        if target <= classical_energy:
            alpha = np.sqrt(target / max(classical_energy, 1e-30))
            control = tuple(alpha * x for x in arrays["classical"])
            method = "attenuate_classical"
        else:
            direction = dry - classical
            a, b, c = (
                np.mean(direction**2),
                2 * np.mean(classical * direction),
                classical_energy - target,
            )
            roots = np.roots([a, b, c])
            valid = [
                float(x.real) for x in roots if abs(x.imag) < 1e-10 and 0 <= x.real <= 1
            ]
            if not valid:
                report["cases"].append(
                    {
                        "case": case["case"],
                        "calibrated": False,
                        "reason": "target outside classical/dry calibration range",
                    }
                )
                continue
            alpha = min(valid)
            control = tuple(
                (1 - alpha) * x + alpha * y
                for x, y in zip(arrays["classical"], arrays["unity"], strict=True)
            )
            method = "classical_dry_interpolation"
        output, desired, residual = control
        reference_clean = np.zeros_like(output)
        reference_clean[1024 : 1024 + len(clean)] = clean
        snr = (
            float(
                20
                * np.log10(
                    rms(reference_clean) / max(rms(output - reference_clean), 1e-20)
                )
            )
            if np.any(clean)
            else None
        )
        full_noise = np.zeros_like(output)
        full_noise[1024 : 1024 + len(noise)] = noise
        calibration_error = abs(rms(residual[begin:end]) ** 2 - target)
        if calibration_error > 1e-12:
            raise RuntimeError("Noise-lead calibration failed")
        report["cases"].append(
            {
                "case": case["case"],
                "calibrated": True,
                "method": method,
                "alpha": alpha,
                "calibration_mean_square_error": calibration_error,
                "control_snr_db": snr,
                "learned_minus_control_snr_db": case["methods"]["learned"]["snr_db"]
                - snr
                if snr is not None
                else None,
                "control_clean_relative_error": rms(desired - reference_clean)
                / rms(reference_clean)
                if np.any(clean)
                else None,
                "control_full_noise_attenuation_db": float(
                    20
                    * np.log10(max(rms(full_noise), 1e-20) / max(rms(residual), 1e-20))
                ),
                "learned_full_noise_attenuation_db": case["methods"]["learned"][
                    "residual_noise_attenuation_db"
                ],
            }
        )
    with args.output.open("x") as f:
        f.write(json.dumps(report, indent=2, allow_nan=False) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
