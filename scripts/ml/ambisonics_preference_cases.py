"""Generated functional preferences, never presented as actual listener data."""

import json
import math
from pathlib import Path

import numpy as np
from ambisonics_native import normalize_gains, preset, render
from fit_ambisonics_preferences import FORMAT, ZEROS, digest


def session(work, renderer):
    work = Path(work)
    work.mkdir(parents=True, exist_ok=False)
    params = preset(dual_band=True)
    _, m = render(np.zeros((17, 4)), renderer, work / "geometry", params, [1] * 6)
    matrices = [
        np.array(m[k]["matrix"]).reshape(6, 4)
        for k in ["decoder_matrix", "basic_matrix"]
    ]
    desired = [
        [1] * 6,
        [0.95, 1.05, 1.02, 1, 0.96, 1.04],
        [1.05, 0.95, 0.98, 1, 1.04, 0.96],
    ]
    candidates = [
        {
            "id": f"candidate-{i}",
            "parameters": {
                "speaker_gains": normalize_gains(matrices, g, [3])
                .astype(float)
                .tolist()
            },
        }
        for i, g in enumerate(desired)
    ]
    programmes = []
    for i in range(8):
        rng = np.random.default_rng(202610300 + i)
        n = 12001 + i * 13
        t = np.arange(n) / 48000
        x = rng.normal(0, 0.018, (n, 4))
        x += 0.025 * np.sin(2 * np.pi * (170 + 37 * i) * t[:, None] + np.arange(4))
        x *= (
            np.minimum(t / 0.01, 1)[:, None]
            * np.minimum((t[-1] - t) / 0.01, 1)[:, None]
        )
        x = x.astype("<f4")
        path = work / f"programme-{i}.f32le"
        path.write_bytes(x.tobytes())
        levels = []
        for ci, c in enumerate(candidates):
            y, _ = render(
                x,
                renderer,
                work / f"level-{i}-{ci}",
                params,
                c["parameters"]["speaker_gains"],
                zeros=ZEROS,
            )
            levels.append(10 * math.log10(float(np.mean(y.astype(float) ** 2))))
        programmes.append(
            {
                "id": f"programme-{i}",
                "audio_file": str(path.resolve()),
                "audio_sha256": digest(path.read_bytes()),
                "frames": n,
                "post_gains_db": {
                    c["id"]: levels[0] - level
                    for c, level in zip(candidates, levels, strict=True)
                },
            }
        )
    trials = []
    rank = [0, 2, 1]
    for p in programmes:
        for a in range(3):
            for b in range(3):
                if a != b:
                    trials.append(
                        {
                            "id": f"trial-{len(trials)}",
                            "programme_id": p["id"],
                            "a": candidates[a]["id"],
                            "b": candidates[b]["id"],
                            "choice": "a" if rank[a] > rank[b] else "b",
                        }
                    )
    np.random.default_rng(202610301).shuffle(trials)
    data = {
        "format": FORMAT,
        "data_origin": "synthetic_validation",
        "context": {
            "listener_id": "synthetic-listener",
            "playback_setup_id": "synthetic-5.1-array",
            "room_id": "synthetic-room",
            "calibration_id": "uncalibrated-functional-probe",
            "session_id": "synthetic-consistent",
            "criterion": "overall_preference",
            "normalization": "ACN/SN3D",
            "sample_rate": 48000,
            "renderer_sha256": digest(Path(renderer).read_bytes()),
            "decoder_parameters": params,
        },
        "protocol": {
            "blinded": True,
            "randomized_order": True,
            "whole_source_grouping": True,
            "fixed_listener_position": True,
            "maximum_level_mismatch_db": 0.25,
        },
        "candidates": candidates,
        "baseline_id": "candidate-0",
        "programmes": programmes,
        "trials": trials,
    }
    (work / "session.json").write_text(json.dumps(data, indent=2) + "\n")
    return data
