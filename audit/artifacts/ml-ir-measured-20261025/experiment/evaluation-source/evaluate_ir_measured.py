"""Frozen IR-mask evaluation on paired AIR recordings with added controlled noise.

The reference includes the original recording's noise/artifacts. This tests
recovery of added noise, not unknown clean-room ground truth. Six room pairs,
36 unique noisy measurements and 108 correlated arrival variants are reported.
No external recording is trained on or committed; arrays stay in temporary storage.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import zipfile
from pathlib import Path

import numpy as np
from ir_denoise import load_model, propose, sha
from ir_measured_metrics import decay_error, transfer_error
from ir_preservation import load_policy
from scipy.io import loadmat
from scipy.signal import lfilter
from train_ir_denoise import metrics

AIR_SHA256 = "4baa3a0ea31dc72647ff61f53b0bf0b962876de31b6b0569ebc504987898aa02"
ROOMS = (
    ("booth_0_0_1", 68),
    ("office_0_0_1", 146),
    ("meeting_0_0_1", 206),
    ("lecture_0_0_1", 320),
    ("stairway_0_1_1_90", 138),
    ("aula_carolina_0_1_1_90_3", 153),
)
METHODS = ("neural", "wiener", "decay")


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, allow_nan=False) + "\n")


def store(path, values, dtype="<f4"):
    array = np.asarray(values, dtype=dtype)
    path.write_bytes(array.tobytes())
    return {
        "path": str(path.resolve()),
        "sha256": sha(path),
        "values": array.size,
        "dtype": dtype,
    }


def load_pair(archive, name):
    names = [name, name.replace("_0_", "_1_", 1)]
    channels, provenance = [], []
    for expected_channel, identity in enumerate(names):
        member = f"AIR_1_4/air_binaural_{identity}.mat"
        if archive.getinfo(member).file_size > 16 * 1024 * 1024:
            raise ValueError("unexpected AIR member size")
        raw = archive.read(member)
        data = loadmat(io.BytesIO(raw), simplify_cells=True)
        vector = np.asarray(data["h_air"], dtype=float)
        info = data["air_info"]
        if (
            vector.ndim != 1
            or not np.isfinite(vector).all()
            or not 1 < len(vector) < 1400000
            or int(info["fs"]) != 48000
            or int(info["channel"]) != expected_channel
        ):
            raise ValueError("unexpected paired AIR geometry")
        channels.append(vector)
        provenance.append(
            {
                "member": member,
                "member_sha256": hashlib.sha256(raw).hexdigest(),
                "h_air_f64_sha256": hashlib.sha256(
                    np.asarray(vector, dtype="<f8").tobytes()
                ).hexdigest(),
                "channel": expected_channel,
                "room": str(info["room"]),
                "frames": len(vector),
                "sample_rate": 48000,
            }
        )
    if len(channels[0]) != len(channels[1]):
        raise ValueError("AIR pair length mismatch")
    return np.column_stack(channels), provenance


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--frozen-experiment", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--temporary", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists() or args.temporary.exists():
        raise ValueError("preserve previous evidence/arrays")
    if sha(args.archive) != AIR_SHA256:
        raise ValueError("pinned AIR archive identity mismatch")
    previous = json.loads((args.frozen_experiment / "summary.json").read_text())
    model = load_model(args.model, previous["model_sha256"])
    policies = {
        name: load_policy(
            args.frozen_experiment / (name + "-policy.json"),
            previous["policy_sha256"][name],
        )
        for name in METHODS
    }
    args.output.mkdir(parents=True)
    args.temporary.mkdir(parents=True)
    protocol = {
        "format": "sotf_ir_measured_noise_probe_v1",
        "archive_sha256": AIR_SHA256,
        "archive_url": "https://www2.iks.rwth-aachen.de/air/air_database_release_1_4.zip",
        "archive_attribution": "Aachen Impulse Response Database v1.4, RWTH Aachen University, Institute of Communication Systems and Data Processing; Marco Jeub; archive readme copyright 2011.",
        "license_status": "Legacy archive has no license file; reconcile terms before redistribution. No raw recording or derived audio is committed or used for training.",
        "rooms": ROOMS,
        "model_sha256": previous["model_sha256"],
        "policies": policies,
        "policy_sha256": previous["policy_sha256"],
        "snr_db": [20, 40, 60],
        "noise_types": ["white", "colored_ar_095"],
        "arrival_offsets_samples": [-24, 0, 24],
        "sample_rate": 48000,
        "prefix_frames": 3840,
        "noise_reference_frames": 2880,
        "seed": 20283025,
        "unique_recorded_room_pairs": 6,
        "unique_noisy_measurements": 36,
        "cases": 108,
        "selection": "Same six preselected AIR rooms as earlier AAE investigation; not globally untouched data. Never used for this model's synthetic-only training. Model/policies frozen before this probe; no tuning from results.",
        "reference": "Original h_air doubles paired by channel, cast to float32 and prefixed with 80 ms zeros. Recording noise/artifacts remain in target. No normalization, resampling, trimming or denoising of the recorded reference.",
        "scope": "Measured IR structure with synthetic added noise; not clean-room ground truth, acoustic calibration, acquisition-noise validation, independent arrival annotations or listening acceptance. Three arrival variants share each noisy source and are not independent measurements.",
        "arrival_basis": "Prior approximate first substantial channel-0 arrival, tested +/-0.5 ms; not dataset ground truth.",
        "metrics": "Waveform reconstruction SNR; normalized backward-energy shape at reference >= -40dB; 192 log-grid complex response points >= -20dB/channel peak. No response alignment or level fitting.",
        "source_sha256": {
            name: sha(Path(__file__).parent / name)
            for name in [
                Path(__file__).name,
                "ir_measured_metrics.py",
                "ir_denoise.py",
                "ir_preservation.py",
                "train_ir_denoise.py",
            ]
        },
    }
    write_json(args.output / "protocol.json", protocol)
    snapshot = args.output / "evaluation-source"
    snapshot.mkdir()
    for name in protocol["source_sha256"]:
        (snapshot / name).write_bytes((Path(__file__).parent / name).read_bytes())
    rows, identities, reference_records = [], set(), []
    with zipfile.ZipFile(args.archive) as archive:
        for room_index, (name, annotation) in enumerate(ROOMS):
            original, provenance = load_pair(archive, name)
            clean = np.pad(original, ((protocol["prefix_frames"], 0), (0, 0))).astype(
                "<f4"
            )
            reference_path = args.temporary / f"reference-{room_index:02d}.f32le"
            reference = store(reference_path, clean)
            reference_records.append(
                {
                    "room": name,
                    "reference": reference,
                    "provenance": provenance,
                    "frames": len(clean),
                    "channels": 2,
                }
            )
            for condition, (snr, noise_type) in enumerate(
                (s, n) for s in protocol["snr_db"] for n in protocol["noise_types"]
            ):
                seed = protocol["seed"] + room_index * 6 + condition
                rng = np.random.default_rng(seed)
                noise = rng.normal(size=(len(clean) + 48000, 2))
                if noise_type != "white":
                    noise = lfilter([1], [1, -0.95], noise, axis=0)
                noise = noise[48000:]
                noise *= np.sqrt(
                    np.sum(clean.astype(float) ** 2)
                    / (10 ** (snr / 10) * np.sum(noise**2))
                )
                noisy = (clean.astype(float) + noise).astype("<f4")
                noisy_path = (
                    args.temporary / f"source-{room_index:02d}-{condition}.f32le"
                )
                source = store(noisy_path, noisy)
                if source["sha256"] in identities:
                    raise ValueError("duplicate noisy measurement")
                identities.add(source["sha256"])
                for offset in protocol["arrival_offsets_samples"]:
                    index = len(rows)
                    arrival = protocol["prefix_frames"] + annotation + offset
                    directory = args.temporary / f"case-{index:03d}"
                    directory.mkdir()
                    row = {
                        "index": index,
                        "room": name,
                        "room_index": room_index,
                        "condition": condition,
                        "seed": seed,
                        "nominal_added_snr_db": snr,
                        "noise_type": noise_type,
                        "arrival_offset_samples": offset,
                        "arrival": arrival,
                        "noise_frames": protocol["noise_reference_frames"],
                        "frames": len(clean),
                        "channels": 2,
                        "reference": reference,
                        "source": source,
                        "arrays": {},
                        "methods": {},
                    }
                    for method in ["unchanged", *METHODS]:
                        if method == "unchanged":
                            output, checks, mask = (
                                noisy,
                                {"accepted": True},
                                np.ones(len(clean)),
                            )
                        else:
                            try:
                                output, checks, mask = propose(
                                    noisy,
                                    48000,
                                    arrival,
                                    protocol["noise_reference_frames"],
                                    model,
                                    method,
                                    preservation_policy=policies[method],
                                )
                            except ValueError as error:
                                output, checks, mask = (
                                    noisy.copy(),
                                    {"accepted": False, "fallback_error": str(error)},
                                    np.ones(len(clean)),
                                )
                            row["arrays"][method] = store(
                                directory / (method + ".f32le"), output
                            )
                            row["arrays"][method + "_mask"] = store(
                                directory / (method + "_mask.f64le"), mask, "<f8"
                            )
                        effective = (
                            mask
                            if method == "unchanged" or checks["accepted"]
                            else np.ones(len(mask))
                        )
                        conditional = clean.astype(float) * effective[:, None]
                        row["methods"][method] = {
                            "checks": checks,
                            "waveform": metrics(clean, noisy, output),
                            "decay": decay_error(clean, output, 48000, arrival),
                            "transfer": transfer_error(clean, output, 48000),
                            "conditional_decay": decay_error(
                                clean, conditional, 48000, arrival
                            ),
                            "conditional_projection_loss_db": float(
                                -20
                                * np.log10(
                                    max(
                                        float(
                                            np.sum(
                                                clean.astype(float) ** 2
                                                * effective[:, None]
                                            )
                                            / np.sum(clean.astype(float) ** 2)
                                        ),
                                        1e-15,
                                    )
                                )
                            ),
                        }
                    write_json(args.output / f"case-{index:03d}.json", row)
                    rows.append(row)
            print(
                json.dumps({"room": name, "frames": len(clean), "cases": 18}),
                flush=True,
            )
    write_json(args.output / "references.json", reference_records)
    summary = {
        "cases": len(rows),
        "rooms": len(reference_records),
        "unique_noisy_measurements": len(identities),
        "methods": {},
        "room_results": [],
    }
    for method in METHODS:
        summary["methods"][method] = {
            "accepted": sum(r["methods"][method]["checks"]["accepted"] for r in rows),
            "median_improvement_db": float(
                np.median(
                    [r["methods"][method]["waveform"]["improvement_db"] for r in rows]
                )
            ),
            "worst_improvement_db": min(
                r["methods"][method]["waveform"]["improvement_db"] for r in rows
            ),
            "max_conditional_decay_error_db": max(
                r["methods"][method]["conditional_decay"]["max_abs_db"] for r in rows
            ),
        }
    summary["neural_better_than_best_classical_cases"] = sum(
        r["methods"]["neural"]["waveform"]["snr_db"]
        > max(r["methods"][m]["waveform"]["snr_db"] for m in ["wiener", "decay"]) + 1e-6
        for r in rows
    )
    for name, _ in ROOMS:
        subset = [r for r in rows if r["room"] == name]
        summary["room_results"].append(
            {
                "room": name,
                "median_improvement_db": {
                    m: float(
                        np.median(
                            [
                                r["methods"][m]["waveform"]["improvement_db"]
                                for r in subset
                            ]
                        )
                    )
                    for m in METHODS
                },
                "worst_neural_improvement_db": min(
                    r["methods"]["neural"]["waveform"]["improvement_db"] for r in subset
                ),
            }
        )
    write_json(args.output / "summary.json", summary)
    print(json.dumps(summary), flush=True)


if __name__ == "__main__":
    main()
