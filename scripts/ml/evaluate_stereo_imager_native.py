"""Freeze native renders before building an offline scene-setting learner."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from stereo_imager_native import preset, render


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    if args.report.exists():
        raise ValueError("Refuse to overwrite an existing report")
    args.work.mkdir(parents=True, exist_ok=False)
    binary = hashlib.sha256(args.renderer.read_bytes()).hexdigest()
    rows = []
    settings = {
        "neutral": preset(),
        "dry": preset(width=2, mix=0),
        "mono": preset(width=0),
        "shaped": preset(
            width=1.5, low_width=0.3, mid_width=1.1, high_width=1.8, mix=0.75
        ),
        "bass_mono": preset(
            width=1.5,
            low_width=0.3,
            mid_width=1.1,
            high_width=1.8,
            mix=0.75,
            mono_bass=True,
        ),
        "extreme": preset(
            width=2,
            low_width=0,
            mid_width=2,
            high_width=2,
            low_mid_freq=80,
            mid_high_freq=10000,
        ),
    }
    for rate in [44100, 48000, 96000, 192000]:
        rng = np.random.default_rng(20861026 + rate)
        left = rng.normal(size=8193) * 0.12
        right = 0.7 * left + rng.normal(size=8193) * 0.08
        signals = {
            "correlated": np.column_stack([left, right]),
            "dual_mono": np.column_stack([left, left]),
            "antiphase": np.column_stack([left, -left]),
            "one_silent": np.column_stack([left, np.zeros(len(left))]),
        }
        for source, x in signals.items():
            for setting, params in settings.items():
                work = args.work / f"{rate}-{source}-{setting}"
                _, metadata = render(
                    x, args.renderer, work, params, rate=rate, callback=193, zeros=4096
                )
                rows.append(
                    {
                        "rate": rate,
                        "source": source,
                        "setting": setting,
                        "work": str(work.resolve()),
                        "metadata": metadata,
                    }
                )
    if hashlib.sha256(args.renderer.read_bytes()).hexdigest() != binary:
        raise ValueError("Native binary changed during evaluation")
    args.report.write_text(
        json.dumps(
            {
                "renderer_sha256": binary,
                "seed": 20861026,
                "qualification": "Native functional reference probes only; no trained scene model or perceptual quality assessment.",
                "cases": rows,
            },
            indent=2,
            allow_nan=False,
        )
        + "\n"
    )


if __name__ == "__main__":
    main()
