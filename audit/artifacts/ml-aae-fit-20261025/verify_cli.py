"""Run from the repository root using the pinned ML Python environment."""

import json
import hashlib
import os
import subprocess
import sys
import time
from pathlib import Path

import numpy as np
from scipy.io import wavfile

sys.path.insert(0, "scripts/ml")
from aae_room_features import Renderer, actual_unit

ROOT = Path(__file__).parent.resolve()
MODEL = Path("crates/sotf-plugins/models/aae-room-fit")
BINARY = Path("/Volumes/home_tmp/tmp/sotf-daw-audit-20261018-target/release/qa-aae-fit-render")

row = json.loads((ROOT / "corpus/development.json").read_text())["records"][0]
params = row["params"]
with Renderer(BINARY, ROOT / "cli-renders") as renderer:
    audio, metadata = renderer.render(
        actual_unit(params), params["speaker_config"], params["room_preset"]
    )
    wavfile.write(ROOT / "cli-target.wav", 16000, audio.astype(np.float32))

results = []
for variant in ["no-model", "invalid-hash", "invalid-structure", "valid-model"]:
    output = ROOT / ("cli-" + variant)
    output.mkdir(exist_ok=True)
    command = [
        "/usr/bin/time", "-l", sys.executable, "scripts/ml/fit_aae_room.py",
        "--ir", str(ROOT / "cli-target.wav"), "--direct-sample", "0",
        "--renderer", str(BINARY), "--temporary", str(ROOT / "cli-renders"),
        "--layout", params["speaker_config"], "--dictionary", str(MODEL / "native_dictionary.npz"),
        "--budget", "8", "--report", str(output / "report.json"),
        "--preset", str(output / "preset.json"),
    ]
    if variant != "no-model":
        sha = "0" * 64 if variant == "invalid-hash" else (MODEL / "model.sha256").read_text().split()[0]
        model_path = MODEL / "model.json"
        if variant == "invalid-structure":
            model_path = output / "invalid-model.json"
            model_path.write_text("[]\n")
            sha = hashlib.sha256(model_path.read_bytes()).hexdigest()
        command.extend(["--model", str(model_path), "--model-sha256", sha])
    start = time.perf_counter()
    process = subprocess.run(
        command, capture_output=True, text=True,
        env=dict(os.environ, OPENBLAS_NUM_THREADS="1", OMP_NUM_THREADS="1", VECLIB_MAXIMUM_THREADS="1"),
    )
    (output / "stdout.txt").write_text(process.stdout)
    (output / "time-stderr.txt").write_text(process.stderr)
    assert process.returncode == 0, process.stderr
    report = json.loads((output / "report.json").read_text())
    assert not report["automatic_apply"]
    if variant != "valid-model":
        assert report["selected_method"] == "classical_native"
        assert report["neural"] is None
    if variant == "invalid-hash":
        assert "hash" in report["model_error"]
    if variant == "invalid-structure":
        assert "JSON object" in report["model_error"]
    if variant == "valid-model":
        assert report["neural"] is not None
        assert report["neural_fit_error"] is None
    for method in ["classical", "neural"]:
        if report[method] is not None:
            assert report[method]["native_evaluations"] <= 8
            assert report[method]["best"]["native_tail"] == "Infinite"
    assert (output / "preset.json").exists() == report["qualified_profile_proxy"]
    results.append({"variant": variant, "command": command, "exit_code": process.returncode,
                    "seconds": time.perf_counter() - start, "selected": report["selected_method"],
                    "qualified": report["qualified_profile_proxy"]})
(ROOT / "cli-checks.json").write_text(json.dumps(results, indent=2) + "\n")
(ROOT / "cli-target.wav").unlink()
print(json.dumps(results, indent=2))
