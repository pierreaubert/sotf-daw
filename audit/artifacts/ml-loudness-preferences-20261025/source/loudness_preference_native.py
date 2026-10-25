"""Independent analog/bilinear curve and actual native impulse qualification."""

import json
import subprocess
from pathlib import Path

import numpy as np
from scipy.signal import bilinear, sosfreqz


def manual_sos(params, rate):
    sections = []
    for kind, frequency, gain, q in [
        ("low", params["low_freq"], params["low_gain"] / 2, None),
        ("low", params["low_freq"], params["low_gain"] / 2, None),
        ("peak", params["mid_freq"], params["mid_gain"], params["mid_q"]),
        ("high", params["high_freq"], params["high_gain"] / 2, None),
        ("high", params["high_freq"], params["high_gain"] / 2, None),
    ]:
        w, a = 2 * rate * np.tan(np.pi * frequency / rate), 10 ** (gain / 40)
        if kind == "low":
            b, d = (
                [a, a * np.sqrt(2 * a) * w, a * a * w * w],
                [a, np.sqrt(2 * a) * w, w * w],
            )
        elif kind == "high":
            b, d = (
                [a * a, a * np.sqrt(2 * a) * w, a * w * w],
                [1, np.sqrt(2 * a) * w, a * w * w],
            )
        else:
            b, d = [1, w * a / q, w * w], [1, w / (a * q), w * w]
        numerator, denominator = bilinear(b, d, fs=rate)
        sections.append(np.r_[numerator, denominator])
    return np.asarray(sections)


def normalized_response(params, rate, frequencies):
    sos = manual_sos(params, rate)
    if any(np.any(np.abs(np.roots(row[3:])) >= 1) for row in sos):
        raise ValueError("Unstable independently designed manual filters")
    native_grid = np.geomspace(20, rate * 0.499, 128)
    _, raw = sosfreqz(sos, worN=native_grid, fs=rate)
    attenuation = max(0.0, float(np.max(20 * np.log10(np.abs(raw)))))
    _, response = sosfreqz(sos, worN=frequencies, fs=rate)
    return response * 10 ** (-attenuation / 20), attenuation


def qualify(params, rate, renderer, work, callback_frames=193):
    work = Path(work)
    work.mkdir(parents=True, exist_ok=False)
    output = work / "impulse.f32le"
    request = {
        "sample_rate": rate,
        "callback_frames": callback_frames,
        "params": params,
        "output": str(output.resolve()),
    }
    path = work / "request.json"
    path.write_text(json.dumps(request, allow_nan=False) + "\n")
    process = subprocess.run(
        [str(Path(renderer).resolve()), str(path)],
        capture_output=True,
        text=True,
        check=False,
        timeout=120,
    )
    if process.returncode != 0:
        raise ValueError("Native manual curve render failed: " + process.stderr)
    metadata = json.loads(process.stdout)
    raw = np.fromfile(output, dtype="<f4")
    if (
        metadata["status"] != "ok"
        or metadata["sample_rate"] != rate
        or metadata["callback_frames"] != callback_frames
        or metadata["channels"] != 2
        or metadata["frames"] != 131072
        or metadata["latency_samples"] != 0
        or raw.size != 262144
        or not np.isfinite(raw).all()
    ):
        raise ValueError("Invalid native impulse geometry or values")
    impulse = raw.reshape(-1, 2)
    if not np.array_equal(impulse[:, 0], impulse[:, 1]):
        raise ValueError("Native global manual curves disagree across channels")
    tail_peak = float(np.max(np.abs(impulse[-4096:])))
    if tail_peak > 1e-10:
        raise ValueError("Native tail has not decayed within QA record")
    frequencies = np.fft.rfftfreq(len(impulse), 1 / rate)
    selected = (frequencies >= 20) & (frequencies <= 20000)
    actual = np.fft.rfft(impulse[:, 0].astype(np.float64))[selected]
    reference, attenuation = normalized_response(params, rate, frequencies[selected])
    db = 20 * np.log10(np.maximum(np.abs(actual), 1e-15))
    complex_error = float(np.max(np.abs(actual - reference)))
    magnitude_error = float(np.max(np.abs(db - 20 * np.log10(np.abs(reference)))))
    dense = np.unique(
        np.r_[
            np.geomspace(20, 20000, 8193),
            [params[key] for key in ("low_freq", "mid_freq", "high_freq")],
        ]
    )
    independent, _ = normalized_response(params, rate, dense)
    dense_db = 20 * np.log10(np.abs(independent))
    if (
        complex_error > 1e-5
        or magnitude_error > 0.001
        or np.max(np.abs(db)) > 12
        or np.max(np.abs(dense_db)) > 12
    ):
        raise ValueError("Native/independent response or gain-bound check failed")
    report = dict(
        metadata,
        max_complex_error=complex_error,
        max_magnitude_error_db=magnitude_error,
        native_sampled_peak_db=float(db.max()),
        native_sampled_min_db=float(db.min()),
        independent_dense_abs_peak_db=float(np.max(np.abs(dense_db))),
        native_normalization_attenuation_db=attenuation,
        tail_peak=tail_peak,
        qualification="Stationary normalized manual curve only; frequency normalization is not an instantaneous peak ceiling or SPL calibration",
    )
    (work / "response-report.json").write_text(json.dumps(report, indent=2) + "\n")
    return report
