#!/usr/bin/env python3
"""Recommend native release from explicit aligned dry/reference float32 audio."""

import argparse
import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np
from limiter_release import Model, fit, validate_pair


def read_once(path, limit):
    with Path(path).open("rb") as stream:
        raw = stream.read(limit + 1)
    if len(raw) > limit:
        raise ValueError("Input exceeds size limit")
    return raw, hashlib.sha256(raw).hexdigest()


def target(path):
    raw, digest = read_once(path, 65536)
    data = json.loads(raw)
    if not isinstance(data, dict) or set(data) != {
        "format",
        "alignment",
        "sample_rate",
        "frames",
        "channels",
        "dry",
        "reference",
        "fixed_params",
    }:
        raise ValueError("Use the explicit paired native release target format")
    if (
        data["format"] != "sotf-limiter-release-target-v1"
        or data["alignment"] != "latency_removed"
    ):
        raise ValueError(
            "Supply explicitly aligned dry/reference audio without latency prefix"
        )
    frames, channels = data["frames"], data["channels"]
    if (
        type(frames) is not int
        or type(channels) is not int
        or not 1 <= channels <= 8
        or frames <= 0
        or frames * channels > 4_000_000
    ):
        raise ValueError("Invalid paired array geometry")
    arrays, digests = [], {"target_json": digest}
    for key in ["dry", "reference"]:
        if not isinstance(data[key], str) or not data[key]:
            raise ValueError("Supply dry/reference raw float32-le paths")
        source = Path(data[key])
        if not source.is_absolute():
            source = Path(path).resolve().parent / source
        contents, digests[key] = read_once(source, frames * channels * 4)
        if len(contents) != frames * channels * 4:
            raise ValueError("Audio byte count does not match explicit geometry")
        arrays.append(
            np.frombuffer(contents, dtype="<f4").reshape(frames, channels).copy()
        )
    dry, reference = validate_pair(*arrays, data["sample_rate"], data["fixed_params"])
    return data, dry, reference, digests


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", type=Path, required=True)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--model", type=Path)
    parser.add_argument("--model-sha256")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--callback-frames", type=int, default=193)
    parser.add_argument("--budget", type=int, default=24)
    args = parser.parse_args()
    if args.output.exists() or args.work.exists():
        raise ValueError("Use new output/work directories")
    if bool(args.model) != bool(args.model_sha256):
        raise ValueError("Supply both model path and explicit expected SHA-256")
    data, dry, reference, digests = target(args.target)
    model = Model(args.model, args.model_sha256) if args.model else None
    renderer_digest = hashlib.sha256(args.renderer.read_bytes()).hexdigest()
    try:
        result = fit(
            dry,
            reference,
            data["sample_rate"],
            data["fixed_params"],
            args.renderer,
            args.work,
            model,
            args.callback_frames,
            args.budget,
        )
    except (ValueError, subprocess.TimeoutExpired) as error:
        result = {
            "accepted": False,
            "parameters": None,
            "reasons": ["Native qualification failed: " + str(error)],
        }
    if hashlib.sha256(args.renderer.read_bytes()).hexdigest() != renderer_digest:
        result = {
            "accepted": False,
            "parameters": None,
            "reasons": ["Renderer changed during qualification"],
        }
    result.update(
        input_sha256=digests,
        renderer_sha256=renderer_digest,
        model_sha256=model.digest if model else None,
    )
    args.output.mkdir(parents=True, exist_ok=False)
    (args.output / "report.json").write_text(
        json.dumps(result, allow_nan=False, indent=2) + "\n"
    )
    if result["accepted"]:
        (args.output / "preset.json").write_text(
            json.dumps(result["parameters"], allow_nan=False, indent=2) + "\n"
        )
    print(
        json.dumps({key: result[key] for key in ["accepted", "parameters", "reasons"]})
    )
    raise SystemExit(0 if result["accepted"] else 2)


if __name__ == "__main__":
    main()
