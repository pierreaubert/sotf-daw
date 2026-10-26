"""Development-only excitation-policy search using frozen native v1 exports.

This proposes a future contract; it never changes v1 validity or model selection.
No final evaluation or separation-quality claim is made by this experiment.
"""

import argparse
import hashlib
import json
import shutil
from pathlib import Path

import numpy as np
from upmixer_mask_cases import fixture
from upmixer_mask_native import render
from upmixer_source_mask import FFT, analyze, evidence

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
DYNAMIC_RANGES = (40, 50, 60)
ABSOLUTE_DB = -80
TOLERANCE = 1e-4


def eligible(features, valid, dynamic_db):
    """Gate the existing log-excitation coordinate, independently on each side.

    Coordinate 5 is (10log10(band_power / FFT) + 100) / 100, clipped.
    The absolute floor is above its clipping region. The relative floor uses
    the strongest analysis band, excluding DC/Nyquist, in the same source frame.
    These are empirical reliability thresholds, not FFT error bounds.
    """
    excitation = features[:, :, 5]
    peak = np.max(np.where(valid, excitation, 0), axis=1, keepdims=True)
    return (
        valid
        & (excitation >= (ABSOLUTE_DB + 100) / 100)
        & (excitation >= peak - dynamic_db / 100)
    )


def eligible_power(power, valid, dynamic_db):
    """Use unclipped native/reference powers; retain the v1 validity interval."""
    peak = np.max(power, axis=1, keepdims=True)
    return valid & (power >= peak * 10 ** (-dynamic_db / 10))


def assess(
    reference,
    reference_valid,
    native,
    native_valid,
    power,
    dynamic_db,
    native_power=None,
):
    if native_power is None:
        a = eligible(reference, reference_valid, dynamic_db)
        b = eligible(native, native_valid, dynamic_db)
    else:
        a = eligible_power(power, reference_valid, dynamic_db)
        b = eligible_power(native_power, native_valid, dynamic_db)
    shared = a & b
    error = abs(reference[shared] - native[shared])
    disagreements = int(np.count_nonzero(a != b))
    maximum = float(error.max()) if shared.any() else 0.0
    total = float(power.sum())
    retained = float(power[shared].sum() / total) if total else None
    return {
        "selected_band_frames": int(shared.sum()),
        "validity_disagreements": disagreements,
        "maximum_feature_error": maximum,
        "retained_reference_band_energy_fraction": retained,
        # Empty selection cannot pass merely by producing an empty error array.
        "pass": bool(
            disagreements == 0
            and maximum <= TOLERANCE
            and (total == 0 or (shared.any() and retained >= 0.999))
        ),
    }


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--raw-power",
        action="store_true",
        help="Use raw band powers instead of clipped excitation",
    )
    parser.add_argument(
        "--replay",
        action="store_true",
        help="Verify existing exports without rendering again",
    )
    args = parser.parse_args()
    if not args.replay:
        args.output.mkdir(parents=True, exist_ok=False)
    frozen = args.output / "renderer.frozen"
    if not args.replay:
        shutil.copy2(args.renderer, frozen)
    model = args.output / "model.v1.frozen.json"
    if not args.replay:
        shutil.copy2(args.model, model)
    sha = digest(model)
    sources = {}
    for name in (
        Path(__file__).name,
        "upmixer_mask_cases.py",
        "upmixer_mask_native.py",
        "upmixer_source_mask.py",
    ):
        target = args.output / (name + ".txt")
        if not args.replay:
            shutil.copy2(Path(__file__).parent / name, target)
        sources[name] = digest(target)
    renderer_sha = digest(frozen)
    rows = []
    for seed in range(20301120, 20301124):
        for kind in KINDS:
            source, _, _, identity = fixture(seed, kind)
            for level_db in (0, -40, 20):
                signal = np.asarray(source * 10 ** (level_db / 20), dtype="<f4")
                work = args.output / str(len(rows))
                if args.replay:
                    meta = json.loads((work / "metadata.json").read_text())
                    for entry in meta["files"].values():
                        if digest(entry["path"]) != entry["sha256"]:
                            raise RuntimeError("Raw export hash mismatch")
                    if (
                        digest(meta["files"]["input"]["path"])
                        != hashlib.sha256(signal.tobytes()).hexdigest()
                    ):
                        raise RuntimeError("Regenerated development source mismatch")
                else:
                    _, meta = render(signal, frozen, work, model=model, sha=sha)
                if not meta["model_loaded"]:
                    raise RuntimeError("Feature-export model was not selected")
                count = len(meta["rows"])
                spectra = analyze(signal)[:count]
                reference, _, valid = evidence(spectra)
                native_valid = np.array(
                    [
                        [v is not None for v in row["state"]["features"]]
                        for row in meta["rows"]
                    ]
                )
                native = np.array(
                    [
                        [
                            v if v is not None else [0.0] * 6
                            for v in row["state"]["features"]
                        ]
                        for row in meta["rows"]
                    ]
                )
                power = np.stack(
                    [
                        np.sum(
                            abs(spectra[:, s : min(s + 8, FFT // 2)]) ** 2, axis=(1, 2)
                        )
                        for s in range(1, FFT // 2, 8)
                    ],
                    axis=1,
                )
                native_power = None
                if args.raw_power:
                    native_power = np.array(
                        [
                            [
                                v if v is not None else 0.0
                                for v in row["state"]["band_power"]
                            ]
                            for row in meta["rows"]
                        ]
                    )
                    if not np.isfinite(native_power).all() or np.any(native_power < 0):
                        raise RuntimeError("Invalid raw native band powers")
                both = valid & native_valid
                rows.append(
                    {
                        "identity": identity,
                        "level_db": level_db,
                        "files": meta["files"],
                        "v1_maximum_feature_error": float(
                            abs(reference[both] - native[both]).max()
                        )
                        if both.any()
                        else 0.0,
                        "candidates": {
                            str(db): assess(
                                reference,
                                valid,
                                native,
                                native_valid,
                                power,
                                db,
                                native_power,
                            )
                            for db in DYNAMIC_RANGES
                        },
                    }
                )
    passing = [
        db
        for db in DYNAMIC_RANGES
        if all(r["candidates"][str(db)]["pass"] for r in rows)
    ]
    chosen = max(passing) if passing else None
    report = {
        "scope": "development only; v1 exports; no changed runtime or final holdout",
        "renderer_sha256": renderer_sha,
        "model_sha256": sha,
        "source_sha256": sources,
        "analysis_source_sha256": digest(__file__),
        "power_source": "unclipped_band_power"
        if args.raw_power
        else "clipped_feature_excitation",
        "feature_tolerance": TOLERANCE,
        "minimum_retained_reference_band_energy_fraction": 0.999,
        "selection_rule": "largest predeclared dynamic range passing every development case",
        "proposed_policy": None
        if chosen is None
        else {
            "format": "sotf-upmixer-excitation-policy-proposal-v1",
            "absolute_band_power_over_fft_db": None if args.raw_power else ABSOLUTE_DB,
            "feature_valid_band_power_interval": [1e-16, 1e12],
            "power_source": "unclipped_band_power"
            if args.raw_power
            else "clipped_feature_excitation",
            "dynamic_range_below_peak_band_db": chosen,
            "fallback": "ordinary_native_decomposition",
            "deployment_admitted": False,
        },
        "cases": rows,
        "deployment_admitted": False,
    }
    if digest(frozen) != renderer_sha or digest(model) != sha:
        raise RuntimeError(
            "Frozen executable/model identity changed during development"
        )
    (args.output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    print(
        json.dumps(
            {
                "cases": len(rows),
                "passing_dynamic_ranges_db": passing,
                "proposed_dynamic_range_db": chosen,
            }
        )
    )


if __name__ == "__main__":
    main()
