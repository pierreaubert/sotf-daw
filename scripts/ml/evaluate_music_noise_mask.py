"""Native music-mask waveform, leakage and transient probes without retuning."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from music_mask_cases import fixture
from music_mask_native import parameters, render


def metrics(actual, target):
    actual, target = np.asarray(actual, float), np.asarray(target, float)
    error = actual - target
    energy = max(float(np.sum(target**2)), 1e-20)
    error_energy = max(float(np.sum(error**2)), 1e-30)
    return {
        "snr_db": float(10 * np.log10(energy / error_energy)),
        "relative_rms": float(np.sqrt(error_energy / energy)),
        "max_abs_error": float(np.max(abs(error))),
        "projection_gain": float(np.sum(actual * target) / energy),
    }


def crest(values):
    values = np.asarray(values, float)
    power = abs(np.fft.rfft(values * np.hanning(len(values)))) ** 2
    return float(power.max() / max(power.mean(), 1e-20))


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--renderer", type=Path, required=True)
    p.add_argument("--model", type=Path, required=True)
    p.add_argument("--work", type=Path, required=True)
    p.add_argument("--report", type=Path, required=True)
    args = p.parse_args()
    args.work.mkdir(parents=True, exist_ok=False)
    identity = hashlib.sha256(args.model.read_bytes()).hexdigest()
    binary = hashlib.sha256(args.renderer.read_bytes()).hexdigest()
    cases = [
        (44100, 1, "matched"),
        (48000, 1, "matched"),
        (96000, 1, "matched"),
        (48000, 2, "matched"),
        (48000, 2, "clean_only"),
        (48000, 1, "dense_percussion"),
        (48000, 1, "vibrato"),
        (48000, 1, "step_noise"),
        (48000, 1, "tone_noise"),
    ]
    rows = []
    for i, (rate, channels, kind) in enumerate(cases):
        mix, clean, _noise, source = fixture(20269120 + i, rate, kind=kind)
        x = mix[:, None]
        target = clean[:, None]
        if channels == 2:
            _, _, right_noise, _ = fixture(20270120 + i, rate, kind=kind)
            x = np.column_stack([mix, 0.8 * clean + right_noise]).astype("<f4")
            target = np.column_stack([clean, 0.8 * clean]).astype("<f4")
        target_path = args.work / f"target-{i}.f32le"
        target_path.write_bytes(target.astype("<f4").tobytes())
        # Choose transient windows using clean data alone before any output is read.
        difference = abs(np.diff(clean.astype(float), prepend=0))
        onsets = []
        for index in np.argsort(difference)[::-1]:
            if index < rate // 4 or index + rate // 100 >= len(clean):
                continue
            if all(abs(int(index) - j) > rate // 20 for j in onsets):
                onsets.append(int(index))
            if len(onsets) == 12:
                break
        for low in [True, False]:
            name = f"{i}-{kind}-{rate}-{channels}-fft{512 if low else 2048}"
            row = {
                "case": name,
                "kind": kind,
                "source": source,
                "channels": channels,
                "sample_rate": rate,
                "low_latency": low,
                "target_file": str(target_path),
                "target_sha256": hashlib.sha256(
                    target.astype("<f4").tobytes()
                ).hexdigest(),
                "clean_ranked_transient_indices": onsets,
                "methods": {},
            }
            for method in ["classical", "neural"]:
                work = args.work / (name + "-" + method)
                y, m = render(
                    x,
                    args.renderer,
                    work,
                    parameters(low),
                    model=args.model,
                    sha=identity,
                    enabled=method == "neural",
                    rate=rate,
                )
                if not m["candidate_loaded"]:
                    raise ValueError("Frozen native musical model did not load")
                latency = m["latency_samples"]
                aligned = y[latency : latency + len(target)].astype(float)
                scores = {
                    "full": metrics(aligned, target),
                    "steady_after_half_second": metrics(
                        aligned[rate // 2 :], target[rate // 2 :]
                    ),
                    "trailing_energy": float(
                        np.sum(y[latency + len(target) :].astype(float) ** 2)
                    ),
                    "metadata": m,
                    "output_file": str(work / "output.f32le"),
                }
                first, last = 3 * rate // 20, 6 * rate // 25
                if kind != "clean_only":
                    scores["noise_only_lead_attenuation_db"] = float(
                        10
                        * np.log10(
                            max(float(np.sum(x[first:last].astype(float) ** 2)), 1e-30)
                            / max(float(np.sum(aligned[first:last] ** 2)), 1e-30)
                        )
                    )
                    scores["lead_noise_spectral_crest"] = crest(aligned[first:last, 0])
                onset_error = sum(
                    float(
                        np.sum(
                            (aligned[j : j + rate // 100] - target[j : j + rate // 100])
                            ** 2
                        )
                    )
                    for j in onsets
                )
                onset_energy = sum(
                    float(np.sum(target[j : j + rate // 100].astype(float) ** 2))
                    for j in onsets
                )
                scores["clean_ranked_transient_relative_error"] = float(
                    np.sqrt(onset_error / max(onset_energy, 1e-20))
                )
                if channels == 2:
                    scores["stereo_balance_residual_rms"] = float(
                        np.sqrt(np.mean((aligned[:, 1] - 0.8 * aligned[:, 0]) ** 2))
                    )
                row["methods"][method] = scores
            row["steady_neural_minus_classical_snr_db"] = (
                row["methods"]["neural"]["steady_after_half_second"]["snr_db"]
                - row["methods"]["classical"]["steady_after_half_second"]["snr_db"]
            )
            rows.append(row)
    if (
        hashlib.sha256(args.model.read_bytes()).hexdigest() != identity
        or hashlib.sha256(args.renderer.read_bytes()).hexdigest() != binary
    ):
        raise ValueError("Frozen music model/renderer changed")
    report = {
        "model_sha256": identity,
        "renderer_sha256": binary,
        "cases": rows,
        "deployment_admitted": False,
        "scope": "18 generated waveform probes across two native FFT modes. No recorded music or arbitrary-music/perceptual acceptance. Steady SNR, lead noise leakage, clean-ranked transient error, noise spectral crest and stereo residual are distinct diagnostics. Mixture errors include residual noise; projection gain is not isolated target distortion. Native latency and complete frozen-model drain are preserved; no fitted delay/gain or normalization.",
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
