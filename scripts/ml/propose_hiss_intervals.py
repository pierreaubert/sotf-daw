#!/usr/bin/env python3
"""Review-only Hiss intervals from a verified optional model; no profile writes."""

import argparse
import json
from pathlib import Path

import numpy as np
from hiss_intervals import context, digest, load_model, propose


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--audio", type=Path, required=True)
    p.add_argument("--frames", type=int, required=True)
    p.add_argument("--context", type=Path, required=True)
    p.add_argument("--model", type=Path)
    p.add_argument("--model-sha256")
    p.add_argument("--output", type=Path, required=True)
    args = p.parse_args()
    if args.output.exists():
        raise ValueError("Use a new proposal output directory")
    with args.context.open("rb") as f:
        ctxraw = f.read(16385)
    if len(ctxraw) > 16384:
        raise ValueError("Context exceeds16 KiB")
    ctx = context(json.loads(ctxraw))
    count = args.frames * ctx["channels"]
    if args.frames < 1 or count > 4_000_000:
        raise ValueError("Proposal audio exceeds4 million samples")
    with args.audio.open("rb") as f:
        raw = f.read(count * 4 + 1)
    if len(raw) != count * 4:
        raise ValueError("PCM length mismatch")
    model = None
    error = None
    try:
        if not args.model or not args.model_sha256:
            raise ValueError("Provide an explicit model path and SHA256")
        model = load_model(args.model, args.model_sha256)
    except (ValueError, OSError, KeyError, TypeError) as e:
        error = str(e)
    result = propose(
        np.frombuffer(raw, dtype="<f4").reshape(args.frames, ctx["channels"]),
        ctx,
        model,
    )
    result.update(
        input_sha256=digest(raw), model_sha256=args.model_sha256, model_error=error
    )
    args.output.mkdir(parents=True, exist_ok=False)
    (args.output / "proposals.json").write_text(
        json.dumps(result, indent=2, allow_nan=False) + "\n"
    )
    print(
        json.dumps(
            {
                "review_candidates": sum(
                    r["review_candidate"] for r in result["intervals"]
                ),
                "automatic_capture": False,
                "model_error": error,
            }
        )
    )
    if error:
        raise SystemExit(2)


if __name__ == "__main__":
    main()
