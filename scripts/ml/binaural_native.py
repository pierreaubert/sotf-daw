"""Fixed-pose native SOFA audition renders for optional offline personalization."""

import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np

SUPPORTED_CHANNELS = {1, 2, 3, 5, 6, 8, 10, 12, 14, 16}
MAX_SAMPLES = 4_000_000
MAX_SOFA_BYTES = 16_777_216


def render(
    signal, renderer, sofa, work, rate=48000, fft_size=2048, callback=193, eq=False
):
    """Return delay-aligned audio including the native tail, never silently crop it."""
    x = np.asarray(signal, dtype=np.float32)
    if x.ndim == 1:
        x = x[:, None]
    if (
        x.ndim != 2
        or x.shape[1] not in SUPPORTED_CHANNELS
        or not 1 <= x.size <= MAX_SAMPLES
        or not np.isfinite(x).all()
        or np.max(np.abs(x)) > 32
        or type(rate) is not int
        or not 44100 <= rate <= 192000
        or type(fft_size) is not int
        or not 512 <= fft_size <= 8192
        or fft_size & (fft_size - 1)
        or type(callback) is not int
        or not 1 <= callback <= 4096
        or type(eq) is not bool
    ):
        raise ValueError("Use supported bounded binaural audio and geometry")
    # Hash and render one snapshot, preventing path changes between these steps.
    # This caps file bytes, not the SOFA parser's expanded memory or total RSS.
    with Path(sofa).open("rb") as source:
        sofa_bytes = source.read(MAX_SOFA_BYTES + 1)
    if not 1 <= len(sofa_bytes) <= MAX_SOFA_BYTES:
        raise ValueError("SOFA must contain 1..16 MiB")
    work = Path(work)
    work.mkdir(parents=True, exist_ok=False)
    snapshot = work / "candidate.sofa"
    snapshot.write_bytes(sofa_bytes)
    source = x.astype("<f4").tobytes()
    (work / "input.f32le").write_bytes(source)
    request = {
        "input": str((work / "input.f32le").resolve()),
        "output": str((work / "output.f32le").resolve()),
        "hrtf": str(snapshot.resolve()),
        "sample_rate": rate,
        "channels": x.shape[1],
        "frames": len(x),
        "callback_frames": callback,
        "fft_size": fft_size,
        "diffuse_field_eq": eq,
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
    (work / "stderr.txt").write_text(result.stderr)
    if result.returncode:
        raise ValueError("Native binaural render failed: " + result.stderr)
    metadata = json.loads(result.stdout)
    latency, drained = metadata["latency_samples"], metadata["drained_frames"]
    if (
        type(latency) is not int
        or type(drained) is not int
        or latency != fft_size
        or not latency <= drained <= 65536
        or metadata["status"] != "ok"
        or metadata["input_frames"] != len(x)
        or metadata["input_channels"] != x.shape[1]
        or metadata["output_channels"] != 2
        or metadata["sample_rate"] != rate
        or metadata["output_frames"] != len(x) + drained
        or metadata["hrtf_file_bytes"] != len(sofa_bytes)
        or metadata["requested_params"]["room_model"]["max_order"] != 0
        or metadata["requested_params"]["late_reverb_enabled"] is not False
        or metadata["requested_params"]["hrtf_database_dir"] != ""
        or metadata["requested_params"]["srir_file"] != ""
    ):
        raise ValueError("Invalid native binaural geometry/delay")
    output = work / "output.f32le"
    if output.stat().st_size != (len(x) + drained) * 8:
        raise ValueError("Invalid native binaural output byte count")
    raw = np.fromfile(output, dtype="<f4").reshape(-1, 2)
    if not np.isfinite(raw).all():
        raise ValueError("Native binaural output must be finite")
    metadata.update(
        {
            "sofa_sha256": hashlib.sha256(sofa_bytes).hexdigest(),
            "input_sha256": hashlib.sha256(source).hexdigest(),
            "output_sha256": hashlib.sha256(output.read_bytes()).hexdigest(),
            "aligned_output_frames": len(raw) - latency,
            "alignment": "Remove declared latency only; retain full public tail",
        }
    )
    (work / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    return raw[latency:].copy(), metadata
