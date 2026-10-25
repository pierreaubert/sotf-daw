#!/usr/bin/env python3
"""Fit explicit paired Dynamic EQ references offline with actual native scoring."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from dynamic_eq_fit import controls, encode, make_preset, optimize
from dynamic_eq_native import render

FORMAT = "sotf-dynamic-eq-paired-target-v1"
RMS_LIMIT = 0.002
MAX_LIMIT = 0.01


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def read_verified(path, size, expected):
    with Path(path).open("rb") as f:
        raw = f.read(size + 1)
    if len(raw) != size or digest(raw) != expected:
        raise ValueError("Paired PCM geometry or SHA256 mismatch")
    x = np.frombuffer(raw, dtype="<f4")
    if not np.isfinite(x).all() or np.max(np.abs(x)) > 32:
        raise ValueError("Use finite PCM within +/-32")
    return x


def prepare(data):
    if (
        not isinstance(data, dict)
        or set(data)
        != {
            "format",
            "data_origin",
            "alignment",
            "sample_rate",
            "channels",
            "template",
            "clips",
        }
        or data["format"] != FORMAT
        or data["alignment"] != "sample_aligned_zero_latency_fresh_state"
    ):
        raise ValueError("Supply explicit fresh-state sample-aligned paired targets")
    if data["data_origin"] not in {"recorded_pairs", "synthetic_validation"}:
        raise ValueError("Declare paired data origin")
    rate, channels = data["sample_rate"], data["channels"]
    if (
        type(rate) is not int
        or not 44100 <= rate <= 192000
        or type(channels) is not int
        or not 1 <= channels <= 8
    ):
        raise ValueError("Unsupported rate/layout")
    template = data["template"]
    encode(template)
    if template["mix"] <= 0 or not any(
        b["active"] and abs(b["gain"]) >= 0.01 for b in template["bands"]
    ):
        raise ValueError("A nontrivial explicit EQ structure is required")
    if any(
        abs(b["band_threshold"] - template["threshold"]) > 0.001
        or abs(b["band_ratio"] - template["ratio"]) > 0.001
        for b in template["bands"]
    ):
        raise ValueError(
            "This initial fitter uses shared global dynamics; per-band overrides require a separate model"
        )
    clips = data["clips"]
    if not isinstance(clips, list) or not 3 <= len(clips) <= 32:
        raise ValueError("Supply 3..32 whole-source train/validation clips")
    rows = []
    groups = set()
    hashes = set()
    total = 0
    for clip in clips:
        if set(clip) != {
            "id",
            "role",
            "frames",
            "input_file",
            "input_sha256",
            "reference_file",
            "reference_sha256",
        }:
            raise ValueError("Unknown/missing clip fields")
        if (
            not isinstance(clip["id"], str)
            or not 1 <= len(clip["id"]) <= 128
            or clip["id"] in groups
            or clip["role"] not in {"train", "validation"}
        ):
            raise ValueError("Declare unique whole-source IDs and split roles")
        groups.add(clip["id"])
        if clip["input_sha256"] in hashes:
            raise ValueError("Duplicate input identity across source groups")
        hashes.add(clip["input_sha256"])
        if type(clip["frames"]) is not int or clip["frames"] < rate // 4:
            raise ValueError("Each paired clip must span at least 250 ms")
        count = clip["frames"] * channels
        total += count
        if total > 4_000_000:
            raise ValueError("Aggregate input exceeds 4 million samples")
        x = read_verified(clip["input_file"], count * 4, clip["input_sha256"]).reshape(
            -1, channels
        )
        reference = read_verified(
            clip["reference_file"], count * 4, clip["reference_sha256"]
        ).reshape(-1, channels)
        if float(np.mean(x.astype(float) ** 2)) <= 1e-8:
            raise ValueError("Input excitation is too quiet")
        rows.append((clip, x, reference))
    if sum(c[0]["role"] == "train" for c in rows) < 2 or not any(
        c[0]["role"] == "validation" for c in rows
    ):
        raise ValueError(
            "Require at least 2 training sources and a disjoint validation source"
        )
    return rows


def metrics(x, y, reference):
    error = y.astype(float) - reference
    return {
        "relative_rms": float(
            np.sqrt(np.mean(error * error) / np.mean(x.astype(float) ** 2))
        ),
        "max_abs": float(np.max(abs(error))),
    }


def run(data, renderer, output, budget=160, method="classical"):
    if (
        type(budget) is not int
        or not 40 <= budget <= 256
        or method not in {"classical", "surrogate"}
    ):
        raise ValueError("Use declared solver and 40..256 proposal budget")
    with Path(renderer).open("rb") as executable:
        binary = executable.read(128 * 1024 * 1024 + 1)
    if not binary or len(binary) > 128 * 1024 * 1024:
        raise ValueError("Native renderer exceeds binary size bound")
    renderer_hash = digest(binary)
    clips = prepare(data)
    output = Path(output)
    output.mkdir(parents=True, exist_ok=False)
    train = [r for r in clips if r[0]["role"] == "train"]
    template = data["template"]
    rate = data["sample_rate"]
    counter = 0

    def objective(x):
        nonlocal counter
        params = make_preset(template, x)
        values = []
        for i, (_, audio, reference) in enumerate(train):
            y, _ = render(
                audio,
                renderer,
                output / "search" / f"evaluation-{counter}-clip-{i}",
                params,
                rate=rate,
            )
            values.append(metrics(audio, y, reference)["relative_rms"] ** 2)
        counter += 1
        return float(np.mean(values))

    search = optimize(objective, encode(template), budget, method)
    selected = search["best"]
    params = make_preset(template, selected["x"])
    reasons = []
    checks = []
    native_params = None
    # Search sees training only. Validation is inspected once after selection.
    for i, (clip, audio, reference) in enumerate(clips):
        y, meta = render(
            audio, renderer, output / "qualification" / str(i), params, rate=rate
        )
        current = metrics(audio, y, reference)
        baseline = metrics(audio, audio, reference)
        if current["relative_rms"] > RMS_LIMIT or current["max_abs"] > MAX_LIMIT:
            reasons.append(clip["role"] + "_waveform_mismatch")
        if (
            baseline["relative_rms"] - current["relative_rms"] < 0.001
            or current["relative_rms"] > 0.5 * baseline["relative_rms"]
        ):
            reasons.append("no_material_advantage_over_dry")
        native_params = meta["requested_params"]
        reopened, _ = render(
            audio,
            renderer,
            output / "reopen" / str(i),
            native_params,
            rate=rate,
            callback=17,
        )
        reopen_error = float(np.max(abs(reopened.astype(float) - y)))
        if reopen_error > 1e-7:
            reasons.append("preset_reopening_mismatch")
        checks.append(
            {
                "clip_id": clip["id"],
                "role": clip["role"],
                "metrics": current,
                "dry_baseline": baseline,
                "reopen_max_error": reopen_error,
            }
        )
    # Probe local sensitivity in normalized coordinates using training only.
    # This screens observed ambiguities; it is not an identifiability proof.
    probes = []
    best_x = np.asarray(selected["x"])
    best_rms = np.sqrt(selected["loss"])
    for dimension in range(4):
        for direction in [-1, 1]:
            point = best_x.copy()
            point[dimension] = np.clip(point[dimension] + direction * 0.1, 0, 1)
            if np.max(abs(point - best_x)) < 0.099:
                continue
            loss = objective(point)
            probes.append(
                {"x": point.tolist(), "controls": controls(point), "loss": loss}
            )
    competitors = search["trace"] + probes
    if any(
        np.max(abs(np.asarray(row["x"]) - best_x)) >= 0.099
        and np.sqrt(row["loss"]) <= best_rms + 0.0005
        for row in competitors
    ):
        reasons.append("observed_parameter_ambiguity")
    if digest(Path(renderer).read_bytes()) != renderer_hash:
        raise ValueError("Native renderer changed during fitting")
    report = {
        "data_origin": data["data_origin"],
        "accepted": not reasons,
        "parameters": native_params if not reasons else None,
        "reasons": sorted(set(reasons)),
        "search": search,
        "sensitivity_probes": probes,
        "native_objective_calls": counter,
        "native_render_calls": counter * len(train) + 2 * len(clips),
        "training_clips_per_objective": len(train),
        "qualification": checks,
        "renderer_sha256": renderer_hash,
        "scope": "Shared global threshold/ratio/attack/release with explicit frozen band structures, knee/mix/link/routing. Paired clip waveform validation only; no learned runtime envelope, calibrated uncertainty or perceptual/generalization claim.",
    }
    (output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    if report["accepted"]:
        (output / "preset.json").write_text(
            json.dumps(native_params, indent=2, allow_nan=False) + "\n"
        )
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", type=Path, required=True)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--method", choices=["classical", "surrogate"], default="classical"
    )
    parser.add_argument("--budget", type=int, default=160)
    args = parser.parse_args()
    with args.target.open("rb") as f:
        raw = f.read(2 * 1024 * 1024 + 1)
    if len(raw) > 2 * 1024 * 1024:
        raise ValueError("Target exceeds 2 MiB")
    report = run(json.loads(raw), args.renderer, args.output, args.budget, args.method)
    report["target_sha256"] = digest(raw)
    (args.output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    print(
        json.dumps(
            {
                "accepted": report["accepted"],
                "reasons": report["reasons"],
                "method": args.method,
                "native_objective_calls": report["native_objective_calls"],
            }
        )
    )
    if not report["accepted"]:
        raise SystemExit(2)


if __name__ == "__main__":
    main()
