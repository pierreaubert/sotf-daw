"""Synthetic HRTF/choice fixtures: functional checks, never listener evidence."""

import hashlib
from pathlib import Path

import h5py
import numpy as np
from binaural_native import render
from fit_binaural_preferences import FORMAT, digest


def sofa(path, variant):
    """Write simple causal synthetic HRIRs with distinct delayed spectral taps."""
    ir = np.zeros((4, 2, 128), dtype=np.float32)
    ir[:, :, 12] = 1
    ir[:, 0, 20 + variant * 9] = 0.2 + variant * 0.1
    ir[:, 1, 28 + variant * 7] = -0.15 - variant * 0.08
    with h5py.File(path, "w", libver="v108", track_order=True) as f:
        for k, v in {
            "Conventions": "SOFA",
            "SOFAConventions": "SimpleFreeFieldHRIR",
            "SOFAConventionsVersion": "1.0",
            "DataType": "FIR",
            "RoomType": "free field",
        }.items():
            f.attrs.create(k, np.bytes_(v), dtype=f"S{len(v) + 1}")
        for name, size in [("M", 4), ("R", 2), ("N", 128), ("C", 3), ("I", 1)]:
            ds = f.create_dataset(name, data=np.zeros(size, dtype=np.float32))
            ds.make_scale(name)
        f.create_dataset(
            "Data.SamplingRate", data=np.array([48000.0], dtype=np.float32)
        )
        p = f.create_dataset(
            "SourcePosition",
            data=np.array(
                [[0, 0, 1], [90, 0, 1], [180, 0, 1], [270, 0, 1]], dtype=np.float32
            ),
        )
        p.attrs["Type"] = "spherical"
        p.attrs["Units"] = "degree, degree, metre"
        f.create_dataset("Data.IR", data=ir)
    return path


def session(root, renderer, kind="consistent", programmes=8):
    root = Path(root)
    root.mkdir(parents=True, exist_ok=False)
    candidates = []
    for i in range(3):
        path = sofa(root / f"candidate-{i}.sofa", i)
        candidates.append(
            {
                "id": f"candidate-{i}",
                "parameters": {
                    "sofa_file": str(path.resolve()),
                    "sofa_sha256": digest(path.read_bytes()),
                },
            }
        )
    sources = []
    for i in range(programmes):
        x = (
            np.random.default_rng(20531026 + i).normal(size=(12001, 2)) * 0.025
        ).astype(np.float32)
        path = root / f"programme-{i}.f32le"
        raw = x.astype("<f4").tobytes()
        path.write_bytes(raw)
        gains = {}
        for j, c in enumerate(candidates):
            y, _ = render(
                x,
                renderer,
                c["parameters"]["sofa_file"],
                root / f"calibration-{i}-{j}",
                fft_size=512,
            )
            gains[c["id"]] = -36 - 10 * np.log10(np.mean(y.astype(float) ** 2))
        sources.append(
            {
                "id": f"programme-{i}",
                "audio_file": str(path.resolve()),
                "audio_sha256": digest(raw),
                "frames": len(x),
                "post_gains_db": gains,
            }
        )
    trials = []
    for p in sources:
        for a, b in [(0, 1), (0, 2), (1, 2), (1, 0), (2, 0), (2, 1)]:
            for _ in range(2):
                rank = (
                    [0, 2, 1]
                    if kind != "heterogeneous"
                    or int(p["id"].split("-")[-1]) < programmes // 2
                    else [2, 0, 1]
                )
                choice = "a" if rank[a] > rank[b] else "b"
                if kind == "ties":
                    choice = "tie"
                if kind == "side-bias":
                    choice = "a"
                trials.append(
                    {
                        "id": f"trial-{len(trials)}",
                        "programme_id": p["id"],
                        "a": candidates[a]["id"],
                        "b": candidates[b]["id"],
                        "choice": choice,
                    }
                )
    return {
        "format": FORMAT,
        "data_origin": "synthetic_validation",
        "context": {
            "listener_id": "synthetic-listener",
            "headphones_id": "synthetic-headphones",
            "headphone_eq_id": "none",
            "session_id": "functional-" + kind,
            "sample_rate": 48000,
            "input_channels": 2,
            "fft_size": 512,
            "diffuse_field_eq": False,
            "criterion": "overall_preference",
            "head_pose_deg": [0, 0, 0],
            "renderer_sha256": hashlib.sha256(Path(renderer).read_bytes()).hexdigest(),
        },
        "protocol": {
            "blinded": True,
            "randomized_order": True,
            "fixed_head_pose": True,
            "whole_source_grouping": True,
            "maximum_level_mismatch_db": 0.25,
        },
        "baseline_id": "candidate-0",
        "candidates": candidates,
        "programmes": sources,
        "trials": trials,
    }
