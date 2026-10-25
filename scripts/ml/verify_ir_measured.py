"""Replay measured-reference exports and verify native loading and decay reports."""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path

import numpy as np
from ir_denoise import load_model, propose, sha
from ir_measured_metrics import decay_error
from ir_preservation import load_policy
from scipy.io import wavfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--policy", type=Path, required=True)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--temporary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists() or args.temporary.exists():
        raise ValueError("preserve prior verification outputs")
    args.temporary.mkdir(parents=True)
    protocol = json.loads((args.evidence / "protocol.json").read_text())
    model = load_model(args.model, protocol["model_sha256"])
    policy_hash = protocol["policy_sha256"]["neural"]
    policy = load_policy(args.policy, policy_hash)
    if policy != protocol["policies"]["neural"]:
        raise ValueError("asset/experiment policy mismatch")
    replayed = 0
    for index in range(108):
        row = json.loads((args.evidence / f"case-{index:03d}.json").read_text())
        path = Path(row["source"]["path"])
        if sha(path) != row["source"]["sha256"]:
            raise ValueError("source hash mismatch")
        noisy = np.fromfile(path, dtype="<f4").reshape(row["frames"], 2)
        for method in ["neural", "wiener", "decay"]:
            try:
                output, checks, mask = propose(
                    noisy,
                    48000,
                    row["arrival"],
                    row["noise_frames"],
                    model,
                    method,
                    preservation_policy=protocol["policies"][method],
                )
            except ValueError as error:
                output, checks, mask = (
                    noisy.copy(),
                    {"accepted": False, "fallback_error": str(error)},
                    np.ones(len(noisy)),
                )
            if (
                hashlib.sha256(output.tobytes()).hexdigest()
                != row["arrays"][method]["sha256"]
                or checks != row["methods"][method]["checks"]
                or hashlib.sha256(np.asarray(mask, dtype="<f8").tobytes()).hexdigest()
                != row["arrays"][method + "_mask"]["sha256"]
            ):
                raise ValueError("final-source measured replay mismatch")
            replayed += 1
    records = []
    # Central annotation, 40 dB white noise, one case for each preselected room.
    for index in [7, 25, 43, 61, 79, 97]:
        row = json.loads((args.evidence / f"case-{index:03d}.json").read_text())
        noisy = np.fromfile(row["source"]["path"], dtype="<f4").reshape(
            row["frames"], 2
        )
        source = args.temporary / f"source-{index:03d}.wav"
        wavfile.write(source, 48000, noisy)
        for mode in ["guarded", "invalid_policy"]:
            output = args.temporary / f"{index:03d}-{mode}.wav"
            report_path = output.with_suffix(".json")
            command = [
                sys.executable,
                str(Path(__file__).with_name("ir_denoise.py")),
                "--input",
                str(source),
                "--output",
                str(output),
                "--report",
                str(report_path),
                "--model",
                str(args.model),
                "--model-sha256",
                protocol["model_sha256"],
                "--preservation-policy",
                str(args.policy),
                "--policy-sha256",
                policy_hash if mode == "guarded" else "0" * 64,
                "--arrival-ms",
                str(row["arrival"] * 1000 / 48000),
                "--noise-prefix-ms",
                "60",
            ]
            subprocess.run(command, check=True, capture_output=True, text=True)
            report = json.loads(report_path.read_text())
            rate, exported = wavfile.read(output)
            expected = (
                np.fromfile(row["arrays"]["neural"]["path"], dtype="<f4").reshape(
                    noisy.shape
                )
                if mode == "guarded"
                else noisy
            )
            if rate != 48000 or not np.array_equal(exported, expected):
                raise ValueError("CLI samples differ from frozen export/fallback")
            if report["observed_decay_change"] != decay_error(
                noisy, exported, 48000, row["arrival"]
            ):
                raise ValueError("observed decay report mismatch")
            if mode == "invalid_policy" and (
                report["used_learned_output"]
                or not report["fallback_error"]
                or report["observed_decay_change"]["rmse_db"] != 0
            ):
                raise ValueError("invalid identity did not retain exact original")
            for block in [193, 1024]:
                raw = args.temporary / f"{index:03d}-{mode}-{block}.f32le"
                request = {
                    "ir": str(output.resolve()),
                    "output": str(raw.resolve()),
                    "sample_rate": 48000,
                    "channels": 2,
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
                audio = np.fromfile(raw, dtype="<f4").astype(float).reshape(-1, 2)
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
                    or not np.isfinite(audio).all()
                    or native["frames"] != len(audio)
                    or error > 1e-6
                    or outside > 1e-6
                ):
                    raise ValueError("native measured-reference realization mismatch")
                records.append(
                    {
                        "case": index,
                        "room": row["room"],
                        "mode": mode,
                        "block_frames": block,
                        "max_error": error,
                        "outside_ir_max": outside,
                        "wav_sha256": sha(output),
                        "native_sha256": sha(raw),
                        "observed_decay_change": report["observed_decay_change"],
                        "used_learned_output": report["used_learned_output"],
                    }
                )
    report = {
        "model_sha256": protocol["model_sha256"],
        "policy_sha256": policy_hash,
        "renderer_sha256": sha(args.renderer),
        "replayed_arrays_and_masks": replayed,
        "native_runs": len(records),
        "max_native_error": max(r["max_error"] for r in records),
        "records": records,
        "source_sha256": {
            name: sha(Path(__file__).parent / name)
            for name in [
                Path(__file__).name,
                "ir_denoise.py",
                "ir_measured_metrics.py",
                "ir_preservation.py",
            ]
        },
        "scope": "48 kHz paired recorded-reference plus controlled added-noise WAV, unchanged UPC/diagonal routing, guarded callback allocations. Artificial prefix retained in test IR; not acquisition-noise/clean-room validation.",
    }
    args.output.write_text(json.dumps(report, indent=2, allow_nan=False) + "\n")
    print(json.dumps({k: v for k, v in report.items() if k != "records"}))


if __name__ == "__main__":
    main()
