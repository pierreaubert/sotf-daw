#!/usr/bin/env python3
"""Characterize a prepared upmixer with deterministic offline signals.

Requires NumPy and SciPy. The renderer is the `surround_characterization` example.
Results describe these stimuli, not listener preference or an LTI transfer function.
"""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

import numpy as np
from scipy import signal


INPUT_ROLES = {
    "5.0": ["FL", "FR", "C", "SL", "SR"],
    "5.1": ["FL", "FR", "C", "LFE", "SL", "SR"],
    "7.1": ["FL", "FR", "C", "LFE", "SL", "SR", "BL", "BR"],
}
OUTPUT_ROLES = {
    "5.1.4": ["FL", "FR", "C", "LFE", "SL", "SR", "TFL", "TFR", "TBL", "TBR"],
    "7.1.4": ["FL", "FR", "C", "LFE", "SL", "SR", "BL", "BR", "TFL", "TFR", "TBL", "TBR"],
}
FFT = 4096
OVERLAP = 2048


def spectra(source, height, sample_rate):
    options = dict(fs=sample_rate, window="hann", nperseg=FFT, noverlap=OVERLAP,
                   detrend=False, scaling="density")
    frequency, pxx = signal.welch(source, **options)
    _, pyy = signal.welch(height, **options)
    _, pxy = signal.csd(source, height, **options)
    coherence = np.divide(np.abs(pxy) ** 2, pxx * pyy,
                          out=np.zeros_like(pxx), where=(pxx * pyy) > 1e-30)
    return frequency, pxx, pyy, pxy, np.clip(coherence, 0.0, 1.0)


def validate_spectral_estimator():
    rate = 48_000
    rng = np.random.default_rng(20261117)
    source = rng.standard_normal(rate * 4)
    height = np.concatenate([np.zeros(128), source[:-128]]) * 0.5
    frequency, pxx, pyy, pxy, coherence = spectra(source, height, rate)
    assert abs(np.sum(pxx) * (frequency[1] - frequency[0]) / np.mean(source**2) - 1.0) < 0.02
    assert abs(np.sum(pyy) / np.sum(pxx) - 0.25) < 0.01
    for target in [250.0, 1000.0, 4000.0]:
        index = int(np.argmin(np.abs(frequency - target)))
        expected = -2.0 * np.pi * frequency[index] * 128 / rate
        error = np.angle(np.exp(1j * (np.angle(pxy[index]) - expected)))
        assert coherence[index] > 0.95 and abs(error) < 0.08


def render(executable, directory, input_layout, output_layout, rate, bypass, samples):
    input_path = directory / "input.f32le"
    output_path = directory / "output.f32le"
    np.asarray(samples, dtype="<f4").tofile(input_path)
    environment = dict(os.environ, RUST_LOG="error")
    result = subprocess.run(
        [str(executable), input_layout, output_layout, str(rate), str(bypass).lower(),
         str(input_path), str(output_path)],
        check=True, capture_output=True, text=True, env=environment,
    )
    metadata = json.loads(result.stdout.strip().splitlines()[-1])
    output = np.fromfile(output_path, dtype="<f4").reshape(-1, len(OUTPUT_ROLES[output_layout]))
    assert output.shape[0] == len(samples) and np.isfinite(output).all()
    latency = metadata["latency_frames"]
    assert latency > 0
    for source, role in enumerate(INPUT_ROLES[input_layout]):
        target = OUTPUT_ROLES[output_layout].index(role)
        assert np.array_equal(output[latency:, target], samples[:-latency, source]), role
    return output, metadata


def db_ratio(numerator, denominator):
    return float(10.0 * np.log10(numerator / denominator))


def noise_measurements(output, roles, rate):
    start = int(rate * 0.8)
    output = output[start:].astype(np.float64)
    result = {}
    for role, source_role in [("TFL", "FL"), ("TFR", "FR"), ("TBL", "SL"), ("TBR", "SR")]:
        height = output[:, roles.index(role)]
        source = output[:, roles.index(source_role)]
        if source_role in ("SL", "SR") and "BL" in roles:
            back = "BL" if source_role == "SL" else "BR"
            source = source / 3.0 + output[:, roles.index(back)] * (2.0 / 3.0)
            source_role = f"{source_role}/3 + 2*{back}/3"
        frequency, pxx, pyy, pxy, coherence = spectra(source, height, rate)
        assert np.mean(height**2) > 0 and np.mean(source**2) > 0
        bands = {}
        for low, high in [(125, 500), (500, 2000), (2000, 8000)]:
            keep = (frequency >= low) & (frequency < high)
            bands[f"{low}-{high}Hz"] = db_ratio(np.sum(pyy[keep]), np.sum(pxx[keep]))
        phase = {}
        for target in [250, 1000, 4000]:
            index = int(np.argmin(np.abs(frequency - target)))
            phase[str(target)] = {
                "bin_hz": float(frequency[index]),
                "coherence": float(coherence[index]),
                "phase_deg": float(np.degrees(np.angle(pxy[index]))) if coherence[index] >= 0.2 else None,
            }
        result[role] = {
            "source_role": source_role,
            "height_rms_dbfs": db_ratio(np.mean(height**2), 1.0),
            "height_to_source_rms_db": db_ratio(np.mean(height**2), np.mean(source**2)),
            "height_sample_peak": float(np.max(np.abs(height))),
            "zero_lag_source_correlation": float(np.corrcoef(source, height)[0, 1]),
            "band_power_ratio_db": bands,
            "cross_spectral_phase": phase,
        }
    for left, right in [("TFL", "TFR"), ("TBL", "TBR")]:
        result[f"{left}_{right}_zero_lag_correlation"] = float(
            np.corrcoef(output[:, roles.index(left)], output[:, roles.index(right)])[0, 1]
        )
    return result


def transient_measurements(output, roles, rate, latency, burst_start, burst_length):
    result = {}
    for role in ["TFL", "TFR"]:
        energy = output[:, roles.index(role)].astype(np.float64) ** 2
        total = float(np.sum(energy))
        assert total > 1e-20, f"{role}: no transient height energy"
        cumulative = np.cumsum(energy) / total
        reference = burst_start + latency
        percentiles = {
            str(percent): float((np.searchsorted(cumulative, percent / 100) - reference) * 1000 / rate)
            for percent in [5, 50, 95]
        }
        result[role] = {
            "energy_percentile_ms_relative_to_delayed_burst_start": percentiles,
            "energy_fraction_before_delayed_burst": float(np.sum(energy[:reference]) / total),
            "energy_fraction_after_delayed_burst": float(np.sum(energy[reference + burst_length:]) / total),
            "height_sample_peak": float(np.max(np.abs(output[:, roles.index(role)]))),
        }
    return result


def sweep_signal(roles, rate):
    duration = 4.0
    time = np.arange(int(duration * rate), dtype=np.float64) / rate
    ratio = 20_000.0 / 20.0
    phase = 2.0 * np.pi * 20.0 * duration / np.log(ratio) * np.expm1(np.log(ratio) * time / duration)
    window = signal.windows.tukey(len(time), alpha=0.05)
    result = np.zeros((len(time), len(roles)), dtype=np.float32)
    for index, role in enumerate(roles):
        if role not in ("C", "LFE"):
            result[:, index] = 0.1 * window * np.sin(phase + index * np.pi / 3.0)
    return np.concatenate([result, np.zeros((rate // 4, len(roles)), dtype=np.float32)])


def sweep_measurements(output, roles, rate, latency):
    sweep_frames = 4 * rate
    assert len(output) >= latency + sweep_frames
    source_time = np.arange(sweep_frames, dtype=np.float64) / rate
    source_frequency = 20.0 * np.exp(np.log(20_000.0 / 20.0) * source_time / 4.0)
    result = {}
    for role in ["TFL", "TFR", "TBL", "TBR"]:
        height = output[latency:latency + sweep_frames, roles.index(role)].astype(np.float64)
        bands = {}
        for low, high in [(20, 100), (100, 1000), (1000, 10_000), (10_000, 20_000)]:
            selected = height[(source_frequency >= low) & (source_frequency < high)]
            assert selected.size > 0
            bands[f"{low}-{high}Hz"] = float(np.sqrt(np.mean(selected**2)))
        peak = float(np.max(np.abs(height)))
        assert peak > 0.0 and np.isfinite(peak)
        result[role] = {"sample_peak": peak, "rms_by_source_frequency": bands}
    return result


def main():
    if len(sys.argv) != 3:
        raise SystemExit("usage: characterize_surround.py RENDERER OUTPUT_JSON")
    executable = Path(sys.argv[1]).resolve()
    target = Path(sys.argv[2]).resolve()
    target.parent.mkdir(parents=True, exist_ok=True)
    validate_spectral_estimator()
    results = []
    with tempfile.TemporaryDirectory(dir=target.parent) as temporary:
        directory = Path(temporary)
        for input_layout, output_layout in [("5.0", "5.1.4"), ("5.1", "5.1.4"), ("7.1", "7.1.4")]:
            roles = INPUT_ROLES[input_layout]
            output_roles = OUTPUT_ROLES[output_layout]
            for rate in [48_000, 96_000]:
                rng = np.random.default_rng(20261117)
                noise = np.zeros((4 * rate, len(roles)), dtype=np.float32)
                sos = signal.butter(4, [100, 12_000], btype="bandpass", fs=rate, output="sos")
                for role in roles:
                    if role not in ("C", "LFE"):
                        values = signal.sosfilt(sos, rng.standard_normal(4 * rate))
                        noise[:, roles.index(role)] = values * (0.05 / np.sqrt(np.mean(values**2)))
                burst = np.zeros((2 * rate, len(roles)), dtype=np.float32)
                burst_start, burst_length = rate // 2, rate // 100
                window = np.hanning(burst_length)
                burst_rng = np.random.default_rng(20261118)
                for role in ["FL", "FR"]:
                    burst[burst_start:burst_start + burst_length, roles.index(role)] = (
                        burst_rng.standard_normal(burst_length) * window * 0.08
                    )
                sweep = sweep_signal(roles, rate)
                for bypass in [False, True]:
                    rendered, metadata = render(executable, directory, input_layout, output_layout, rate, bypass, noise)
                    metrics = noise_measurements(rendered, output_roles, rate)
                    rendered_burst, burst_metadata = render(executable, directory, input_layout, output_layout, rate, bypass, burst)
                    assert metadata == {**burst_metadata, "frames": metadata["frames"]}
                    transient = transient_measurements(rendered_burst, output_roles, rate,
                                                       metadata["latency_frames"], burst_start, burst_length)
                    rendered_sweep, sweep_metadata = render(executable, directory, input_layout, output_layout, rate, bypass, sweep)
                    assert metadata == {**sweep_metadata, "frames": metadata["frames"]}
                    sweep_metrics = sweep_measurements(rendered_sweep, output_roles, rate,
                                                       metadata["latency_frames"])
                    results.append({"metadata": metadata, "noise": metrics, "transient": transient,
                                    "sweep": sweep_metrics})
    document = {
        "method": {
            "noise": "4 seconds of independent seeded 100-12000 Hz Butterworth band-pass noise per main channel (N=4, eighth order overall), 0.05 RMS; center and LFE silent; first 0.8 seconds excluded",
            "transient": "10 ms Hann-shaped independent noise burst in FL and FR after 0.5 seconds of silence; energy times referenced to input burst onset plus reported plugin latency",
            "sweep": "4-second 20-20000 Hz logarithmic sweep at 0.1 peak per main channel, with role-dependent phases, a 0.05 Tukey window, and 0.25 seconds of trailing silence; center and LFE silent. Height RMS is grouped by the instantaneous source frequency after latency alignment, not a transfer function.",
            "spectral_estimator": "SciPy Welch and CSD, 4096-sample periodic Hann window, 2048 overlap, density scaling, no detrending; phase is CSD(source,height), shown only at coherence >= 0.2",
            "interpretation": "Adaptive nonlinear processing: frequency ratios and phase are stimulus-dependent empirical measurements, not a fixed transfer function or a listening result.",
            "numpy": np.__version__, "scipy": __import__("scipy").__version__,
        },
        "results": results,
    }
    target.write_text(json.dumps(document, indent=2, allow_nan=False) + "\n")
    print(f"Wrote {len(results)} configurations to {target}")


if __name__ == "__main__":
    main()
