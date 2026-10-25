"""Compare actual native denoising and classical expansion on fresh waveforms."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from expander_mask_native import parameters, reference, render
from music_mask_cases import fixture


def rms(x):
    return float(np.sqrt(np.mean(np.asarray(x, float) ** 2)))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    sha = hashlib.sha256(args.model.read_bytes()).hexdigest()
    binary_sha = hashlib.sha256(args.renderer.read_bytes()).hexdigest()
    report = {
        "model_sha256": sha,
        "renderer_sha256": binary_sha,
        "cases": [],
        "deployment_admitted": False,
        "scope": "Generated music/noise probes only; actual native full stream and independent conditional stem decomposition. No recorded-music or production admission.",
    }
    cases = [
        ("matched", rate, channels)
        for rate in [44100, 48000, 96000]
        for channels in [1, 2]
    ]
    cases += [
        (kind, 48000, 2 if kind in {"dense_percussion", "step_noise"} else 1)
        for kind in [
            "clean_only",
            "noise_only",
            "dense_percussion",
            "step_noise",
            "tone_noise",
            "vibrato",
            "level_shift",
        ]
    ]
    for i, (kind, rate, channels) in enumerate(cases):
        sources = [
            fixture(20278120 + i + 1000 * c, rate, kind=kind) for c in range(channels)
        ]
        clean, noise = [
            np.column_stack([x[k] for x in sources]).astype("<f4") for k in [1, 2]
        ]
        if kind == "noise_only":
            clean.fill(0)
        if kind == "level_shift":
            clean *= 4
            noise *= 4
        mixture = (clean + noise).astype("<f4")
        item = {
            "case": f"{i}-{kind}-{rate}-{channels}ch",
            "kind": kind,
            "sample_rate": rate,
            "channels": channels,
            "source_seeds": [x[3]["seed"] for x in sources],
            "methods": {},
        }
        for name, values in [("clean", clean), ("noise", noise)]:
            path = args.output / f"{i}-{name}.f32le"
            path.write_bytes(values.tobytes())
            item[name + "_file"] = {
                "path": str(path.resolve()),
                "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            }
        for mode in ["unity", "classical", "learned"]:
            output, gains, metadata = render(
                mixture,
                args.renderer,
                args.output / item["case"] / mode,
                model=args.model,
                sha=sha,
                enabled=mode == "learned",
                rate=rate,
                params=parameters(ratio=1.0 if mode == "unity" else 4.0),
            )
            expected = reference(
                mixture, metadata["analysis_window"], gains, len(output)
            )
            desired = reference(clean, metadata["analysis_window"], gains, len(output))
            residual = reference(noise, metadata["analysis_window"], gains, len(output))
            wola_error = float(np.max(abs(output - expected)))
            sum_error = float(np.max(abs(output - desired - residual)))
            if max(wola_error, sum_error) > 2e-7:
                raise RuntimeError(
                    "Native output does not match independent conditional WOLA"
                )
            target = np.zeros_like(expected)
            target[1024 : 1024 + len(clean)] = clean
            original_noise = np.zeros_like(expected)
            original_noise[1024 : 1024 + len(noise)] = noise
            error = rms(output - target)
            item["methods"][mode] = {
                "snr_db": float(
                    20 * np.log10(max(rms(target), 1e-20) / max(error, 1e-20))
                )
                if np.any(clean)
                else None,
                "clean_component_relative_error": rms(desired - target)
                / max(rms(target), 1e-20)
                if np.any(clean)
                else None,
                "residual_noise_attenuation_db": float(
                    20
                    * np.log10(
                        max(rms(original_noise), 1e-20) / max(rms(residual), 1e-20)
                    )
                )
                if np.any(noise)
                else None,
                "conditional_wola_max_error": wola_error,
                "conditional_stem_sum_max_error": sum_error,
                "metadata": metadata,
            }
        item["learned_minus_classical_snr_db"] = (
            (
                item["methods"]["learned"]["snr_db"]
                - item["methods"]["classical"]["snr_db"]
            )
            if np.any(clean)
            else None
        )
        report["cases"].append(item)
    if (
        hashlib.sha256(args.model.read_bytes()).hexdigest() != sha
        or hashlib.sha256(args.renderer.read_bytes()).hexdigest() != binary_sha
    ):
        raise RuntimeError("Frozen evaluation inputs changed")
    (args.output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    print(
        json.dumps(
            {
                "cases": len(report["cases"]),
                "snr_deltas": [
                    x["learned_minus_classical_snr_db"] for x in report["cases"]
                ],
                "deployment_admitted": False,
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
