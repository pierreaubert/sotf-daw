"""Freeze Python feature/score references for deterministic Rust arithmetic probes.

Non-training geometries test numerical parity only, not model qualification.
No audio files, capture commands or profile mutations are produced.
"""

import hashlib
import json
from pathlib import Path

import hiss_intervals as learner
import numpy as np

ROOT = Path(__file__).resolve().parents[2]
MODEL = ROOT / "crates/sotf-plugins/models/hiss-intervals/model.synthetic.json"
OUTPUT = (
    ROOT
    / "crates/sotf-plugins/crates/sotf-plugin-hiss-reducer/tests/fixtures/interval-parity.json"
)


def pcm(rate, channels, kind, seed):
    state = seed
    result = np.empty((rate, channels), dtype=np.float32)
    for frame in range(rate):
        for channel in range(channels):
            state = (state * 1664525 + 1013904223) & 0xFFFFFFFF
            noise = (state / 4294967296 - 0.5) * 0.02
            saw = ((frame * (channel + 3)) % 97) / 97 - 0.5
            envelope = 0.1 + (frame % 10007) / 10007
            value = noise if kind == "noise" else (noise * 0.1 + saw * envelope)
            result[frame, channel] = value / (channel + 1)
    return result


def main():
    raw = MODEL.read_bytes()
    model = learner.load_model(MODEL, hashlib.sha256(raw).hexdigest())
    rows = []
    for rate, channels in [(44100, 1), (48000, 1), (96000, 2), (192000, 8), (44150, 2)]:
        for kind in ["noise", "programme"]:
            ctx = dict(model["context"], sample_rate=rate, channels=channels)
            signal = pcm(rate, channels, kind, 20781026)
            feature = learner.features(signal, ctx)
            rows.append(
                {
                    "context": ctx,
                    "kind": kind,
                    "seed": 20781026,
                    "pcm_sha256": hashlib.sha256(
                        signal.astype("<f4").tobytes()
                    ).hexdigest(),
                    "features": feature.tolist(),
                    "scores": learner.score(model["parameters"], feature),
                    "context_matches_model": ctx == model["context"],
                }
            )
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_text(
        json.dumps(
            {
                "qualification": "Arithmetic parity only. Scores outside model context are not qualified predictions.",
                "model_sha256": hashlib.sha256(raw).hexdigest(),
                "cases": rows,
            },
            indent=2,
            allow_nan=False,
        )
        + "\n"
    )


if __name__ == "__main__":
    main()
