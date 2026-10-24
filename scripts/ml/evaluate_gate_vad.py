#!/usr/bin/env python3
"""Compare a frozen speech detector on existing development recordings.

No fitting or threshold selection occurs. Clip pass fractions are descriptive:
speech clips contain unlabeled pauses and their overlap with model training is
unknown. The independent 100 ms replay matches the bounded-window Gate design;
the streaming control retains recurrent history and follows the upstream wrapper.
"""

import argparse
import hashlib
import json
import platform
import sys
import time
from pathlib import Path

import numpy as np
import onnxruntime as ort

MODEL_SHAS = {
    "7ed98ddbad84ccac4cd0aeb3099049280713df825c610a8ed34543318f1b2c49",
    "7671cd04b004e9076da0d4a7b1a5aec36adf161c39230c1cb94a4fd5db6bbd28",
    "7d47a98acce5f4b688a7d541ee0d9061b64171fdb39581cbcf29fe72bfe57c51",
}
THRESHOLD = 0.8
ROOT = Path(__file__).resolve().parents[2]
CRATES = ROOT / "crates/sotf-plugins/crates"


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def resampling_weights(rate=48000):
    """Causal finite-window sinc/Blackman interpolation, with DC normalization."""
    positions = np.arange(1600, dtype=np.float64) * rate / 16000
    indices = np.arange(int(np.ceil(rate * 0.1)))
    distances = indices[None, :] - positions[:, None]
    cutoff = min(16000 / rate, 1)
    radius = np.ceil(16 / cutoff)
    window = (
        0.42
        + 0.5 * np.cos(np.pi * distances / radius)
        + 0.08 * np.cos(2 * np.pi * distances / radius)
    )
    weights = np.sinc(distances * cutoff) * cutoff * window
    weights[np.abs(distances) > radius] = 0
    weights /= weights.sum(axis=1, keepdims=True)
    return weights


def step(session, audio, state):
    output, next_state = session.run(
        ["output", "stateN"],
        {
            "input": audio.reshape(1, 576),
            "state": state,
            "sr": np.array(16000, np.int64),
        },
    )
    score = float(output.item())
    if (
        not np.isfinite(score)
        or not 0 <= score <= 1
        or not np.isfinite(next_state).all()
    ):
        raise ValueError("Nonfinite or out-of-range model output")
    return score, next_state


def replay(session, audio):
    """Reset state; replay complete 512-sample hops with 64 samples of context."""
    state = np.zeros((2, 1, 128), np.float32)
    scores = []
    for offset in range(0, len(audio) - 575, 512):
        score, state = step(session, audio[offset : offset + 576], state)
        scores.append(score)
    return scores, state


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    model_sha = sha(args.model)
    if model_sha not in MODEL_SHAS:
        raise ValueError("Pinned model hash mismatch")
    if args.output.exists():
        raise ValueError("Output already exists; preserve previous measurements")
    args.output.mkdir(parents=True)
    options = ort.SessionOptions()
    options.intra_op_num_threads = 1
    options.inter_op_num_threads = 1
    session = ort.InferenceSession(
        str(args.model), options, providers=["CPUExecutionProvider"]
    )
    weights = resampling_weights()
    fixtures = [
        (
            f"speech-{n}",
            CRATES
            / f"sotf-plugin-speech-denoiser/tests/fixtures/recorded/speech-{n}.pcm",
        )
        for n in range(3)
    ] + [
        (name, CRATES / f"sotf-plugin-de-esser/tests/fixtures/recorded/{name}.pcm")
        for name in ["violin", "cymbal", "s", "sh", "p"]
    ]
    rows = []
    golden = []
    for name, path in fixtures:
        source = (
            np.frombuffer(path.read_bytes(), dtype="<i2").astype(np.float32) / 32768
        )
        started = time.perf_counter()
        scores = []
        # Identical source-window ends and 10 ms hop to the RNNoise diagnostic.
        for end in range(4800, len(source) + 1, 480):
            audio = (weights @ source[end - 4800 : end].astype(np.float64)).astype(
                np.float32
            )
            result, state = replay(session, audio)
            scores.append(result[-1])
            if end == 4800:
                golden.append(
                    {
                        "fixture": name,
                        "audio_16k": audio.tolist(),
                        "step_scores": result,
                        "final_state": state.reshape(-1).tolist(),
                    }
                )
        elapsed = time.perf_counter() - started
        # Streaming control uses the same bounded interpolation in nonoverlapping
        # 100 ms tiles; concatenated output feeds the unchanged upstream hop/state
        # convention. Tile boundaries differ from the replay grid, so this is a
        # protocol diagnostic rather than a paired score superiority test.
        audio = np.concatenate(
            [
                weights @ source[start : start + 4800].astype(np.float64)
                for start in range(0, len(source) - 4799, 4800)
            ]
        ).astype(np.float32)
        state = np.zeros((2, 1, 128), np.float32)
        context = np.zeros(64, np.float32)
        streaming = []
        for start in range(0, len(audio) - 511, 512):
            chunk = np.concatenate([context, audio[start : start + 512]])
            score, state = step(session, chunk, state)
            streaming.append(score)
            context = chunk[-64:]
        # Eight consecutive steps also test recurrent numerical drift, using
        # the wrapper's zero initial context and zero initial hidden state.
        stream_probe = np.concatenate([np.zeros(64, np.float32), audio[:4096]])
        probe_scores, probe_state = replay(session, stream_probe)
        golden.append(
            {
                "fixture": name + "-streaming-eight-steps",
                "audio_16k": stream_probe.tolist(),
                "step_scores": probe_scores,
                "final_state": probe_state.reshape(-1).tolist(),
            }
        )
        row = {
            "fixture": name,
            "source_path": str(path.relative_to(ROOT)),
            "source_sha256": sha(path),
            "source_frames": len(source),
            "threshold": THRESHOLD,
            "window_ms": 100,
            "hop_ms": 10,
            "replay_scores": scores,
            "streaming_scores": streaming,
            "replay_pass_fraction": float(np.mean(np.array(scores) >= THRESHOLD)),
            "streaming_pass_fraction": float(np.mean(np.array(streaming) >= THRESHOLD)),
            "replay_elapsed_seconds": elapsed,
        }
        rows.append(row)
        print(
            {
                key: row[key]
                for key in [
                    "fixture",
                    "replay_pass_fraction",
                    "streaming_pass_fraction",
                ]
            },
            flush=True,
        )
    report = {
        "model_sha256": model_sha,
        "script_sha256": sha(Path(__file__)),
        "command": [sys.executable, *sys.argv],
        "threshold": THRESHOLD,
        "python": platform.python_version(),
        "numpy": np.__version__,
        "onnxruntime": ort.__version__,
        "providers": session.get_providers(),
        "protocol": "1600 source samples at 16 kHz: first 64 are context, then three 512-sample hops with zero initial state. All 100 ms belong to the observed past. No threshold tuning.",
        "limits": "Existing development recordings, unknown training overlap. Speech pauses lack labels; clip pass fractions are not recall. Model-only replay excludes worker deadlines and gain smoothing.",
        "measurements": rows,
    }
    (args.output / "comparison.json").write_text(json.dumps(report, indent=2) + "\n")
    (args.output / "ort-golden.json").write_text(
        json.dumps({"model_sha256": model_sha, "cases": golden}, indent=2) + "\n"
    )


if __name__ == "__main__":
    main()
