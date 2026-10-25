"""Verify guarded IR CLI exports, policy fallback and native convolution realization."""

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
    parser.add_argument("--previous-evidence", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--temporary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists() or args.temporary.exists():
        raise ValueError("preserve prior verification")
    args.temporary.mkdir(parents=True)
    summary = json.loads((args.evidence / "summary.json").read_text())
    model = load_model(args.model, summary["model_sha256"])
    policy_file = args.evidence / "neural-policy.json"
    policy_hash = summary["policy_sha256"]["neural"]
    if sha(policy_file) != policy_hash:
        raise ValueError("selected policy hash mismatch")
    replayed = 0
    for index in range(78):
        row = json.loads(
            (args.previous_evidence / f"case-{index:03d}.json").read_text()
        )
        info = row["metadata"]
        path = Path(row["directory"]) / "noisy.f32le"
        if sha(path) != row["arrays"]["noisy"]["sha256"]:
            raise ValueError("legacy source hash mismatch")
        noisy = np.fromfile(path, dtype="<f4").reshape(info["frames"], info["channels"])
        for method in ["neural", "wiener", "decay"]:
            output, checks, _ = propose(
                noisy,
                info["rate"],
                info["arrival"],
                info["noise_frames"],
                model,
                method,
            )
            if (
                hashlib.sha256(output.tobytes()).hexdigest()
                != row["arrays"][method]["sha256"]
                or checks["accepted"] != row["methods"][method]["checks"]["accepted"]
            ):
                raise ValueError("optional preservation changed legacy outputs")
            replayed += 1
    guarded_replayed = 0
    for index in range(104):
        row = json.loads((args.evidence / f"case-{index:03d}.json").read_text())
        info = row["metadata"]
        path = Path(row["directory"]) / "noisy.f32le"
        if sha(path) != row["arrays"]["noisy"]["sha256"]:
            raise ValueError("guarded source hash mismatch")
        noisy = np.fromfile(path, dtype="<f4").reshape(info["frames"], info["channels"])
        for method in ["neural", "wiener", "decay"]:
            try:
                output, checks, _ = propose(
                    noisy,
                    info["rate"],
                    info["arrival"],
                    info["noise_frames"],
                    model,
                    method,
                    preservation_policy=summary["policies"][method],
                )
            except ValueError as error:
                output, checks = (
                    noisy.copy(),
                    {"accepted": False, "fallback_error": str(error)},
                )
            if (
                hashlib.sha256(output.tobytes()).hexdigest()
                != row["arrays"][method]["sha256"]
                or checks != row["methods"][method]["checks"]
            ):
                raise ValueError("final-source guarded replay differs")
            guarded_replayed += 1
    records = []
    # Fixed indices cover late reflections, late packets and linked four-channel
    # files at every final rate; choice is independent of metric ranking.
    for index in [12, 13, 14, 15, 72, 73, 74, 75, 96, 97, 98, 99]:
        row = json.loads((args.evidence / f"case-{index:03d}.json").read_text())
        info = row["metadata"]
        source_path = Path(row["directory"]) / "noisy.f32le"
        if sha(source_path) != row["arrays"]["noisy"]["sha256"]:
            raise ValueError("new source hash mismatch")
        noisy = np.fromfile(source_path, dtype="<f4").reshape(
            info["frames"], info["channels"]
        )
        source = args.temporary / f"source-{index:03d}.wav"
        wavfile.write(source, info["rate"], noisy)
        for mode in ["guarded", "bad_policy", "missing_model"]:
            wav = args.temporary / f"{index:03d}-{mode}.wav"
            report = wav.with_suffix(".json")
            command = [
                sys.executable,
                str(Path(__file__).with_name("ir_denoise.py")),
                "--input",
                str(source),
                "--output",
                str(wav),
                "--report",
                str(report),
                "--model",
                str(
                    args.model
                    if mode != "missing_model"
                    else args.temporary / "absent.json"
                ),
                "--model-sha256",
                summary["model_sha256"],
                "--preservation-policy",
                str(policy_file),
                "--policy-sha256",
                policy_hash if mode != "bad_policy" else "0" * 64,
                "--arrival-ms",
                "80",
                "--noise-prefix-ms",
                "60",
            ]
            subprocess.run(command, check=True, capture_output=True, text=True)
            rate, exported = wavfile.read(wav)
            if exported.ndim == 1:
                exported = exported[:, None]
            details = json.loads(report.read_text())
            expected = noisy
            if mode == "guarded":
                path = Path(row["directory"]) / "neural.f32le"
                if sha(path) != row["arrays"]["neural"]["sha256"]:
                    raise ValueError("guarded output hash mismatch")
                expected = np.fromfile(path, dtype="<f4").reshape(noisy.shape)
                if details["checks"] != row["methods"]["neural"]["checks"]:
                    raise ValueError("CLI preservation decisions differ")
            elif not details["fallback_error"] or details["used_learned_output"]:
                raise ValueError(
                    "identity/preparation error did not trigger exact fallback"
                )
            if rate != info["rate"] or not np.array_equal(exported, expected):
                raise ValueError(
                    "CLI WAV differs from verified guarded/fallback samples"
                )
            for block in [193, 1024]:
                raw = args.temporary / f"{index:03d}-{mode}-{block}.f32le"
                request = {
                    "ir": str(wav.resolve()),
                    "output": str(raw.resolve()),
                    "sample_rate": rate,
                    "channels": info["channels"],
                    "ir_frames": len(exported),
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
                native = json.loads(completed.stdout)
                audio = (
                    np.fromfile(raw, dtype="<f4")
                    .astype(float)
                    .reshape(-1, info["channels"])
                )
                latency = native["latency_samples"]
                error = float(
                    np.max(np.abs(audio[latency : latency + len(exported)] - exported))
                )
                outside = max(
                    float(np.max(np.abs(audio[:latency]))),
                    float(np.max(np.abs(audio[latency + len(exported) :]))),
                )
                if (
                    latency != 1024
                    or len(audio) != native["frames"]
                    or not np.isfinite(audio).all()
                    or error > 1e-6
                    or outside > 1e-6
                ):
                    raise ValueError("native IR realization mismatch")
                records.append(
                    {
                        "case": index,
                        "mode": mode,
                        "sample_rate": rate,
                        "channels": info["channels"],
                        "block_frames": block,
                        "output_wav_sha256": sha(wav),
                        "native_output_sha256": sha(raw),
                        "max_error": error,
                        "outside_ir_max": outside,
                        "used_learned_output": details["used_learned_output"],
                    }
                )
    report = {
        "model_sha256": summary["model_sha256"],
        "policy_sha256": policy_hash,
        "renderer_sha256": sha(args.renderer),
        "legacy_arrays_identical": replayed,
        "guarded_arrays_and_decisions_identical": guarded_replayed,
        "native_runs": len(records),
        "wav_exports": len(records) // 2,
        "max_native_error": max(r["max_error"] for r in records),
        "records": records,
        "source_sha256": {
            name: sha(Path(__file__).parent / name)
            for name in [Path(__file__).name, "ir_denoise.py", "ir_preservation.py"]
        },
        "scope": "Public UPC, full wet/unity gain, diagonal/cyclic routing, allocation-guarded callbacks. Mono/stereo/linked four-channel exports at 16/44.1/48/96 kHz. No true-stereo or DAW deadline claim.",
    }
    args.output.write_text(json.dumps(report, indent=2, allow_nan=False) + "\n")
    print(json.dumps({k: v for k, v in report.items() if k != "records"}))


if __name__ == "__main__":
    main()
