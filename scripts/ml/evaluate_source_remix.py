"""Native separate-remix probes with independently reconstructed conditional stems."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from music_mask_cases import fixture as music
from source_remix_native import reference, remix_parameters, render
from train_aec_residual import source


def itu(x):
    # Frozen current native ITU coefficient (0.707f32), not a recalibration.
    attenuation = float(np.float32(0.707))
    return np.column_stack(
        [
            x[:, 0] + attenuation * x[:, 2] + attenuation * x[:, 4],
            x[:, 1] + attenuation * x[:, 2] + attenuation * x[:, 5],
        ]
    )


def fixture(rate, kind, seed):
    frames = rate * 2 + 17
    rng = np.random.default_rng(seed)
    t = np.arange(frames) / rate
    foreground = source(rate, frames, rng) * 0.025
    _, left, _, _ = music(seed + 1000, rate, seconds=2)
    _, right, _, _ = music(seed + 2000, rate, seconds=2)
    left *= 0.7
    right *= 0.7
    if kind in {"dual_mono_bed", "bed_only_centered"}:
        right = left.copy()
    fg = np.zeros((frames, 6))
    bed = np.zeros_like(fg)
    fg[:, 2] = foreground
    if kind == "panned_foreground":
        fg[:, 2] *= 0.2
        fg[:, 0] = foreground
    if kind == "phase_shifted_foreground":
        fg[:, 2] = 0
        fg[:, 0] = foreground
        fg[:, 1] = np.interp(
            np.arange(frames) - rate * 0.0002,
            np.arange(frames),
            foreground,
            left=0,
            right=0,
        )
    if kind in {"bed_only", "bed_only_centered"}:
        fg.fill(0)
    if kind == "foreground_only":
        left.fill(0)
        right.fill(0)
    bed[:, 0] = left
    bed[:, 1] = right
    bed[:, 3] = 0.01 * np.sin(2 * np.pi * 60 * t)
    bed[37:, 4] = 0.4 * left[:-37]
    bed[37:, 5] = 0.4 * right[:-37]
    if kind == "level_shift":
        fg *= 4
        bed *= 4
    return (fg + bed).astype("<f4"), fg.astype("<f4"), bed.astype("<f4")


def metric(actual, target):
    error = actual - target
    energy = float(np.sum(target**2))
    if energy <= 1e-20:
        return {"target_energy": energy, "output_energy": float(np.sum(actual**2))}
    residual = max(float(np.sum(error**2)), 1e-30)
    return {
        "snr_db": float(10 * np.log10(energy / residual)),
        "relative_rms": float(np.sqrt(residual / energy)),
        "projection_gain": float(np.sum(actual * target) / energy),
    }


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ["renderer", "model", "work", "report"]:
        p.add_argument("--" + name, type=Path, required=True)
    args = p.parse_args()
    args.work.mkdir(parents=True, exist_ok=False)
    identity = hashlib.sha256(args.model.read_bytes()).hexdigest()
    binary = hashlib.sha256(args.renderer.read_bytes()).hexdigest()
    cases = [
        (44100, "matched"),
        (48000, "matched"),
        (96000, "matched"),
        (48000, "foreground_only"),
        (48000, "bed_only"),
        (48000, "dual_mono_bed"),
        (48000, "bed_only_centered"),
        (48000, "panned_foreground"),
        (48000, "phase_shifted_foreground"),
        (48000, "level_shift"),
    ]
    rows = []
    for i, (rate, kind) in enumerate(cases):
        x, fg, bed = fixture(rate, kind, 20274120 + i)
        for label, array in [("foreground", fg), ("bed", bed)]:
            (args.work / f"{i}-{label}.f32le").write_bytes(array.tobytes())
        f, b = itu(fg.astype(float)), itu(bed.astype(float))
        target = 10 ** (3 / 20) * f + 10 ** (-3 / 20) * b
        row = {
            "case": f"{i}-{kind}-{rate}",
            "kind": kind,
            "sample_rate": rate,
            "seed": 20274120 + i,
            "foreground_file": str(args.work / f"{i}-foreground.f32le"),
            "bed_file": str(args.work / f"{i}-bed.f32le"),
            "methods": {},
        }
        for method in ["bypass", "analytic", "learned"]:
            work = args.work / (row["case"] + "-" + method)
            arrays, m = render(
                x,
                args.renderer,
                work,
                remix_parameters(method),
                model=args.model,
                sha=identity,
                rate=rate,
            )
            if not m["candidate_loaded"]:
                raise ValueError("Frozen source model did not load")
            baseline = arrays["baseline"].astype(float)
            baseline_error = float(np.max(abs(baseline - itu(x.astype(float)))))
            if baseline_error > 2e-7:
                raise ValueError(
                    "Unchanged ITU downmix differs from frozen matrix reference"
                )
            expected = reference(
                baseline, m["analysis_window"], arrays["gains"], len(arrays["output"])
            )
            reconstruction = float(np.max(abs(expected - arrays["output"])))
            if reconstruction > 2e-7:
                raise ValueError("Native remix differs from conditional WOLA reference")
            f_out = reference(f, m["analysis_window"], arrays["gains"], len(expected))
            b_out = reference(b, m["analysis_window"], arrays["gains"], len(expected))
            stem_error = float(np.max(abs(f_out + b_out - arrays["output"])))
            if stem_error > 2e-7:
                raise ValueError("Conditional stems do not reconstruct native output")
            aligned = arrays["output"][512 : 512 + len(x)].astype(float)
            row["methods"][method] = {
                "full": metric(aligned, target),
                "steady_after_half_second": metric(
                    aligned[rate // 2 :], target[rate // 2 :]
                ),
                "foreground_component": metric(
                    f_out[512 : 512 + len(x)], 10 ** (3 / 20) * f
                ),
                "bed_component": metric(b_out[512 : 512 + len(x)], 10 ** (-3 / 20) * b),
                "native_baseline_max_error": baseline_error,
                "conditional_wola_max_error": reconstruction,
                "conditional_stem_sum_max_error": stem_error,
                "metadata": m,
                "directory": str(work),
            }
        row["learned_minus_analytic_steady_snr_db"] = (
            row["methods"]["learned"]["steady_after_half_second"]["snr_db"]
            - row["methods"]["analytic"]["steady_after_half_second"]["snr_db"]
        )
        rows.append(row)
    if (
        hashlib.sha256(args.model.read_bytes()).hexdigest() != identity
        or hashlib.sha256(args.renderer.read_bytes()).hexdigest() != binary
    ):
        raise ValueError("Frozen source-remix model/renderer changed")
    report = {
        "model_sha256": identity,
        "renderer_sha256": binary,
        "cases": rows,
        "deployment_admitted": False,
        "scope": "Ten generated6-channel fixtures, three separate remix methods after actual unchanged ITU5.1 downmix. Desired gains+3dB foreground/-3dB bed. All source metrics replay the mixture's fixed conditional gain trace; isolated stems never retune masks. Spatial centeredness is not semantic identity. No fitted gain/delay, normalization, limiter, real recording or product admission.",
    }
    with args.report.open("x") as out:
        out.write(json.dumps(report, indent=2, allow_nan=False) + "\n")
    print(
        json.dumps(
            {
                "cases": len(rows),
                "learned_minus_analytic_snr_db": [
                    r["learned_minus_analytic_steady_snr_db"] for r in rows
                ],
            }
        )
    )


if __name__ == "__main__":
    main()
