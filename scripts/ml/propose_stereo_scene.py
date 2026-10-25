"""Export a natively qualified offline scene suggestion; never applies it live."""

import argparse
import json
from pathlib import Path

import numpy as np
from stereo_scenes import context, digest, load_model, propose


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--audio", type=Path, required=True)
    parser.add_argument("--frames", type=int, required=True)
    parser.add_argument("--context", type=Path, required=True)
    parser.add_argument("--model", type=Path)
    parser.add_argument("--model-sha256")
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        raise ValueError("Use a new proposal directory")
    with args.context.open("rb") as f:
        ctxraw = f.read(16385)
    if len(ctxraw) > 16384:
        raise ValueError("Context exceeds16KiB")
    ctx = context(json.loads(ctxraw))
    if (
        not ctx["sample_rate"] <= args.frames <= 10 * ctx["sample_rate"]
        or args.frames * 2 > 4_000_000
    ):
        raise ValueError("Use bounded1..10s stereo PCM")
    with args.audio.open("rb") as f:
        raw = f.read(args.frames * 8 + 1)
    if len(raw) != args.frames * 8:
        raise ValueError("PCM byte length mismatch")
    model, error = None, None
    try:
        if not args.model or not args.model_sha256:
            raise ValueError("Provide explicit model path and SHA256")
        model = load_model(args.model, args.model_sha256)
    except (ValueError, KeyError, TypeError, OSError) as exc:
        error = str(exc)
    args.output.mkdir(parents=True, exist_ok=False)
    result = propose(
        np.frombuffer(raw, dtype="<f4").reshape(-1, 2),
        ctx,
        model,
        args.renderer,
        args.output / "native",
    )
    result.update(
        input_sha256=digest(raw), model_sha256=args.model_sha256, model_error=error
    )
    if result["accepted"]:
        (args.output / "preset.json").write_text(
            json.dumps(
                {
                    "plugin_type": "stereo_imager",
                    "parameters": result["suggested_params"],
                },
                indent=2,
                allow_nan=False,
            )
            + "\n"
        )
    (args.output / "report.json").write_text(
        json.dumps(result, indent=2, allow_nan=False) + "\n"
    )
    print(
        json.dumps(
            {
                "accepted": result["accepted"],
                "reasons": result["reasons"],
                "automatic_apply": False,
            }
        )
    )
    if not result["accepted"]:
        raise SystemExit(2)


if __name__ == "__main__":
    main()
