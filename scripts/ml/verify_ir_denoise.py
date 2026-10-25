"""Replay frozen IR outputs and check reviewed WAVs through native convolution."""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path

import numpy as np
from ir_denoise import load_model, propose, sha
from scipy.io import wavfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--temporary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists() or args.temporary.exists():
        raise ValueError("preserve prior verification data")
    args.temporary.mkdir(parents=True)
    model_path = args.evidence / "model.json"
    expected = json.loads((args.evidence / "training.json").read_text())["model_sha256"]
    model = load_model(model_path, expected)
    replayed = 0
    for index in range(78):
        row = json.loads((args.evidence / f"case-{index:03d}.json").read_text())
        info = row["metadata"]
        path = Path(row["directory"]) / "noisy.f32le"
        if sha(path) != row["arrays"]["noisy"]["sha256"]:
            raise ValueError("source hash mismatch")
        noisy = np.fromfile(path, dtype="<f4").reshape(info["frames"], info["channels"])
        for method in ["neural", "wiener", "decay"]:
            result, checks, _ = propose(
                noisy,
                info["rate"],
                info["arrival"],
                info["noise_frames"],
                model,
                method,
            )
            raw = result.tobytes()
            if hashlib.sha256(raw).hexdigest() != row["arrays"][method]["sha256"]:
                raise ValueError(
                    "final export changed after numerical/report-only fixes"
                )
            if checks["accepted"] != row["methods"][method]["checks"]["accepted"]:
                raise ValueError("admission decision changed")
            replayed += 1
    records = []
    for index in [0, 1, 52, 53, 76, 77]:
        row = json.loads((args.evidence / f"case-{index:03d}.json").read_text())
        info = row["metadata"]
        noisy = np.fromfile(
            Path(row["directory"]) / "noisy.f32le", dtype="<f4"
        ).reshape(info["frames"], info["channels"])
        source = args.temporary / f"source-{index:03d}.wav"
        wavfile.write(source, info["rate"], noisy)
        for mode in ["model", "missing"]:
            output = args.temporary / f"{index:03d}-{mode}.wav"
            report = output.with_suffix(".json")
            command = [
                sys.executable,
                str(Path(__file__).with_name("ir_denoise.py")),
                "--input",
                str(source),
                "--output",
                str(output),
                "--report",
                str(report),
                "--model",
                str(model_path if mode == "model" else args.temporary / "absent.json"),
                "--model-sha256",
                expected,
                "--arrival-ms",
                "80",
                "--noise-prefix-ms",
                "60",
            ]
            subprocess.run(command, check=True, capture_output=True, text=True)
            exported_rate, exported = wavfile.read(output)
            if exported.ndim == 1:
                exported = exported[:, None]
            details = json.loads(report.read_text())
            target = (
                np.fromfile(
                    Path(row["directory"]) / "neural.f32le", dtype="<f4"
                ).reshape(noisy.shape)
                if mode == "model"
                else noisy
            )
            if exported_rate != info["rate"] or not np.array_equal(exported, target):
                raise ValueError(
                    "WAV export does not match frozen array or exact fallback"
                )
            for block in [1, 193, 1024]:
                raw = args.temporary / f"{index:03d}-{mode}-{block}.f32le"
                request = {
                    "ir": str(output.resolve()),
                    "output": str(raw.resolve()),
                    "sample_rate": info["rate"],
                    "channels": info["channels"],
                    "ir_frames": info["frames"],
                    "block_frames": block,
                }
                request_path = raw.with_suffix(".request.json")
                request_path.write_text(json.dumps(request))
                completed = subprocess.run(
                    [str(args.renderer.resolve()), str(request_path.resolve())],
                    check=True,
                    capture_output=True,
                    text=True,
                )
                metadata = json.loads(completed.stdout)
                samples = (
                    np.fromfile(raw, dtype="<f4")
                    .astype(float)
                    .reshape(-1, info["channels"])
                )
                latency = metadata["latency_samples"]
                if latency != 1024 or metadata["frames"] != len(samples):
                    raise ValueError("native geometry/latency mismatch")
                error = float(
                    np.max(
                        np.abs(samples[latency : latency + len(exported)] - exported)
                    )
                )
                outside = max(
                    float(np.max(np.abs(samples[:latency]))),
                    float(np.max(np.abs(samples[latency + len(exported) :]))),
                )
                if not np.isfinite(samples).all() or error > 1e-6 or outside > 1e-6:
                    raise ValueError("native convolution IR load/render mismatch")
                records.append(
                    {
                        "case": index,
                        "mode": mode,
                        "block_frames": block,
                        "wav_sha256": sha(output),
                        "raw_sha256": sha(raw),
                        "model_used": details["used_learned_output"],
                        "max_error": error,
                        "outside_ir_max": outside,
                        "native": metadata,
                    }
                )
    report = {
        "model_sha256": expected,
        "renderer_sha256": sha(args.renderer),
        "replayed_export_arrays": replayed,
        "native_runs": len(records),
        "max_native_error": max(r["max_error"] for r in records),
        "source_sha256": {
            name: sha(Path(__file__).parent / name)
            for name in [Path(__file__).name, "ir_denoise.py"]
        },
        "records": records,
        "scope": "Synthetic mono/stereo float32 WAV, 16/48 kHz, UPC diagonal routing, callbacks 1/193/1024, guarded callback allocations. No native DSP or automatic IR substitution changed.",
    }
    args.output.write_text(json.dumps(report, indent=2, allow_nan=False) + "\n")
    print(json.dumps({k: v for k, v in report.items() if k != "records"}))


if __name__ == "__main__":
    main()
