"""Recompute frozen v2 final numerical gates from hashed raw native exports."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from upmixer_mask_cases import fixture
from upmixer_source_mask import FFT, analyze, evidence, predict
from upmixer_source_mask_v2 import load


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def verify(root, training_root):
    report = json.loads((root / "report.json").read_text())
    training = json.loads((training_root / "report.json").read_text())
    kinds = {
        "matched",
        "hard_pan",
        "correlated_bed",
        "antiphase_bed",
        "direct_only",
        "ambient_only",
        "level_shift",
        "silence",
    }
    expected = {
        (seed, kind, level)
        for seed in range(20304120, 20304128)
        for kind in kinds
        for level in [0, -40, 20]
    }
    actual = [
        (row["identity"]["seed"], row["identity"]["kind"], row["level_db"])
        for row in report["cases"]
    ]
    if len(actual) != len(expected) or set(actual) != expected:
        raise ValueError("Final case matrix is incomplete or duplicated")
    if digest(training_root / "report.json") != report["training_report_sha256"]:
        raise ValueError("Training report binding mismatch")
    if (
        digest(root / "renderer.frozen") != report["renderer_sha256"]
        or report["renderer_sha256"] != training["renderer_sha256"]
    ):
        raise ValueError("Frozen renderer identity mismatch")
    theta = load(root / "model.frozen.json", report["model_sha256"])
    if report["model_sha256"] != training["model_sha256"]:
        raise ValueError("Final model differs from training candidate")
    if (
        report["feature_tolerance"],
        report["network_tolerance"],
        report["minimum_retained_energy"],
    ) != (1e-4, 2e-7, 0.999):
        raise ValueError("Final gates changed")
    development_seeds = {
        row["identity"]["seed"]
        for split in ["training", "validation"]
        for row in training[split]
    }
    checked = 0
    gate_failures = 0
    maximum_feature = 0.0
    maximum_network = 0.0
    minimum_retained = 1.0
    total_eligible = 0
    for row in report["cases"]:
        identity = row["identity"]
        if row["layout"] != ("5.1" if identity["seed"] % 2 == 0 else "7.1"):
            raise ValueError("Final layout assignment differs from the frozen matrix")
        if (
            identity["seed"] in development_seeds
            or 20301120 <= identity["seed"] <= 20301123
        ):
            raise ValueError("Final source overlaps development")
        source, direct, ambient, expected_identity = fixture(
            identity["seed"], identity["kind"]
        )
        signal = np.asarray(source * 10 ** (row["level_db"] / 20), dtype="<f4")
        if (
            identity != expected_identity
            or hashlib.sha256(signal.tobytes()).hexdigest()
            != row["files"]["input"]["sha256"]
        ):
            raise ValueError("Regenerated source mismatch")
        for key in ["files", "native_files"]:
            for entry in row[key].values():
                if digest(entry["path"]) != entry["sha256"]:
                    raise ValueError("Raw evidence hash mismatch")
                checked += 1
        meta = json.loads(Path(row["files"]["features_output"]["path"]).read_text())
        native_meta = json.loads(
            Path(row["native_files"]["features_output"]["path"]).read_text()
        )
        if (
            not meta["model_loaded"]
            or native_meta["model_loaded"]
            or meta["latency_samples"] != 512
            or meta["model_lookahead_samples"] != 0
        ):
            raise ValueError("Native selection/latency mismatch")
        spectra = analyze(signal)[: len(meta["rows"])]
        reference, _, valid = evidence(spectra)
        power = np.stack(
            [
                np.sum(abs(spectra[:, s : min(s + 8, FFT // 2)]) ** 2, axis=(1, 2))
                for s in range(1, FFT // 2, 8)
            ],
            axis=1,
        )
        state = [x["state"] for x in meta["rows"]]
        features = np.array(
            [[x if x is not None else [0.0] * 6 for x in s["features"]] for s in state]
        )
        available = np.array([[x is not None for x in s["features"]] for s in state])
        native_power = np.array([s["band_power"] for s in state], dtype=float)
        if not np.isfinite(native_power).all() or np.any(native_power < 0):
            raise ValueError("Invalid native band powers")
        selected = np.array([[x is not None for x in s["masks"]] for s in state])
        masks = np.array(
            [[x if x is not None else 0.0 for x in s["masks"]] for s in state]
        )
        a = valid & (power >= power.max(axis=1, keepdims=True) * 1e-6)
        b = available & (native_power >= native_power.max(axis=1, keepdims=True) * 1e-6)
        shared = a & b
        error = (
            float(abs(features[shared] - reference[shared]).max())
            if shared.any()
            else 0.0
        )
        network = (
            float(abs(predict(theta, features[b]) - masks[b]).max()) if b.any() else 0.0
        )
        retained = float(power[shared].sum() / power.sum()) if power.sum() else None
        feature_pass = bool(
            np.array_equal(a, b)
            and error <= 1e-4
            and (retained is None or (shared.any() and retained >= 0.999))
        )
        network_pass = bool(np.array_equal(selected, b) and network <= 2e-7)
        recomputed = {
            "eligible_band_frames": int(b.sum()),
            "eligibility_disagreements": int(np.count_nonzero(a != b)),
            "native_mask_selection_disagreements": int(np.count_nonzero(selected != b)),
            "maximum_selected_feature_error": error,
            "maximum_network_error_on_native_features": network,
            "retained_reference_band_energy_fraction": retained,
            "feature_gate_pass": feature_pass,
            "network_gate_pass": network_pass,
            "native_power_finite": bool(np.isfinite(native_power).all()),
        }
        if recomputed != row["parity"]:
            raise ValueError("Final numerical report differs from recomputation")
        stem_power = []
        for stem in [direct, ambient]:
            spectrum = analyze(
                np.asarray(stem * 10 ** (row["level_db"] / 20), dtype="<f4")
            )[: len(state)]
            stem_power.append(
                np.stack(
                    [
                        np.sum(
                            abs(spectrum[:, s : min(s + 8, FFT // 2)]) ** 2, axis=(1, 2)
                        )
                        for s in range(1, FFT // 2, 8)
                    ],
                    axis=1,
                )
            )
        pd, pa = stem_power
        target = pd / np.maximum(pd + pa, 1e-16)
        scalar = np.maximum(features[:, :, 0], np.maximum(2 * features[:, :, 1] - 1, 0))
        for name, mask in [("learned", masks), ("scalar_analytic_diagnostic", scalar)]:
            values = {
                "selected_band_mask_mse": float(np.mean((mask[b] - target[b]) ** 2))
                if b.any()
                else None,
                "conditional_direct_mask_relative_rms_damage": float(
                    np.sqrt((pd * (1 - mask) ** 2)[b].sum() / pd[b].sum())
                )
                if pd[b].sum()
                else None,
            }
            for key, p in [
                ("conditional_spectral_direct_retention_db", pd),
                ("conditional_spectral_ambient_leakage_db", pa),
            ]:
                values[key] = (
                    float(
                        10
                        * np.log10(
                            max(float((p * mask**2)[b].sum()), 1e-300) / p[b].sum()
                        )
                    )
                    if p[b].sum()
                    else None
                )
            if values != row["methods"][name]:
                raise ValueError(
                    "Conditional spectral metrics differ from independent stem reconstruction"
                )
        total_eligible += int(b.sum())
        maximum_feature = max(maximum_feature, error)
        maximum_network = max(maximum_network, network)
        if retained is not None:
            minimum_retained = min(minimum_retained, retained)
        gate_failures += not (feature_pass and network_pass)
    summary = {}
    for kind in sorted({r["identity"]["kind"] for r in report["cases"]}):
        rows = [
            r
            for r in report["cases"]
            if r["identity"]["kind"] == kind and r["level_db"] == 0
        ]
        summary[kind] = {}
        for method in ["learned", "scalar_analytic_diagnostic"]:
            summary[kind][method] = {
                key: float(np.mean(values)) if values else None
                for key in rows[0]["methods"][method]
                for values in [
                    [
                        r["methods"][method][key]
                        for r in rows
                        if r["methods"][method][key] is not None
                    ]
                ]
            }
    return {
        "cases": len(report["cases"]),
        "verified_raw_hashes": checked,
        "eligible_band_frames": total_eligible,
        "numerical_gate_failures": gate_failures,
        "maximum_selected_feature_error": maximum_feature,
        "maximum_native_network_error": maximum_network,
        "minimum_retained_reference_band_energy_fraction": minimum_retained,
        "conditional_spectral_summary_at_original_level": summary,
        "quality_summary_scope": "conditional spectral metrics recomputed from regenerated known stems and native masks; excludes nonlinear multichannel routing",
        "report_sha256": digest(root / "report.json"),
        "deployment_admitted": False,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--final", type=Path, required=True)
    parser.add_argument("--training", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = verify(args.final, args.training)
    with args.output.open("x") as stream:
        stream.write(json.dumps(result, indent=2, allow_nan=False) + "\n")
    print(
        json.dumps(
            {
                k: v
                for k, v in result.items()
                if k != "conditional_spectral_summary_at_original_level"
            }
        )
    )


if __name__ == "__main__":
    main()
