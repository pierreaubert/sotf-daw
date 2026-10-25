"""Independent FIR DTFT/Tustin checks and native preset reopening for fixed probes."""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np
from scipy.fft import rfft
from scipy.io import wavfile
from scipy.signal import bilinear, sosfreqz


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def dtft(fir, frequencies, rate):
    result = []
    index = np.arange(len(fir))
    for first in range(0, len(frequencies), 32):
        z = np.exp(-2j * np.pi * frequencies[first : first + 32, None] * index / rate)
        result.extend(z @ fir)
    return np.asarray(result)


def reference(params, frequencies, rate):
    sections = []
    for band in params["filters"]:
        cutoff = np.tan(np.pi * band["frequency"] / rate)
        a = 10 ** (band["gain_db"] / 40)
        if band["filter_type"] == "Peak":
            b, denominator = (
                [1 / cutoff**2, a / (band["q"] * cutoff), 1],
                [1 / cutoff**2, 1 / (a * band["q"] * cutoff), 1],
            )
        elif band["filter_type"] == "Lowshelf":
            b, denominator = (
                [a / cutoff**2, a * np.sqrt(2 * a) / cutoff, a * a],
                [a / cutoff**2, np.sqrt(2 * a) / cutoff, 1],
            )
        else:
            b, denominator = (
                [a * a / cutoff**2, a * np.sqrt(2 * a) / cutoff, a],
                [1 / cutoff**2, np.sqrt(2 * a) / cutoff, a],
            )
        numerator, denominator = bilinear(b, denominator, fs=0.5)
        sections.append(np.r_[numerator, denominator])
    return (
        sosfreqz(sections, worN=frequencies, fs=rate)[1]
        if sections
        else np.ones(len(frequencies), dtype=complex)
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--temporary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists() or args.temporary.exists():
        raise ValueError("preserve prior analysis")
    args.temporary.mkdir(parents=True)
    protocol = json.loads((args.evidence / "protocol.json").read_text())
    for name, expected in protocol["source_sha256"].items():
        if digest(args.evidence / "evaluation-source" / name) != expected:
            raise ValueError("frozen source identity mismatch")
    if digest(args.renderer) != protocol["renderer_sha256"]:
        raise ValueError("renderer identity mismatch")
    rows = []
    for index in range(len(protocol["cases"])):
        case = json.loads((args.evidence / f"case-{index:03d}.json").read_text())
        report, directory = case["report"], Path(case["directory"])
        target_path = args.evidence / (case["case"] + "-target.json")
        if digest(target_path) != case["target_sha256"]:
            raise ValueError("target identity mismatch")
        if not report["accepted"]:
            if (
                report["parameters"] is not None
                or (directory / "output/preset.json").exists()
                or (directory / "output/fir.wav").exists()
            ):
                raise ValueError("rejected case published an export")
            rows.append(
                {"case": case["case"], "accepted": False, "error": report.get("error")}
            )
            continue
        target = json.loads(target_path.read_text())
        rate, fir = wavfile.read(directory / "output/fir.wav")
        params = json.loads((directory / "output/preset.json").read_text())
        native = report["native"]
        raw_fir = Path(native["directory"]) / "fir.f32le"
        if (
            params != report["parameters"]
            or digest(directory / "output/fir.wav") != report["fir_wav_sha256"]
            or digest(raw_fir) != report["checks"]["fir_sha256"]
            or not np.array_equal(fir, np.fromfile(raw_fir, dtype="<f4"))
        ):
            raise ValueError("FIR/preset identity mismatch")
        fir = fir.astype(float)
        frequencies = np.asarray(target["frequencies_hz"])
        h = dtft(fir, frequencies, rate)
        residual = 20 * np.log10(np.maximum(np.abs(h), 1e-30)) - target["target_db"]
        rms, maximum = (
            float(np.sqrt(np.mean(residual**2))),
            float(np.max(np.abs(residual))),
        )
        if (
            rms > 0.1
            or maximum > 0.5
            or abs(rms - report["checks"]["target_rms_db"]) > 1e-7
            or abs(maximum - report["checks"]["target_max_db"]) > 1e-7
        ):
            raise ValueError("independent supplied-target qualification failed")
        length = 262144
        grid = np.arange(length // 2 + 1) * rate / length
        selected = (grid >= 20) & (grid <= 20000)
        grid = grid[selected]
        spectrum = rfft(fir, n=length)[selected]
        ideal = np.abs(reference(params, grid, rate))
        error = float(
            np.max(
                np.abs(
                    20
                    * np.log10(
                        np.maximum(np.abs(spectrum), 1e-30) / np.maximum(ideal, 1e-30)
                    )
                )
            )
        )
        phase = float(
            np.max(
                np.abs(
                    np.angle(
                        spectrum * np.exp(2j * np.pi * grid * (len(fir) / 2) / rate),
                        deg=True,
                    )
                )
            )
        )
        if error > 0.0501 or phase > 0.001:
            raise ValueError("independent dense response/phase qualification failed")
        request = {
            "sample_rate": rate,
            "params": params,
            "output_dir": str((args.temporary / f"native-{index}").resolve()),
            "frequencies_hz": frequencies[::8].tolist(),
            "callback_frames": 511,
        }
        request_path = args.temporary / f"request-{index}.json"
        request_path.write_text(json.dumps(request))
        reopened = subprocess.run(
            [str(args.renderer.resolve()), str(request_path.resolve())],
            check=True,
            capture_output=True,
            text=True,
        )
        reopened = json.loads(reopened.stdout)
        if (
            digest(Path(reopened["directory"]) / "fir.f32le")
            != report["checks"]["fir_sha256"]
            or reopened["latency_samples"] != report["latency_samples"]
        ):
            raise ValueError("saved preset did not reopen identically")
        impulse_path = Path(reopened["directory"]) / "impulse.f32le"
        impulse = np.fromfile(impulse_path, dtype="<f4").astype(float)
        impulse_error = float(np.max(np.abs(impulse[32 : 32 + len(fir)] - fir)))
        if impulse_error > 1e-6 * max(1.0, float(np.max(np.abs(fir)))):
            raise ValueError("reopened streaming kernel mismatch")
        if report["latency_samples"] != len(fir) // 2 + 32 or report[
            "added_latency_samples"
        ] != max(0, len(fir) // 2 - 512):
            raise ValueError("latency report mismatch")
        rows.append(
            {
                "case": case["case"],
                "accepted": True,
                "fit_method": report["fit_method"],
                "fir_taps": len(fir),
                "latency_samples": report["latency_samples"],
                "added_latency_samples": report["added_latency_samples"],
                "independent_target_rms_db": rms,
                "independent_target_max_db": maximum,
                "uniform_fft_tustin_max_error_db": error,
                "phase_residual_degrees": phase,
                "reopened_impulse_error": impulse_error,
            }
        )
    report = {
        "cases": len(rows),
        "qualified": sum(row["accepted"] for row in rows),
        "rows": rows,
        "scope": "Independent direct FIR DTFT, dense uniform SciPy FFT versus Tustin polynomial conversion, and native preset reopening. Fixed synthetic functional probes, not learned-superiority or measured-target acceptance.",
    }
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report))


if __name__ == "__main__":
    main()
