"""Native waveform probes for a frozen generated spatial mask, without retuning."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from beamformer_mask_native import parameters, render
from train_aec_residual import source


def fixture(rate, mics, kind, seed):
    rng = np.random.default_rng(seed)
    frames = rate * 2 + 17
    target = source(rate, frames, rng) * 0.02
    noise = source(rate, frames, rng) * 0.03
    if kind == "target_only":
        noise.fill(0)
    # Noise-only lead-in allows the classical path to adapt without target leakage.
    if kind != "target_only":
        target[: rate // 3] = 0
    angle = 5 if kind == "near_interference" else 55
    position = np.arange(frames)
    microphones = []
    for mic in range(mics):
        delay = rate * mic * 0.05 * np.sin(np.deg2rad(angle)) / 343
        local_target = target.copy()
        if kind == "reverberant_target":
            reflection = rate * (0.012 + mic * 0.05 * np.sin(np.deg2rad(40)) / 343)
            local_target += 0.7 * np.interp(
                position - reflection, position, target, left=0, right=0
            )
        mixture = local_target + np.interp(
            position - delay, position, noise, left=0, right=0
        )
        if kind == "sensor_gain":
            mixture *= np.exp(rng.normal(0, 0.35))
        microphones.append(mixture)
    return np.column_stack(microphones).astype("<f4"), target.astype("<f4")


def metric(actual, target):
    error = actual.astype(float) - target
    energy = max(float(np.dot(target, target)), 1e-20)
    error_energy = max(float(np.dot(error, error)), 1e-30)
    return {
        "snr_db": float(10 * np.log10(energy / error_energy)),
        "relative_rms": float(np.sqrt(error_energy / energy)),
        "max_abs_error": float(np.max(abs(error))),
        "projection_gain": float(np.dot(actual, target) / energy),
    }


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--renderer", type=Path, required=True)
    p.add_argument("--model", type=Path, required=True)
    p.add_argument("--work", type=Path, required=True)
    p.add_argument("--report", type=Path, required=True)
    args = p.parse_args()
    args.work.mkdir(parents=True, exist_ok=False)
    model_sha = hashlib.sha256(args.model.read_bytes()).hexdigest()
    binary_sha = hashlib.sha256(args.renderer.read_bytes()).hexdigest()
    cases = [
        (16000, 2, "matched"),
        (48000, 2, "matched"),
        (96000, 2, "matched"),
        (48000, 8, "matched"),
        (96000, 8, "matched"),
        (48000, 4, "target_only"),
        (48000, 4, "near_interference"),
        (48000, 4, "sensor_gain"),
        (48000, 4, "reverberant_target"),
    ]
    rows = []
    for i, (rate, mics, kind) in enumerate(cases):
        x, target = fixture(rate, mics, kind, 20264120 + i)
        name = f"{i}-{kind}-{rate}-{mics}"
        target_path = args.work / (name + "-target.f32le")
        target_path.write_bytes(target.tobytes())
        row = {
            "case": name,
            "kind": kind,
            "sample_rate": rate,
            "microphones": mics,
            "seed": 20264120 + i,
            "target_file": str(target_path),
            "target_sha256": hashlib.sha256(target.tobytes()).hexdigest(),
            "input_reference": metric(x[:, 0].astype(float), target.astype(float)),
            "methods": {},
        }
        for method in ["classical", "neural"]:
            y, metadata = render(
                x,
                args.renderer,
                args.work / (name + "-" + method),
                parameters(mics),
                model=args.model,
                sha=model_sha,
                enabled=method == "neural",
                rate=rate,
            )
            if not metadata["candidate_loaded"]:
                raise ValueError("Frozen native mask did not load")
            aligned = y[512 : 512 + len(target)].astype(float)
            row["methods"][method] = {
                "full": metric(aligned, target.astype(float)),
                "steady_after_one_second": metric(
                    aligned[rate:], target[rate:].astype(float)
                ),
                "metadata": metadata,
                "output_file": str(args.work / (name + "-" + method) / "output.f32le"),
            }
        row["steady_neural_minus_classical_snr_db"] = (
            row["methods"]["neural"]["steady_after_one_second"]["snr_db"]
            - row["methods"]["classical"]["steady_after_one_second"]["snr_db"]
        )
        rows.append(row)
    if (
        hashlib.sha256(args.model.read_bytes()).hexdigest() != model_sha
        or hashlib.sha256(args.renderer.read_bytes()).hexdigest() != binary_sha
    ):
        raise ValueError("Frozen model/renderer changed during evaluation")
    report = {
        "model_sha256": model_sha,
        "renderer_sha256": binary_sha,
        "cases": rows,
        "deployment_admitted": False,
        "scope": "Nine generated waveform probes; voiced/noise envelopes are not recorded speech. Correlated two-second sources, linearly interpolated physical delays, fixed broadside target, no gain/delay fitting or normalization. Output offset is the native512-sample latency. Reverberant/sensor-gain errors include desired transfer mismatch; mixture projection gain is not isolated target distortion. No general model advantage or host deadline claim.",
    }
    with args.report.open("x") as out:
        out.write(json.dumps(report, indent=2, allow_nan=False) + "\n")
    print(
        json.dumps(
            {
                "cases": len(rows),
                "steady_snr_deltas_db": [
                    r["steady_neural_minus_classical_snr_db"] for r in rows
                ],
            }
        )
    )


if __name__ == "__main__":
    main()
