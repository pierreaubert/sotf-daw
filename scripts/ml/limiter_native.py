"""Offline native limiter rendering for paired-reference release fitting.

Every render retains the native hard, fully wet ceiling. This adapter removes
only the reported latency prefix after the plugin has drained its finite tail.
It makes no independent inter-sample/continuous-time peak claim.
"""

import json
import subprocess
from pathlib import Path

import numpy as np

PARAMS = {
    "threshold_db": -6.0,
    "release_ms": 50.0,
    "lookahead_ms": 5.0,
    "soft": False,
    "true_peak": False,
    "isp_mode": False,
    "dual_release": False,
    "mix": 1.0,
    "feed_forward": True,
    "link_amount": 1.0,
    "oversampling": 0,
}


def render(signal, sample_rate, renderer, work, callback_frames=193, params=None):
    """Render a bounded interleaved float32 array using a fresh native instance."""
    signal = np.asarray(signal, dtype=np.float32)
    if signal.ndim == 1:
        signal = signal[:, None]
    if (
        signal.ndim != 2
        or not 1 <= signal.shape[1] <= 8
        or not 1 <= signal.size <= 4_000_000
        or not np.isfinite(signal).all()
        or np.max(np.abs(signal)) > 32
        or type(sample_rate) is not int
        or not 8000 <= sample_rate <= 192000
        or type(callback_frames) is not int
        or not 1 <= callback_frames <= 4096
    ):
        raise ValueError("Unsupported bounded native input or geometry")
    work = Path(work)
    work.mkdir(parents=True, exist_ok=False)
    signal.astype("<f4").tofile(work / "input.f32le")
    request = {
        "input": str((work / "input.f32le").resolve()),
        "output": str((work / "output.f32le").resolve()),
        "sample_rate": sample_rate,
        "channels": signal.shape[1],
        "frames": len(signal),
        "callback_frames": callback_frames,
        "params": dict(PARAMS if params is None else params),
    }
    path = work / "request.json"
    path.write_text(json.dumps(request, allow_nan=False) + "\n")
    result = subprocess.run(
        [str(Path(renderer).resolve()), str(path)],
        capture_output=True,
        text=True,
        check=False,
        timeout=120,
    )
    if result.returncode:
        raise ValueError("Native limiter render failed: " + result.stderr)
    metadata = json.loads(result.stdout)
    (work / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    latency = metadata["latency_samples"]
    # 20 ms lookahead plus a conservative bound for the prepared rate filters.
    if type(latency) is not int or not 0 <= latency <= sample_rate // 50 + 4096:
        raise ValueError("Invalid native latency")
    drained = metadata["drained_frames"]
    if type(drained) is not int or not latency <= drained <= 65536:
        raise ValueError("Invalid native drain length")
    expected_samples = (len(signal) + drained) * signal.shape[1]
    output_path = work / "output.f32le"
    if output_path.stat().st_size != 4 * expected_samples:
        raise ValueError("Invalid native output size")
    raw = np.fromfile(output_path, dtype="<f4")
    ceiling = 10 ** (request["params"].get("threshold_db", -0.1) / 20)
    if (
        metadata["status"] != "ok"
        or metadata["sample_rate"] != sample_rate
        or metadata["channels"] != signal.shape[1]
        or metadata["input_frames"] != len(signal)
        or metadata["output_frames"] != len(signal) + drained
        or not np.isfinite(raw).all()
        or np.max(np.abs(raw)) > ceiling + 2e-6
    ):
        raise ValueError("Invalid native alignment/output/sample ceiling")
    return raw.reshape(-1, signal.shape[1])[
        latency : latency + len(signal)
    ].copy(), metadata
