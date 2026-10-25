"""Offline, opt-in learned IR tail denoising with response checks and exact fallback.

Inference never runs in the convolution callback. The early response and noise
prefix are protected, with no trimming, resampling, delay or normalization. A
linked time-domain mask is applied to every channel. Validation may reject it.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from ir_preservation import load_policy, preserve_tail
from scipy.io import wavfile

FORMAT = "sotf_ir_tail_mask_v1"
FEATURES = 8
HIDDEN = 24
MAX_FRAMES = 1440000
MAX_SAMPLES = 8000000


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def validate_input(samples, rate, arrival, noise_frames):
    samples = np.asarray(samples, dtype=float)
    if samples.ndim == 1:
        samples = samples[:, None]
    if (
        samples.ndim != 2
        or not 1 <= samples.shape[1] <= 32
        or not 8000 <= rate <= 96000
        or not 0 < len(samples) <= min(MAX_FRAMES, rate * 30)
        or samples.size > MAX_SAMPLES
        or not np.isfinite(samples).all()
        or np.max(np.abs(samples)) > 16
        or not rate // 100 <= noise_frames <= arrival < len(samples)
        or arrival + rate // 10 >= len(samples)
    ):
        raise ValueError("invalid bounded IR geometry, level, arrival or noise prefix")
    return samples


def analyze(samples, rate, arrival, noise_frames):
    samples = validate_input(samples, rate, arrival, noise_frames)
    prefix = samples[:noise_frames]
    noise = float(np.mean(prefix**2))
    # A noisy or zero noise reference must not authorize learned changes.
    split = len(prefix) // 2
    powers = [float(np.mean(part**2)) for part in [prefix[:split], prefix[split:]]]
    if noise < 1e-18 or min(powers) <= 0 or max(powers) / min(powers) > 2:
        raise ValueError("noise reference is silent or nonstationary")
    hop = max(1, round(rate * 0.02))
    starts = np.arange(arrival, len(samples), hop)
    power = np.array(
        [
            np.mean(samples[start : min(start + hop, len(samples))] ** 2)
            for start in starts
        ]
    )
    ratio = np.maximum(power / noise, 1e-12)
    logarithm = np.clip(10 * np.log10(ratio), -60, 60) / 60
    padded = np.pad(logarithm, (1, 1), mode="edge")
    age = (starts - arrival) / rate
    # Fit an offline exponential power trend to samples clearly above noise.
    usable = (ratio > 4) & (age >= 0.05)
    if np.count_nonzero(usable) >= 4:
        slope, intercept = np.polyfit(
            age[usable], np.log(np.maximum(power[usable] - noise, 1e-30)), 1
        )
        slope = float(np.clip(slope, -200, 0))
        predicted = np.exp(np.clip(intercept + age * slope, -70, 30)) / noise
    else:
        slope = 0.0
        predicted = np.maximum(ratio - 1, 0)
    predicted_db = np.clip(10 * np.log10(np.maximum(predicted, 1e-6)), -60, 60) / 60
    difference = np.array(
        [
            np.mean(
                np.diff(samples[start : min(start + hop, len(samples))], axis=0) ** 2
            )
            for start in starts
        ]
    )
    color = np.clip(difference / np.maximum(power, 1e-30) / 4, 0, 1)
    features = np.column_stack(
        [
            logarithm,
            padded[:-2],
            padded[2:],
            predicted_db,
            np.clip(age / 2, 0, 1),
            np.full(len(age), slope / 200),
            color,
            np.full(len(age), np.clip(np.log10(noise) / 12, -1, 0)),
        ]
    )
    return features, starts, hop, ratio, predicted


def load_model(path, expected):
    if not expected or path.stat().st_size > 131072 or sha(path) != expected:
        raise ValueError("model identity or size mismatch")
    model = json.loads(path.read_text())
    if not isinstance(model, dict) or model.get("format") != FORMAT:
        raise ValueError("unsupported model format")
    for name, shape in [
        ("w1", (HIDDEN, FEATURES)),
        ("b1", (HIDDEN,)),
        ("w2", (HIDDEN,)),
        ("b2", ()),
    ]:
        values = np.asarray(model[name], dtype=float)
        if (
            values.shape != shape
            or not np.isfinite(values).all()
            or np.max(np.abs(values)) > 100
        ):
            raise ValueError("invalid model weights")
        model[name] = values
    return model


def neural_gains(features, model):
    hidden = np.tanh(features @ model["w1"].T + model["b1"])
    logits = hidden @ model["w2"] + model["b2"]
    return 0.05 + 0.95 / (1 + np.exp(-np.clip(logits, -60, 60)))


def apply(samples, rate, arrival, starts, gains):
    """Interpolate bounded linked gains; preserve the first 50 ms exactly."""
    protected = arrival + round(rate * 0.05)
    gains = np.asarray(gains, dtype=float).copy()
    gains[starts < protected] = 1
    mask = np.interp(np.arange(len(samples)), starts, gains, left=1, right=gains[-1])
    mask[:protected] = 1
    if (
        not np.isfinite(mask).all()
        or np.any(mask < 0.05 - 1e-12)
        or np.any(mask > 1 + 1e-12)
    ):
        raise ValueError("invalid tail mask")
    return (samples * mask[:, None]).astype("<f4"), mask


def response_checks(original, candidate, rate, arrival):
    """Compare complex finite-record responses without alignment or normalization."""
    protected = arrival + round(rate * 0.05)
    difference = candidate.astype(float) - original
    energy = float(np.sum(original**2))
    change = float(np.sqrt(np.sum(difference**2) / max(energy, 1e-30)))
    nfft = 1 << (len(original) - 1).bit_length()
    raw = np.fft.rfft(original, n=nfft, axis=0)
    changed = np.fft.rfft(candidate.astype(float), n=nfft, axis=0)
    frequencies = np.fft.rfftfreq(nfft, 1 / rate)
    # Fixed log-grid points; no peak-dependent frequency selection.
    points = np.unique(
        np.searchsorted(frequencies, np.geomspace(40, min(16000, rate * 0.45), 192))
    )
    raw, changed = raw[points], changed[points]
    energetic = np.abs(raw) >= np.max(np.abs(raw), axis=0, keepdims=True) * 0.1
    ratio = changed / np.where(np.abs(raw) > 1e-30, raw, 1)
    magnitude = float(
        np.max(np.abs(20 * np.log10(np.maximum(np.abs(ratio[energetic]), 1e-30))))
    )
    phase = float(np.max(np.abs(np.angle(ratio[energetic], deg=True))))
    early_exact = np.array_equal(
        candidate[:protected], original[:protected].astype("<f4")
    )
    accepted = bool(
        early_exact
        and energy > 1e-20
        and change <= 0.05
        and magnitude <= 1.0
        and phase <= 10.0
    )
    return {
        "accepted": accepted,
        "early_f32_exact": early_exact,
        "relative_ir_change": change,
        "max_magnitude_change_db": magnitude,
        "max_phase_change_degrees": phase,
        "sample_grid_points": len(points),
        "limits": {
            "relative_ir_change": 0.05,
            "magnitude_db": 1.0,
            "phase_degrees": 10.0,
        },
        "scope": "192 log-grid points above -20 dB per-channel peak; not a guarantee between points or at deep nulls.",
    }


def propose(
    samples,
    rate,
    arrival,
    noise_frames,
    model,
    method="neural",
    preservation_policy=None,
):
    samples = validate_input(samples, rate, arrival, noise_frames)
    features, starts, _, ratio, predicted = analyze(
        samples, rate, arrival, noise_frames
    )
    if method == "neural":
        gains = neural_gains(features, model)
    elif method == "wiener":
        smooth = np.convolve(
            np.pad(ratio, (1, 1), mode="edge"), np.ones(3) / 3, mode="valid"
        )
        gains = np.clip(1 - 1 / np.maximum(smooth, 1e-30), 0.05, 1)
    elif method == "decay":
        gains = np.clip(predicted / (predicted + 1), 0.05, 1)
    else:
        raise ValueError("unknown method")
    candidate, mask = apply(samples, rate, arrival, starts, gains)
    preservation = None
    if preservation_policy is not None:
        candidate, mask, preservation = preserve_tail(
            samples, rate, arrival, noise_frames, mask, preservation_policy
        )
    checks = response_checks(samples, candidate, rate, arrival)
    if preservation is not None:
        checks["preservation"] = preservation
        checks["accepted"] = checks["accepted"] and preservation["within_budget"]
    return candidate if checks["accepted"] else samples.astype("<f4"), checks, mask


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--model-sha256", required=True)
    parser.add_argument("--arrival-ms", type=float, required=True)
    parser.add_argument("--noise-prefix-ms", type=float, required=True)
    parser.add_argument("--preservation-policy", type=Path)
    parser.add_argument("--policy-sha256")
    args = parser.parse_args()
    if bool(args.preservation_policy) != bool(args.policy_sha256):
        raise ValueError(
            "preservation policy path and SHA-256 must be supplied together"
        )
    if (
        args.output.exists()
        or args.report.exists()
        or args.output.resolve() in [args.input.resolve(), args.report.resolve()]
    ):
        raise ValueError(
            "choose new output/report paths; original IR is never overwritten"
        )
    if args.input.stat().st_size > MAX_SAMPLES * 4 + 65536:
        raise ValueError("IR file exceeds offline input budget")
    rate, data = wavfile.read(args.input)
    if data.dtype != np.dtype("float32"):
        raise ValueError("use float32 WAV to make scaling and exact fallback explicit")
    samples = validate_input(
        data,
        rate,
        round(rate * args.arrival_ms / 1000),
        round(rate * args.noise_prefix_ms / 1000),
    )
    arrival, noise_frames = (
        round(rate * args.arrival_ms / 1000),
        round(rate * args.noise_prefix_ms / 1000),
    )
    checks, error = None, None
    try:
        model = load_model(args.model, args.model_sha256)
        policy = (
            load_policy(args.preservation_policy, args.policy_sha256)
            if args.preservation_policy
            else None
        )
        result, checks, _ = propose(
            samples, rate, arrival, noise_frames, model, preservation_policy=policy
        )
    except (OSError, KeyError, TypeError, ValueError) as exc:
        result, error = samples.astype("<f4"), str(exc)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("xb") as output:
        wavfile.write(output, rate, result[:, 0] if data.ndim == 1 else result)
    report = {
        "format": FORMAT,
        "input_sha256": sha(args.input),
        "output_sha256": sha(args.output),
        "model_sha256": args.model_sha256,
        "preservation_policy_sha256": args.policy_sha256,
        "sample_rate": rate,
        "frames": len(samples),
        "channels": samples.shape[1],
        "arrival_samples": arrival,
        "noise_prefix_samples": noise_frames,
        "checks": checks,
        "fallback_error": error,
        "used_learned_output": bool(checks and checks["accepted"]),
        "normalization": "none",
        "adoption_recommended": False,
        "review": "Experimental offline export; review response and listening before choosing it as a convolution IR.",
    }
    args.report.parent.mkdir(parents=True, exist_ok=True)
    with args.report.open("x") as report_file:
        report_file.write(json.dumps(report, indent=2, allow_nan=False) + "\n")
    print(json.dumps(report))


if __name__ == "__main__":
    main()
