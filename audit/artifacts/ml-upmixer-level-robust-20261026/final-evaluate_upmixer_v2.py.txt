"""Fresh final numerical and conditional spectral diagnostics for frozen v2 masks.

Conditional spectral metrics isolate the shared amplitude mask on known generated
stems. They do not measure leakage of the complete nonlinear multichannel chain.
"""

import argparse
import hashlib
import json
import shutil
from pathlib import Path

import numpy as np
from evaluate_upmixer_mask_native import audio_metrics
from upmixer_mask_cases import fixture
from upmixer_mask_native import render
from upmixer_source_mask import FFT, analyze, evidence, predict, targets
from upmixer_source_mask_v2 import load, observations

KINDS = (
    "matched",
    "hard_pan",
    "correlated_bed",
    "antiphase_bed",
    "direct_only",
    "ambient_only",
    "level_shift",
    "silence",
)


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def powers(spectra):
    return np.stack(
        [
            np.sum(abs(spectra[:, start : min(start + 8, FFT // 2)]) ** 2, axis=(1, 2))
            for start in range(1, FFT // 2, 8)
        ],
        axis=1,
    )


def diagnostics(mask, target, direct_power, ambient_power, eligible):
    def attenuation(power):
        denominator = power[eligible].sum()
        return (
            float(
                10
                * np.log10(
                    max(float((power * mask**2)[eligible].sum()), 1e-300) / denominator
                )
            )
            if denominator
            else None
        )

    pd = direct_power[eligible].sum()
    return {
        "selected_band_mask_mse": float(
            np.mean((mask[eligible] - target[eligible]) ** 2)
        )
        if eligible.any()
        else None,
        "conditional_spectral_direct_retention_db": attenuation(direct_power),
        "conditional_spectral_ambient_leakage_db": attenuation(ambient_power),
        "conditional_direct_mask_relative_rms_damage": float(
            np.sqrt((direct_power * (1 - mask) ** 2)[eligible].sum() / pd)
        )
        if pd
        else None,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--training", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--seed-start", type=int, default=20304120)
    parser.add_argument("--level-validation", type=Path)
    args = parser.parse_args()
    training = json.loads((args.training / "report.json").read_text())
    validation_sha = None
    if training.get("training_profile") == "level_robust_v2":
        if args.level_validation is None:
            raise ValueError(
                "Level-robust candidate requires a passing development validation report"
            )
        validation = json.loads(args.level_validation.read_text())
        if (
            not validation["development_gate_pass"]
            or validation["model_sha256"] != training["model_sha256"]
            or validation["training_report_sha256"]
            != digest(args.training / "report.json")
        ):
            raise ValueError("Development validation gate or candidate binding failed")
        validation_sha = digest(args.level_validation)
    args.output.mkdir(parents=True, exist_ok=False)
    renderer = args.output / "renderer.frozen"
    model = args.output / "model.frozen.json"
    shutil.copy2(args.training / "renderer.frozen", renderer)
    shutil.copy2(args.training / "model.json", model)
    sha = training["model_sha256"]
    theta = load(model, sha)
    if digest(renderer) != training["renderer_sha256"]:
        raise RuntimeError("Training renderer identity mismatch")
    sources = {}
    for name in [
        Path(__file__).name,
        "upmixer_source_mask.py",
        "upmixer_source_mask_v2.py",
        "upmixer_mask_cases.py",
        "upmixer_mask_native.py",
        "evaluate_upmixer_mask_native.py",
    ]:
        target = args.output / (name + ".txt")
        shutil.copy2(Path(__file__).parent / name, target)
        sources[name] = digest(target)
    cases = []
    for seed in range(args.seed_start, args.seed_start + 8):
        for kind in KINDS:
            original, direct, ambient, identity = fixture(seed, kind)
            for level_db in [0, -40, 20]:
                gain = 10 ** (level_db / 20)
                signal = np.asarray(original * gain, dtype="<f4")
                layout = "5.1" if seed % 2 == 0 else "7.1"
                work = args.output / str(len(cases))
                output, meta = render(
                    signal,
                    renderer,
                    work / "selected",
                    model=model,
                    sha=sha,
                    layout=layout,
                )
                native_output, native_meta = render(
                    signal, renderer, work / "native", layout=layout
                )
                if not meta["model_loaded"] or native_meta["model_loaded"]:
                    raise RuntimeError("Frozen v2/native model selection mismatch")
                features, eligible, native_power = observations(meta)
                count = len(features)
                spectra = analyze(signal)[:count]
                reference, _, valid = evidence(spectra)
                power = powers(spectra)
                reference_eligible = valid & (
                    power >= power.max(axis=1, keepdims=True) * 1e-6
                )
                present = np.array(
                    [
                        [v is not None for v in row["state"]["masks"]]
                        for row in meta["rows"]
                    ]
                )
                masks = np.array(
                    [
                        [v if v is not None else 0.0 for v in row["state"]["masks"]]
                        for row in meta["rows"]
                    ]
                )
                shared = eligible & reference_eligible
                maximum = (
                    float(abs(features[shared] - reference[shared]).max())
                    if shared.any()
                    else 0.0
                )
                network_error = (
                    float(
                        abs(predict(theta, features[eligible]) - masks[eligible]).max()
                    )
                    if eligible.any()
                    else 0.0
                )
                retained_energy = (
                    float(power[shared].sum() / power.sum()) if power.sum() else None
                )
                sd, sa = (
                    analyze(np.asarray(stem * gain, dtype="<f4"))[:count]
                    for stem in [direct, ambient]
                )
                target = targets(sd, sa)
                scalar = np.maximum(
                    features[:, :, 0], np.maximum(2 * features[:, :, 1] - 1, 0)
                )
                delta = output.astype(float) - native_output
                cases.append(
                    {
                        "identity": identity,
                        "level_db": level_db,
                        "layout": layout,
                        "files": meta["files"],
                        "native_files": native_meta["files"],
                        "parity": {
                            "eligible_band_frames": int(eligible.sum()),
                            "eligibility_disagreements": int(
                                np.count_nonzero(eligible != reference_eligible)
                            ),
                            "native_mask_selection_disagreements": int(
                                np.count_nonzero(present != eligible)
                            ),
                            "maximum_selected_feature_error": maximum,
                            "maximum_network_error_on_native_features": network_error,
                            "retained_reference_band_energy_fraction": retained_energy,
                            "feature_gate_pass": bool(
                                np.array_equal(eligible, reference_eligible)
                                and maximum <= 1e-4
                                and (
                                    retained_energy is None
                                    or (shared.any() and retained_energy >= 0.999)
                                )
                            ),
                            "network_gate_pass": bool(
                                np.array_equal(present, eligible)
                                and network_error <= 2e-7
                            ),
                            "native_power_finite": bool(
                                np.isfinite(native_power).all()
                            ),
                        },
                        "methods": {
                            name: diagnostics(
                                mask, target, powers(sd), powers(sa), eligible
                            )
                            for name, mask in [
                                ("learned", masks),
                                ("scalar_analytic_diagnostic", scalar),
                            ]
                        },
                        "selected_audio": audio_metrics(output, meta),
                        "native_audio": audio_metrics(native_output, native_meta),
                        "relative_rms_output_change": float(
                            np.sqrt(
                                np.sum(delta**2)
                                / max(np.sum(native_output.astype(float) ** 2), 1e-30)
                            )
                        ),
                    }
                )
    if digest(renderer) != training["renderer_sha256"] or digest(model) != sha:
        raise RuntimeError("Frozen candidate changed during final evaluation")
    report = {
        "scope": "fresh generated final sources; native 48kHz FFT512; conditional spectral metrics are not full-chain source separation or listener acceptance",
        "training_report_sha256": digest(args.training / "report.json"),
        "renderer_sha256": digest(renderer),
        "model_sha256": sha,
        "source_sha256": sources,
        "final_seed_start": args.seed_start,
        "level_validation_report_sha256": validation_sha,
        "feature_tolerance": 1e-4,
        "network_tolerance": 2e-7,
        "minimum_retained_energy": 0.999,
        "cases": cases,
        "deployment_admitted": False,
    }
    (args.output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    print(
        json.dumps(
            {
                "cases": len(cases),
                "feature_gate_failures": sum(
                    not x["parity"]["feature_gate_pass"] for x in cases
                ),
                "network_gate_failures": sum(
                    not x["parity"]["network_gate_pass"] for x in cases
                ),
                "deployment_admitted": False,
            }
        )
    )


if __name__ == "__main__":
    main()
