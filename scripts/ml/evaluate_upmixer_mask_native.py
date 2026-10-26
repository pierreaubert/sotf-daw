"""Frozen native/Python feature parity and descriptive multichannel routing probes.

The1e-4 feature tolerance is an assessment gate, not a reason to drop dark bins.
Native probabilities must match inference on their own exported features within2e-7.
Mixture output changes do not measure isolated source leakage in a nonlinear chain.
"""

import argparse
import hashlib
import json
import shutil
from pathlib import Path

import numpy as np
from upmixer_mask_cases import fixture
from upmixer_mask_native import render
from upmixer_source_mask import analyze, evidence, load, predict


def parity(source, metadata, theta):
    reference, _, valid = evidence(analyze(np.asarray(source, dtype="<f4")))
    count = len(metadata["rows"])
    reference, valid = reference[:count], valid[:count]
    native_valid = np.array(
        [[f is not None for f in row["state"]["features"]] for row in metadata["rows"]]
    )
    native = np.array(
        [
            [f if f is not None else [0.0] * 6 for f in row["state"]["features"]]
            for row in metadata["rows"]
        ]
    )
    masks = np.array(
        [
            [m if m is not None else 0.0 for m in row["state"]["masks"]]
            for row in metadata["rows"]
        ]
    )
    shared = valid & native_valid
    errors = abs(native[shared] - reference[shared])
    prediction_error = (
        float(np.max(abs(predict(theta, native[native_valid]) - masks[native_valid])))
        if native_valid.any()
        else 0.0
    )
    max_feature = np.max(errors, axis=0).tolist() if shared.any() else [0.0] * 6
    return {
        "compared_band_frames": int(shared.sum()),
        "validity_disagreements": int(np.count_nonzero(valid != native_valid)),
        "maximum_feature_errors": max_feature,
        "feature_error_p95": np.quantile(errors, 0.95, axis=0).tolist()
        if shared.any()
        else [0.0] * 6,
        "feature_gate_pass": not np.any(valid != native_valid)
        and max(max_feature) <= 1e-4,
        "maximum_network_error_on_native_features": prediction_error,
        "network_gate_pass": prediction_error <= 2e-7,
    }


def audio_metrics(audio, metadata):
    a = np.asarray(audio, float)
    energy = np.sum(a * a, axis=0)
    positions = metadata["speaker_positions"]
    groups = {
        "front": [p["channel"] for p in positions if p["label"] in {"FL", "FR", "C"}],
        "surround": [
            p["channel"]
            for p in positions
            if not p["is_lfe"] and p["label"] not in {"FL", "FR", "C"}
        ],
        "lfe": [p["channel"] for p in positions if p["is_lfe"]],
    }
    group_energy = {
        name: float(energy[indices].sum()) for name, indices in groups.items()
    }
    return {
        "group_energy": group_energy,
        "surround_to_front_db": float(
            10
            * np.log10(
                max(group_energy["surround"], 1e-30) / max(group_energy["front"], 1e-30)
            )
        )
        if group_energy["front"] > 1e-30
        else None,
        "peak": float(np.max(abs(a))),
        "maximum_sample_difference": float(np.max(abs(np.diff(a, axis=0)))),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", required=True, type=Path)
    parser.add_argument("--model", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    frozen = args.output / "renderer.frozen"
    shutil.copy2(args.renderer, frozen)
    sha = hashlib.sha256(args.model.read_bytes()).hexdigest()
    theta = load(args.model, sha)
    cases = [("training", s, "matched", "5.1") for s in range(20295120, 20295152)]
    cases += [("validation", s, "matched", "5.1") for s in range(20296120, 20296128)]
    kinds = [
        "matched",
        "hard_pan",
        "correlated_bed",
        "antiphase_bed",
        "direct_only",
        "ambient_only",
        "level_shift",
        "silence",
    ]
    cases += [
        ("prior_final", s, k, "5.1") for k in kinds for s in range(20297120, 20297136)
    ]
    cases += [
        ("fresh_layout_probe", s, k, "7.1")
        for k in kinds
        for s in range(20298120, 20298124)
    ]
    rows = []
    for i, (split, seed, kind, layout) in enumerate(cases):
        x, _, _, identity = fixture(seed, kind)
        root = args.output / str(i)
        selected, smeta = render(
            x, frozen, root / "selected", model=args.model, sha=sha, layout=layout
        )
        native, nmeta = render(x, frozen, root / "native", layout=layout)
        if (
            not smeta["model_loaded"]
            or nmeta["model_loaded"]
            or selected.shape != native.shape
        ):
            raise RuntimeError("Model selection or output geometry mismatch")
        p = parity(x, smeta, theta)
        if not p["network_gate_pass"]:
            raise RuntimeError("Native inference differs from independent Python")
        if kind == "silence" and (np.any(selected) or np.any(native)):
            raise RuntimeError("Silent audio generated output")
        delta = selected.astype(float) - native
        rows.append(
            {
                "split": split,
                "identity": identity,
                "layout": layout,
                "parity": p,
                "selected": audio_metrics(selected, smeta),
                "native": audio_metrics(native, nmeta),
                "relative_rms_output_change": float(
                    np.sqrt(
                        np.sum(delta * delta)
                        / max(np.sum(native.astype(float) ** 2), 1e-30)
                    )
                ),
                "selected_files": smeta["files"],
                "native_files": nmeta["files"],
                "selected_source_state": smeta["final_state"],
            }
        )
    report = {
        "model_sha256": sha,
        "renderer_sha256": hashlib.sha256(frozen.read_bytes()).hexdigest(),
        "scope": "native 48kHz FFT512 HR/scalar-ML/AutoGain disabled; dialogue weight0; other native defaults; mono harmonic source panned into stereo plus generated bed",
        "feature_tolerance": 1e-4,
        "network_tolerance": 2e-7,
        "cases": rows,
        "deployment_admitted": False,
    }
    (args.output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    print(
        json.dumps(
            {
                "cases": len(rows),
                "feature_gate_failures": sum(
                    not r["parity"]["feature_gate_pass"] for r in rows
                ),
                "deployment_admitted": False,
            }
        )
    )


if __name__ == "__main__":
    main()
