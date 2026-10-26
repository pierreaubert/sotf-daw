"""Actual native source masks, complete 5.1/7.1 output and raw evidence hashes."""

import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np


def render(signal, renderer, work, *, model=None, sha=None, layout="5.1", callback=113):
    x = np.asarray(signal, dtype="<f4")
    if (
        x.ndim != 2
        or x.shape[1] != 2
        or not 0 < len(x) <= 1_000_000
        or not np.isfinite(x).all()
        or np.max(abs(x)) > 64
        or layout not in {"5.1", "7.1"}
        or type(callback) is not int
        or not 1 <= callback <= 256
    ):
        raise ValueError("Invalid bounded native Upmixer geometry")
    root = Path(work).resolve()
    root.mkdir(parents=True, exist_ok=False)
    request = {
        "input": str(root / "input.f32le"),
        "output": str(root / "output.f32le"),
        "features_output": str(root / "features.json"),
        "frames": len(x),
        "callback_frames": callback,
        "speaker_config": layout,
        "model": None
        if model is None
        else {"path": str(Path(model).resolve()), "sha256": sha},
    }
    Path(request["input"]).write_bytes(x.tobytes())
    path = root / "request.json"
    path.write_text(json.dumps(request, indent=2) + "\n")
    p = subprocess.run(
        [str(Path(renderer).resolve()), str(path)],
        capture_output=True,
        text=True,
        timeout=120,
        check=False,
    )
    (root / "stderr.txt").write_text(p.stderr)
    if p.returncode:
        raise RuntimeError(p.stderr)
    metadata = json.loads(Path(request["features_output"]).read_text())
    channels = 6 if layout == "5.1" else 8
    if (
        metadata["channels"] != channels
        or metadata["source_frames"] != len(x)
        or metadata["sample_rate"] != 48000
        or metadata["hop_frames"] != 256
        or metadata["latency_samples"] != 512
        or metadata["model_lookahead_samples"] != 0
        or not len(x) <= metadata["output_frames"] <= len(x) + 65536
        or len(metadata["rows"]) != len(x) // 256
    ):
        raise RuntimeError("Native Upmixer export geometry mismatch")
    for i, row in enumerate(metadata["rows"]):
        if (
            row["end_frame"] != (i + 1) * 256
            or not row["end_frame"]
            <= row["observed_after_frame"]
            < row["end_frame"] + callback
        ):
            raise RuntimeError("Native feature timestamp mismatch")
    output = np.fromfile(request["output"], dtype="<f4").reshape(-1, channels)
    if len(output) != metadata["output_frames"] or not np.isfinite(output).all():
        raise RuntimeError("Invalid native Upmixer output")
    metadata["files"] = {
        key: {
            "path": request[key],
            "sha256": hashlib.sha256(Path(request[key]).read_bytes()).hexdigest(),
        }
        for key in ["input", "output", "features_output"]
    }
    (root / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    return output, metadata
