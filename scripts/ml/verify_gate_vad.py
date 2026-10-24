#!/usr/bin/env python3
"""Verify pinned VAD provenance, deterministic specialization, and ORT golden data.

Native parity is a separate qa-gate-silero command; this verifier does not turn
saved native results into a claim about the current native executable.
"""

import hashlib
import json
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np
import onnxruntime as ort
from evaluate_gate_vad import replay

ROOT = Path(__file__).resolve().parents[2]
MODEL = ROOT / "crates/sotf-plugins/models/silero-vad"
ARTIFACTS = ROOT / "audit/artifacts/ml-gate-silero-20261025/fixed-batch"
DERIVED_SHA = "7d47a98acce5f4b688a7d541ee0d9061b64171fdb39581cbcf29fe72bfe57c51"


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    manifest = json.loads((MODEL / "manifest.json").read_text())
    if manifest["commit"] != "7e30209a3e901f9842f81b225f3e93d8199902b1":
        raise ValueError("Source commit mismatch")
    for name, expected in manifest["files"].items():
        path = MODEL / name
        if sha(path) != expected["sha256"] or path.stat().st_size != expected["bytes"]:
            raise ValueError(f"Source identity mismatch: {name}")
    derived = MODEL / "silero_vad_16k_batch1.onnx"
    if (
        sha(derived) != DERIVED_SHA
        or manifest["derived"]["output_sha256"] != DERIVED_SHA
    ):
        raise ValueError("Derived model identity mismatch")
    recipe_script = ROOT / manifest["derived"]["recipe_script"]
    if sha(recipe_script) != manifest["derived"]["recipe_script_sha256"]:
        raise ValueError("Specialization recipe changed")
    scratch_root = Path("/Volumes/home_tmp/tmp")
    with tempfile.TemporaryDirectory(
        prefix="sotf-gate-vad-verify-",
        dir=scratch_root if scratch_root.exists() else None,
    ) as temporary:
        reproduced = Path(temporary) / "fixed.onnx"
        subprocess.run(
            [
                sys.executable,
                str(recipe_script),
                str(MODEL / "silero_vad_op18_ifless.onnx"),
                str(reproduced),
            ],
            check=True,
        )
        if sha(reproduced) != DERIVED_SHA:
            raise ValueError("Specialization is not byte-reproducible")
    report = json.loads((ARTIFACTS / "comparison.json").read_text())
    if report["script_sha256"] != sha(ROOT / "scripts/ml/evaluate_gate_vad.py"):
        raise ValueError("Evaluation recipe changed")
    for row in report["measurements"]:
        if sha(ROOT / row["source_path"]) != row["source_sha256"]:
            raise ValueError("Development fixture identity mismatch")
    golden = json.loads((ARTIFACTS / "ort-golden.json").read_text())
    if golden["model_sha256"] != DERIVED_SHA or len(golden["cases"]) != 16:
        raise ValueError("Golden identity or case count mismatch")
    options = ort.SessionOptions()
    options.intra_op_num_threads = 1
    options.inter_op_num_threads = 1
    session = ort.InferenceSession(
        str(derived), options, providers=["CPUExecutionProvider"]
    )
    score_error = 0.0
    state_error = 0.0
    for case in golden["cases"]:
        audio = np.array(case["audio_16k"], np.float32)
        expected_scores = np.array(case["step_scores"], np.float32)
        expected_state = np.array(case["final_state"], np.float32)
        if (
            not 1 <= len(expected_scores) <= 128
            or audio.shape != (64 + 512 * len(expected_scores),)
            or expected_state.shape != (256,)
            or not all(
                np.isfinite(v).all() for v in [audio, expected_scores, expected_state]
            )
            or not ((expected_scores >= 0) & (expected_scores <= 1)).all()
        ):
            raise ValueError("Invalid reference shape or values")
        scores, state = replay(session, audio)
        score_error = max(
            score_error, float(np.max(np.abs(np.array(scores) - case["step_scores"])))
        )
        state_error = max(
            state_error, float(np.max(np.abs(state.reshape(-1) - case["final_state"])))
        )
    if score_error > 1e-6 or state_error > 1e-5:
        raise ValueError("Reference golden replay changed")
    print(
        json.dumps(
            {
                "source_files_verified": len(manifest["files"]),
                "specialization_byte_reproducible": True,
                "golden_cases_replayed": len(golden["cases"]),
                "score_error": score_error,
                "state_error": state_error,
                "scope": "Provenance, specialization, fixture identity, and independent runtime golden replay. Native parity and realtime worker integration are separate.",
            }
        )
    )


if __name__ == "__main__":
    main()
