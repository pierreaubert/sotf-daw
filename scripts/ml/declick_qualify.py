#!/usr/bin/env python3
"""Opt-in offline veto of native repairs; unsupported/uncertain groups stay dry."""

import argparse
import hashlib
import io
import json
from pathlib import Path

import numpy as np
from declick_model import Model, qualify
from declick_native import render
from scipy.io import wavfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--model-sha256", required=True)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists() or args.work.exists():
        raise ValueError("Use new output and work directories")
    with args.input.open("rb") as source:
        raw = source.read(16_100_001)
    if len(raw) > 16_100_000:
        raise ValueError("Input exceeds offline WAV bound")
    rate, dry = wavfile.read(io.BytesIO(raw))
    if dry.dtype != np.float32:
        raise ValueError("Use float32 WAV; no implicit PCM level conversion")
    if dry.ndim == 1:
        dry = dry[:, None]
    native, metadata = render(dry, rate, args.renderer, args.work)
    model, model_error = None, None
    try:
        model = Model(args.model, args.model_sha256)
    except (OSError, ValueError, TypeError, KeyError, OverflowError) as error:
        model_error = str(error)
    output, decisions = qualify(dry, native, rate, model)
    args.output.mkdir(parents=True, exist_ok=False)
    wavfile.write(
        args.output / "qualified.wav",
        rate,
        output[:, 0] if output.shape[1] == 1 else output,
    )
    report = {
        "status": "ok" if model is not None else "dry_fallback",
        "model_error": model_error,
        "input_sha256": hashlib.sha256(raw).hexdigest(),
        "model_sha256": None if model is None else model.digest,
        "sample_rate": rate,
        "channels": dry.shape[1],
        "frames": len(dry),
        "native_latency_samples": metadata["latency_samples"],
        "export_latency_samples": 0,
        "alignment": "Native latency prefix removed after exact drain; original frame count preserved",
        "native_groups": len(decisions),
        "accepted_groups": sum(r["accepted"] for r in decisions),
        "decisions": decisions,
        "qualification": "Experimental synthetic-trained repair utility score; no semantic click identity or listening safety guarantee",
    }
    (args.output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    print(
        json.dumps(
            {
                k: report[k]
                for k in ["status", "native_groups", "accepted_groups", "model_error"]
            }
        )
    )


if __name__ == "__main__":
    main()
