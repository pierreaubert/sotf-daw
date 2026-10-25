"""Independently reduce measured-reference IR evidence and reconstruct its sources."""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import zipfile
from pathlib import Path

import numpy as np
from scipy.fft import fft
from scipy.io import loadmat
from scipy.signal import lfilter


def read_json(path):
    return json.loads(path.read_text())


def read_array(record):
    raw = Path(record["path"]).read_bytes()
    if hashlib.sha256(raw).hexdigest() != record["sha256"]:
        raise ValueError("array hash mismatch")
    values = np.frombuffer(raw, dtype=record["dtype"]).astype(float)
    if len(values) != record["values"] or not np.isfinite(values).all():
        raise ValueError("array geometry/nonfinite")
    return values


def decay(reference, output, rate, arrival):
    count = len(reference) - arrival
    times = np.geomspace(max(1, round(rate / 200)), count - 1, 64)
    indices = arrival + np.unique(np.append(0, np.rint(times).astype(int)))
    target = np.flip(np.cumsum(np.flip(np.sum(reference**2, axis=1))))
    observed = np.flip(np.cumsum(np.flip(np.sum(output**2, axis=1))))
    levels = 10 * np.log10(
        np.maximum(target[indices] / max(float(target[arrival]), 1e-30), 1e-30)
    )
    selected = levels >= -40
    difference = (
        10
        * np.log10(
            np.maximum(
                observed[indices][selected] / max(float(observed[arrival]), 1e-30),
                1e-30,
            )
        )
        - levels[selected]
    )
    return {
        "rmse_db": float(np.sqrt(np.sum(difference**2) / len(difference))),
        "max_abs_db": float(np.max(np.abs(difference))),
        "points": len(difference),
    }


def transfer(reference, output, rate):
    length = 2 ** int(np.ceil(np.log2(len(reference))))
    indices = np.unique(
        np.ceil(np.geomspace(40, min(16000, rate * 0.45), 192) * length / rate).astype(
            int
        )
    )
    target = fft(reference, n=length, axis=0)[indices]
    modified = fft(output, n=length, axis=0)[indices]
    selected = np.abs(target) >= 0.1 * np.max(np.abs(target), axis=0, keepdims=True)
    ratio = modified / np.where(np.abs(target) > 1e-30, target, 1)
    magnitude = 20 * np.log10(np.maximum(np.abs(ratio[selected]), 1e-30))
    angle = np.angle(ratio[selected]) * 180 / np.pi
    return {
        "magnitude_rmse_db": float(np.sqrt(np.sum(magnitude**2) / len(magnitude))),
        "phase_rmse_degrees": float(np.sqrt(np.sum(angle**2) / len(angle))),
        "magnitude_max_abs_db": float(np.max(np.abs(magnitude))),
        "phase_max_abs_degrees": float(np.max(np.abs(angle))),
        "points": len(angle),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        raise ValueError("preserve prior independent analysis")
    protocol = read_json(args.evidence / "protocol.json")
    if (
        hashlib.sha256(args.archive.read_bytes()).hexdigest()
        != protocol["archive_sha256"]
    ):
        raise ValueError("archive identity mismatch")
    for name, expected in protocol["source_sha256"].items():
        if (
            hashlib.sha256(
                (args.evidence / "evaluation-source" / name).read_bytes()
            ).hexdigest()
            != expected
        ):
            raise ValueError("frozen evaluation source mismatch")
    references = read_json(args.evidence / "references.json")
    with zipfile.ZipFile(args.archive) as archive:
        for room in references:
            channels = []
            for info in room["provenance"]:
                raw = archive.read(info["member"])
                if hashlib.sha256(raw).hexdigest() != info["member_sha256"]:
                    raise ValueError("MAT member identity mismatch")
                data = loadmat(io.BytesIO(raw), simplify_cells=True)
                vector = np.asarray(data["h_air"], dtype="<f8")
                if (
                    hashlib.sha256(vector.tobytes()).hexdigest()
                    != info["h_air_f64_sha256"]
                    or int(data["air_info"]["channel"]) != info["channel"]
                ):
                    raise ValueError("source channel identity mismatch")
                channels.append(vector)
            reference = np.pad(
                np.stack(channels, axis=1), ((protocol["prefix_frames"], 0), (0, 0))
            ).astype("<f4")
            if (
                hashlib.sha256(reference.tobytes()).hexdigest()
                != room["reference"]["sha256"]
            ):
                raise ValueError("recorded reference transformation mismatch")
    known_sources, source_cache = {}, {}
    metric_error, budget_max = 0.0, 0.0
    rows = []
    for index in range(108):
        row = read_json(args.evidence / f"case-{index:03d}.json")
        clean = read_array(row["reference"]).reshape(row["frames"], 2)
        noisy = read_array(row["source"]).reshape(row["frames"], 2)
        source_key = row["source"]["sha256"]
        room_info = references[row["room_index"]]
        expected_arrival = (
            protocol["prefix_frames"]
            + protocol["rooms"][row["room_index"]][1]
            + row["arrival_offset_samples"]
        )
        if (
            row["room"] != room_info["room"]
            or row["arrival"] != expected_arrival
            or row["reference"] != room_info["reference"]
        ):
            raise ValueError("case/reference/arrival mismatch")
        condition = (row["room_index"], row["condition"])
        if source_key not in known_sources:
            rng = np.random.default_rng(row["seed"])
            noise = rng.standard_normal((len(clean) + 48000, 2))
            if row["noise_type"] != "white":
                noise = lfilter([1], [1, -0.95], noise, axis=0)
            noise = noise[48000:]
            noise *= np.sqrt(
                np.sum(clean**2)
                / (10 ** (row["nominal_added_snr_db"] / 10) * np.sum(noise**2))
            )
            regenerated = (clean + noise).astype("<f4")
            if not np.array_equal(regenerated, noisy):
                raise ValueError("controlled noise generation mismatch")
            known_sources[source_key] = condition
        elif known_sources[source_key] != condition:
            raise ValueError("unexpected repeated source")
        source_cache.setdefault(condition, set()).add(row["arrival_offset_samples"])
        if set(row["methods"]) != {"unchanged", "neural", "wiener", "decay"}:
            raise ValueError("method coverage mismatch")
        protected = row["arrival"] + round(0.05 * 48000)
        for method, details in row["methods"].items():
            if method == "unchanged":
                output, effective = noisy, np.ones(len(noisy))
            else:
                output = read_array(row["arrays"][method]).reshape(noisy.shape)
                mask = read_array(row["arrays"][method + "_mask"])
                if (
                    len(mask) != len(noisy)
                    or np.any(mask < 0.05 - 1e-12)
                    or np.any(mask > 1 + 1e-12)
                    or not np.array_equal(mask[:protected], np.ones(protected))
                ):
                    raise ValueError("invalid linked mask")
                candidate = (noisy * mask[:, None]).astype("<f4").astype(float)
                checks = details["checks"]
                if checks["accepted"]:
                    if not np.array_equal(candidate, output):
                        raise ValueError("candidate reconstruction mismatch")
                    effective = mask
                else:
                    if not np.array_equal(output, noisy):
                        raise ValueError("fallback differs from original")
                    effective = np.ones(len(mask))
                if "fallback_error" not in checks:
                    policy = protocol["policies"][method]
                    width = round(policy["window_ms"] * 48)
                    starts = np.arange(protected, len(noisy), width)
                    lengths = np.minimum(width, len(noisy) - starts)
                    noise_power = (
                        np.sum(noisy[: row["noise_frames"]] ** 2, axis=0)
                        / row["noise_frames"]
                    )
                    removed = np.add.reduceat(
                        (candidate[protected:] - noisy[protected:]) ** 2,
                        starts - protected,
                        axis=0,
                    )
                    budget = (
                        lengths[:, None]
                        * noise_power[None, :]
                        * policy["noise_fraction"]
                    )
                    ratio = np.divide(
                        removed, budget, out=np.zeros_like(removed), where=budget > 0
                    )
                    maximum = float(np.max(ratio))
                    budget_max = max(budget_max, maximum)
                    if (
                        np.any((budget == 0) & (removed > 0))
                        or maximum > 1.00001
                        or abs(
                            maximum - checks["preservation"]["max_removed_noise_ratio"]
                        )
                        > 1e-9
                        or not checks["preservation"]["within_budget"]
                    ):
                        raise ValueError("removal budget mismatch")
                    response = transfer(noisy, candidate, 48000)
                    relative = float(
                        np.sqrt(np.sum((candidate - noisy) ** 2) / np.sum(noisy**2))
                    )
                    accepted = (
                        relative <= 0.05
                        and response["magnitude_max_abs_db"] <= 1
                        and response["phase_max_abs_degrees"] <= 10
                    )
                    if accepted != checks["accepted"]:
                        raise ValueError("response admission mismatch")
                if not np.array_equal(output[:protected], noisy[:protected]):
                    raise ValueError("early response changed")
            energy = max(float(np.sum(clean**2)), 1e-30)
            residual = max(float(np.sum((output - clean) ** 2)), 1e-30)
            initial = max(float(np.sum((noisy - clean) ** 2)), 1e-30)
            waveform = {
                "snr_db": float(10 * np.log10(energy / residual)),
                "improvement_db": float(10 * np.log10(initial / residual)),
                "projection_loss_db": float(
                    -20 * np.log10(max(float(np.sum(output * clean)) / energy, 1e-15))
                ),
            }
            calculated = {
                "waveform": waveform,
                "decay": decay(clean, output, 48000, row["arrival"]),
                "transfer": transfer(clean, output, 48000),
                "conditional_decay": decay(
                    clean, clean * effective[:, None], 48000, row["arrival"]
                ),
            }
            for category, values in calculated.items():
                for name, value in values.items():
                    difference = abs(value - details[category][name])
                    metric_error = max(metric_error, difference)
                    if difference > 1e-7:
                        raise ValueError(
                            f"metric mismatch: {index}/{method}/{category}/{name}/{difference}"
                        )
            projection = float(
                -20
                * np.log10(
                    max(float(np.sum(clean**2 * effective[:, None])) / energy, 1e-15)
                )
            )
            if abs(projection - details["conditional_projection_loss_db"]) > 1e-9:
                raise ValueError("conditional projection mismatch")
        rows.append(row)
    if len(known_sources) != 36 or any(
        offsets != {-24, 0, 24} for offsets in source_cache.values()
    ):
        raise ValueError("source/arrival coverage mismatch")
    original_summary = read_json(args.evidence / "summary.json")
    recomputed = {}
    for method in ["neural", "wiener", "decay"]:
        recomputed[method] = {
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
        if recomputed[method] != original_summary["methods"][method]:
            raise ValueError("summary mismatch")
    wins = sum(
        r["methods"]["neural"]["waveform"]["snr_db"]
        > max(r["methods"][m]["waveform"]["snr_db"] for m in ["wiener", "decay"]) + 1e-6
        for r in rows
    )
    if wins != original_summary["neural_better_than_best_classical_cases"]:
        raise ValueError("classical comparison mismatch")
    worst = max(
        rows, key=lambda r: r["methods"]["neural"]["conditional_decay"]["max_abs_db"]
    )
    grouped_deltas = []
    for condition in source_cache:
        group = [
            row for row in rows if (row["room_index"], row["condition"]) == condition
        ]
        medians = {
            method: float(
                np.median(
                    [
                        row["methods"][method]["waveform"]["improvement_db"]
                        for row in group
                    ]
                )
            )
            for method in ["neural", "wiener", "decay"]
        }
        grouped_deltas.append(
            medians["neural"] - max(medians["wiener"], medians["decay"])
        )
    report = {
        "cases": len(rows),
        "recorded_room_pairs": len(references),
        "unique_added_noise_measurements": len(known_sources),
        "max_metric_error": metric_error,
        "max_removed_noise_ratio": budget_max,
        "methods": recomputed,
        "neural_better_than_best_classical_cases": wins,
        "unique_measurement_comparison": {
            "groups": len(grouped_deltas),
            "neural_wins": sum(delta > 1e-6 for delta in grouped_deltas),
            "median_delta_db": float(np.median(grouped_deltas)),
            "worst_delta_db": min(grouped_deltas),
            "best_delta_db": max(grouped_deltas),
            "aggregation": "Within-measurement medians across three correlated arrival variants; still only six recorded room pairs.",
        },
        "worst_conditional_decay": {
            "case": worst["index"],
            "room": worst["room"],
            "snr_db": worst["nominal_added_snr_db"],
            "noise_type": worst["noise_type"],
            "max_abs_db": worst["methods"]["neural"]["conditional_decay"]["max_abs_db"],
        },
        "adoption_recommended": False,
        "scope": protocol["scope"],
        "conclusion": "Small added-noise recovery gains do not prove measured decay preservation. Weak reference-profile regions change by up to 2.411 dB under conditional masks; actual clean-room/acquisition-noise and listening acceptance remain open.",
    }
    args.output.write_text(json.dumps(report, indent=2, allow_nan=False) + "\n")
    print(json.dumps(report))


if __name__ == "__main__":
    main()
