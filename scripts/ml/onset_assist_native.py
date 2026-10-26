"""Native causal-feature export and actual shaped-output adapter."""

import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np


def render(
    signal,
    renderer,
    work,
    *,
    rate=48000,
    model=None,
    sha=None,
    attack=0.0,
    sustain=0.0,
    callback=64,
):
    x = np.asarray(signal, dtype="<f4")
    if x.ndim == 1:
        x = x[:, None]
    if (
        x.ndim != 2
        or not 1 <= x.shape[1] <= 2
        or not 0 < len(x) <= 1_000_000
        or not np.isfinite(x).all()
        or np.max(abs(x)) > 64
        or type(rate) is not int
        or not 44100 <= rate <= 192000
        or type(callback) is not int
        or not 1 <= callback <= 64
    ):
        raise ValueError("Invalid bounded onset render geometry")
    root = Path(work).resolve()
    root.mkdir(parents=True, exist_ok=False)
    request = {
        "input": str(root / "input.f32le"),
        "output": str(root / "output.f32le"),
        "features_output": str(root / "features.json"),
        "frames": len(x),
        "channels": x.shape[1],
        "sample_rate": rate,
        "callback_frames": callback,
        "parameters": {
            "attack": attack,
            "sustain": sustain,
            "sensitivity_db": 0.0,
            "output_gain_db": 0.0,
            "mix": 1.0,
        },
        "model": None
        if model is None
        else {"path": str(Path(model).resolve()), "sha256": sha},
    }
    Path(request["input"]).write_bytes(x.tobytes())
    path = root / "request.json"
    path.write_text(json.dumps(request, indent=2, allow_nan=False) + "\n")
    result = subprocess.run(
        [str(Path(renderer).resolve()), str(path)],
        capture_output=True,
        text=True,
        timeout=120,
        check=False,
    )
    (root / "stderr.txt").write_text(result.stderr)
    if result.returncode:
        raise RuntimeError(result.stderr)
    data = json.loads(Path(request["features_output"]).read_text())
    hop = int(np.floor(rate * 0.002 + 0.5))
    if (
        data["hop_frames"] != hop
        or len(data["rows"]) != len(x) // hop
        or data["latency_samples"] != 0
        or data["added_model_lookahead_samples"] != 0
        or data["source_frames"] != len(x)
        or data["final_state"]["counters"]["frames"] != len(x)
    ):
        raise RuntimeError("Native onset geometry mismatch")
    for i, row in enumerate(data["rows"]):
        if (
            row["end_frame"] != (i + 1) * hop
            or not row["end_frame"]
            <= row["observed_after_frame"]
            < row["end_frame"] + callback
        ):
            raise RuntimeError("Noncausal native feature timestamp")
    output = np.fromfile(request["output"], dtype="<f4").reshape(x.shape)
    if not np.isfinite(output).all():
        raise RuntimeError("Nonfinite native onset output")
    data["files"] = {
        key: {
            "path": request[key],
            "sha256": hashlib.sha256(Path(request[key]).read_bytes()).hexdigest(),
        }
        for key in ["input", "output", "features_output"]
    }
    (root / "metadata.json").write_text(json.dumps(data, indent=2) + "\n")
    return output, data
