"""Frozen AAE observation-shift and optional measured AIR database evaluation.

No training, threshold selection, raw-recording export or automatic preset apply.
Measured arrival annotations are approximate and tested at +/-0.5 ms; they are
not dataset ground truth. Input archive provenance is pinned before any fit.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import zipfile
from pathlib import Path

import numpy as np
from aae_room_features import Renderer, actual_unit, describe
from aae_room_model import Surrogate
from fit_aae_room import fit
from scipy.io import loadmat

AIR_SHA256 = "4baa3a0ea31dc72647ff61f53b0bf0b962876de31b6b0569ebc504987898aa02"
# Fixed before inspecting fitting results. First substantial arrival in each
# selected closest-distance response; these annotations have explicit uncertainty.
AIR_CASES = (
    ("booth_0_0_1", 68),
    ("office_0_0_1", 146),
    ("meeting_0_0_1", 206),
    ("lecture_0_0_1", 320),
    ("stairway_0_1_1_90", 138),
    ("aula_carolina_0_1_1_90_3", 153),
)
SYNTHETIC_CONDITIONS = (
    ("reference_16k", 16000, 8.0, None, 0, 1.0),
    ("reference_48k", 48000, 8.0, None, 0, 1.0),
    ("reference_96k", 96000, 8.0, None, 0, 1.0),
    ("truncated_2s", 48000, 2.0, None, 0, 1.0),
    ("truncated_250ms", 48000, 0.25, None, 0, 1.0),
    ("noise_40db", 48000, 8.0, 40.0, 0, 1.0),
    ("noise_20db", 48000, 8.0, 20.0, 0, 1.0),
    ("noise_10db", 48000, 8.0, 10.0, 0, 1.0),
    ("arrival_late_5ms", 48000, 8.0, None, 240, 1.0),
    ("gain_minus20db", 48000, 8.0, None, 0, 0.1),
)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--temporary", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--model-sha256", required=True)
    parser.add_argument("--dictionary", type=Path, required=True)
    parser.add_argument("--air-archive", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--budget", type=int, default=64)
    args = parser.parse_args()
    if args.output.exists():
        raise ValueError("preserve existing evaluation; choose a new output directory")
    args.output.mkdir(parents=True)
    model = Surrogate(args.model, args.model_sha256)
    protocol = {
        "model_sha256": model.sha256,
        "dictionary_sha256": hashlib.sha256(args.dictionary.read_bytes()).hexdigest(),
        "synthetic_conditions": SYNTHETIC_CONDITIONS,
        "synthetic_seed": 20268025,
        "synthetic_controls": [0.5, 0.5, 0.5, 0.5, 0.2, 0.5],
        "layout": "5.1",
        "native_budget_per_method": args.budget,
        "extra_validation_renders_per_admitted_synthetic_case": 2,
        "air_cases": AIR_CASES if args.air_archive else [],
        "arrival_uncertainty_samples_48k": [-24, 0, 24],
        "selection": "fixed cases/controls/annotations before fits; no retuning",
        "raw_recording_export": False,
        "automatic_apply": False,
        "scope": "finite-window normalized energy only; no spatial/perceptual acceptance",
        "source_sha256": {
            name: hashlib.sha256(
                (Path(__file__).parent / name).read_bytes()
            ).hexdigest()
            for name in [
                "aae_room_features.py",
                "aae_room_model.py",
                "fit_aae_room.py",
                Path(__file__).name,
            ]
        },
    }
    archive = None
    if args.air_archive:
        sha = hashlib.sha256(args.air_archive.read_bytes()).hexdigest()
        if sha != AIR_SHA256:
            raise ValueError("AIR archive hash mismatch")
        protocol["air_archive_sha256"] = sha
        protocol["air_archive_url"] = (
            "https://www2.iks.rwth-aachen.de/air/air_database_release_1_4.zip"
        )
        protocol["air_license_note"] = (
            "Current primary project page names MIT; historical pinned archive has no license file. "
            "Archive remains external, is used only for evaluation, and is not redistributed or trained on."
        )
        archive = zipfile.ZipFile(args.air_archive)
    (args.output / "protocol.json").write_text(json.dumps(protocol, indent=2) + "\n")
    rows = []
    with Renderer(args.renderer, args.temporary) as renderer:
        protocol["renderer_sha256"] = renderer.binary_sha256
        (args.output / "protocol.json").write_text(
            json.dumps(protocol, indent=2) + "\n"
        )

        def evaluate(identity, audio, rate, direct_sample, provenance, clean=None):
            row = {
                "case": identity,
                "rate": rate,
                "direct_sample": direct_sample,
                "provenance": provenance,
                "input_array_sha256": hashlib.sha256(
                    np.asarray(audio, dtype="<f8").tobytes()
                ).hexdigest(),
            }
            try:
                target = describe(audio, rate, direct_sample=direct_sample)
                row["target_features"] = target["features"].tolist()
                row["target_mask"] = target["mask"].tolist()
                row["observed_seconds"] = target["observed_seconds"]
                row["target_t20_fits"] = target["t20_fits"]
                for method in (
                    ["classical", "neural"]
                    if len(rows) % 2 == 0
                    else ["neural", "classical"]
                ):
                    row[method] = fit(
                        target,
                        renderer,
                        layout="5.1",
                        rate=rate,
                        budget=args.budget,
                        dictionary=args.dictionary,
                        model=model if method == "neural" else None,
                    )
                    if clean is not None:
                        params = row[method]["best"]["params"]
                        validation, _ = renderer.render(
                            actual_unit(params),
                            "5.1",
                            params["room_preset"],
                            rate=rate,
                            seconds=8,
                        )
                        features = describe(validation, rate)["features"]
                        row[method]["clean_full_window_rmse_db"] = float(
                            np.sqrt(np.mean((features - clean["features"]) ** 2))
                        )
                selected = min(
                    ["classical", "neural"],
                    key=lambda name: row[name]["profile_rmse_db"],
                )
                row["selected"] = selected
                row["qualified_profile_proxy"] = row[selected][
                    "qualified_profile_proxy"
                ]
            except ValueError as error:
                row["admission_error"] = str(error)
                row["qualified_profile_proxy"] = False
            (args.output / f"case-{len(rows):03d}.json").write_text(
                json.dumps(row, indent=2, allow_nan=False) + "\n"
            )
            rows.append(row)
            print(
                json.dumps(
                    {
                        "case": identity,
                        "qualified": row["qualified_profile_proxy"],
                        "error": row.get("admission_error"),
                        **{
                            method: row[method]["profile_rmse_db"]
                            for method in ["classical", "neural"]
                            if method in row
                        },
                    }
                ),
                flush=True,
            )

        unit = np.array(protocol["synthetic_controls"])
        clean_responses = {}
        for name, rate, seconds, snr, arrival, gain in SYNTHETIC_CONDITIONS:
            if rate not in clean_responses:
                audio, metadata = renderer.render(unit, "5.1", "medium", rate=rate)
                clean_responses[rate] = (audio, metadata, describe(audio, rate))
            full, metadata, clean = clean_responses[rate]
            audio = full[: round(seconds * rate)].copy() * gain
            if snr is not None:
                noise = np.random.default_rng(20268025).normal(size=audio.shape)
                scale = np.sqrt(
                    np.mean(audio * audio) / np.mean(noise * noise)
                ) * 10 ** (-snr / 20)
                audio += noise * scale
            evaluate(
                "synthetic:" + name,
                audio,
                rate,
                arrival,
                {"native": metadata, "snr_db": snr, "gain": gain},
                clean,
            )
        if archive is not None:
            with archive:
                for name, annotation in AIR_CASES:
                    member = f"AIR_1_4/air_binaural_{name}.mat"
                    raw = archive.read(member)
                    data = loadmat(io.BytesIO(raw), simplify_cells=True)
                    audio = np.asarray(data["h_air"], dtype=np.float64)
                    rate = int(data["air_info"]["fs"])
                    if audio.ndim != 1 or rate != 48000:
                        raise ValueError("unexpected measured AIR geometry")
                    provenance = {
                        "member": member,
                        "sha256": hashlib.sha256(raw).hexdigest(),
                        "room": data["air_info"]["room"],
                        "arrival_annotation": annotation,
                        "annotation_basis": "first substantial direct arrival; approximate, not dataset ground truth",
                        "acquisition": "published AIR measured room impulse response; original h_air doubles, no added fade/denoising",
                    }
                    for offset in [-24, 0, 24]:
                        evaluate(
                            f"measured:{name}:arrival{offset:+d}",
                            audio,
                            rate,
                            annotation + offset,
                            provenance,
                        )
    summary = {
        "cases": len(rows),
        "admitted": sum("admission_error" not in row for row in rows),
        "qualified_profile_proxy": sum(row["qualified_profile_proxy"] for row in rows),
        "rows": [
            {
                k: v
                for k, v in row.items()
                if k
                in ["case", "selected", "qualified_profile_proxy", "admission_error"]
            }
            for row in rows
        ],
        "no_acceptance_or_training_change": True,
    }
    (args.output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary), flush=True)


if __name__ == "__main__":
    main()
