#!/usr/bin/env python3
"""Offline static compressor fitting with actual native per-band qualification."""

import argparse
import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np
from compressor_curve import Model, fit_curve, response, validate_target


def bounded_json(path, limit):
    with Path(path).open("rb") as source:
        raw = source.read(limit + 1)
    if len(raw) > limit:
        raise ValueError("Target exceeds JSON size bound")
    return json.loads(raw), hashlib.sha256(raw).hexdigest()


def validate(data):
    if (
        not isinstance(data, dict)
        or data.get("format") != "sotf-compressor-static-target-v1"
        or data.get("level_convention") != "constant_amplitude_dbfs"
    ):
        raise ValueError("Use the explicit constant-amplitude static-target format")
    if set(data) != {
        "format",
        "level_convention",
        "sample_rate",
        "crossovers_hz",
        "bands",
    }:
        raise ValueError("Unknown or missing target fields")
    if (
        type(data["sample_rate"]) is not int
        or not 44100 <= data["sample_rate"] <= 192000
    ):
        raise ValueError("Unsupported sample rate")
    bands = data["bands"]
    if not isinstance(bands, list) or not 1 <= len(bands) <= 5:
        raise ValueError("Supply one to five explicit band targets")
    crossovers = np.asarray(data["crossovers_hz"], dtype=np.float64)
    if (
        crossovers.shape != (len(bands) - 1,)
        or not np.isfinite(crossovers).all()
        or np.any(crossovers <= 0)
        or np.any(crossovers >= data["sample_rate"] / 2)
        or np.any(np.diff(crossovers) <= 0)
    ):
        raise ValueError("Supply ordered crossovers below Nyquist for every split")
    for band in bands:
        if set(band) != {"input_dbfs", "output_dbfs", "attack_ms", "release_ms"}:
            raise ValueError(
                "Each band requires static levels and explicit attack/release"
            )
        validate_target(band)
        for key, lower, upper in [("attack_ms", 0.1, 100), ("release_ms", 10, 1000)]:
            value = band[key]
            if (
                isinstance(value, bool)
                or not isinstance(value, (int, float))
                or not np.isfinite(value)
                or not lower <= value <= upper
            ):
                raise ValueError("Invalid explicit timing control: " + key)
    return data


def make_preset(target, fits):
    bands = []
    for source, fit in zip(target["bands"], fits):
        threshold, log_ratio, knee, makeup = fit["parameters"]
        bands.append(
            {
                "threshold_db": threshold,
                "ratio": float(np.exp(log_ratio)),
                "knee_db": knee,
                "makeup_gain_db": makeup,
                "attack_ms": source["attack_ms"],
                "release_ms": source["release_ms"],
                "auto_makeup": False,
                "measured_auto_makeup": False,
                "active": True,
                "solo": False,
                "bypass": False,
                "range_db": 120.0,
                "hold_ms": 0.0,
            }
        )
    return {
        "num_bands": len(bands),
        "crossover_preset": -1,
        "crossover_frequencies": target["crossovers_hz"],
        "threshold_db": bands[0]["threshold_db"],
        "ratio": bands[0]["ratio"],
        "knee_db": bands[0]["knee_db"],
        "attack_ms": bands[0]["attack_ms"],
        "release_ms": bands[0]["release_ms"],
        "range_db": 120.0,
        "hold_ms": 0.0,
        "mix": 1.0,
        "ms_mode": False,
        "link_channels": True,
        "link_amount": 1.0,
        "sidechain_tilt_db": 0.0,
        "per_band_lookahead_ms": 0.0,
        "bands": bands,
    }


def strip_controls(value):
    if isinstance(value, dict):
        return {
            k: None if k == "parameters" else strip_controls(v)
            for k, v in value.items()
        }
    if isinstance(value, list):
        return [strip_controls(v) for v in value]
    return value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", type=Path, required=True)
    parser.add_argument("--model", type=Path)
    parser.add_argument("--model-sha256")
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--max-evaluations", type=int, default=100)
    parser.add_argument("--callback-frames", type=int, default=193)
    args = parser.parse_args()
    if args.output.exists() or args.work.exists():
        raise ValueError("Use new output/work directories")
    if not 1 <= args.max_evaluations <= 500 or not 1 <= args.callback_frames <= 4096:
        raise ValueError("Unsupported fitting or callback budget")
    if bool(args.model) != bool(args.model_sha256):
        raise ValueError("Provide both model path and explicit SHA-256")
    data, target_digest = bounded_json(args.target, 262144)
    target = validate(data)
    model = Model(args.model, args.model_sha256) if args.model else None
    fits = [
        fit_curve(*validate_target(band), model, args.max_evaluations)
        for band in target["bands"]
    ]
    accepted = all(fit["accepted"] for fit in fits)
    result = {
        "accepted": False,
        "parameters": None,
        "target_sha256": target_digest,
        "model_sha256": None if model is None else model.digest,
        "sample_rate": target["sample_rate"],
        "band_fits": fits,
        "native": None,
        "reasons": [],
        "qualification": "Sampled static target and isolated native band transfer only; timing/layout copied explicitly, no temporal, hardware or listening match inferred",
    }
    if accepted:
        preset = make_preset(target, fits)
        args.work.mkdir(parents=True, exist_ok=False)
        request = {
            "sample_rate": target["sample_rate"],
            "callback_frames": args.callback_frames,
            "settle_multiple": 32.0,
            "params": preset,
            "levels_dbfs": [band["input_dbfs"] for band in target["bands"]],
        }
        request_path = args.work / "request.json"
        request_path.write_text(json.dumps(request, allow_nan=False) + "\n")
        try:
            process = subprocess.run(
                [str(args.renderer.resolve()), str(request_path)],
                capture_output=True,
                text=True,
                check=False,
                timeout=600,
            )
            if process.returncode != 0:
                raise ValueError("Native validation failed: " + process.stderr)
            native = json.loads(process.stdout)
            if (
                native["status"] != "ok"
                or len(native["curves"]) != len(fits)
                or native["sample_rate"] != target["sample_rate"]
                or native["callback_frames"] != args.callback_frames
                or native["settle_multiple"] != 32.0
                or native["full_preset_latency_samples"] != 0
            ):
                raise ValueError("Invalid native curve metadata")
            checks = []
            for index, (band, fit, curve) in enumerate(
                zip(target["bands"], fits, native["curves"])
            ):
                x, y = validate_target(band)
                actual = np.asarray(curve["output_dbfs"], dtype=np.float64)
                if (
                    curve["band"] != index
                    or curve["input_dbfs"] != band["input_dbfs"]
                    or actual.shape != y.shape
                    or not np.isfinite(actual).all()
                ):
                    raise ValueError("Native curve geometry mismatch")
                target_error, ideal_error = (
                    actual - y,
                    actual - response(fit["parameters"], x),
                )
                rms, maximum, ideal_max = (
                    float(np.sqrt(np.mean(target_error**2))),
                    float(np.max(np.abs(target_error))),
                    float(np.max(np.abs(ideal_error))),
                )
                checks.append(
                    {
                        "band": index,
                        "rms_db": rms,
                        "max_db": maximum,
                        "native_ideal_max_db": ideal_max,
                    }
                )
                if rms > 0.03 or maximum > 0.06 or ideal_max > 0.05:
                    accepted = False
            result["native"] = dict(native, checks=checks)
            if accepted:
                result.update(
                    accepted=True,
                    parameters=preset,
                    maximum_isolated_band_sampled_output_dbfs=max(
                        max(c["output_dbfs"]) for c in native["curves"]
                    ),
                )
                result["required_isolated_band_sampled_headroom_db"] = max(
                    0.0, result["maximum_isolated_band_sampled_output_dbfs"]
                )
            else:
                result["reasons"].append("native_static_accuracy_failed")
        except (
            OSError,
            ValueError,
            KeyError,
            TypeError,
            subprocess.TimeoutExpired,
        ) as error:
            result["reasons"].append(str(error))
    else:
        result["reasons"].append("sampled_curve_fit_failed")
    if not result["accepted"]:
        result = strip_controls(result)
    args.output.mkdir(parents=True, exist_ok=False)
    (args.output / "report.json").write_text(
        json.dumps(result, indent=2, allow_nan=False) + "\n"
    )
    if result["accepted"]:
        (args.output / "preset.json").write_text(
            json.dumps(result["parameters"], indent=2, allow_nan=False) + "\n"
        )
    print(json.dumps({"accepted": result["accepted"], "reasons": result["reasons"]}))
    if not result["accepted"]:
        raise SystemExit(2)


if __name__ == "__main__":
    main()
