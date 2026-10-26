"""Verify same-network runtime parity, policy stability and measured-call timing."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from crepe_runtime_corpus import corpus


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def decode(row):
    # Rust Iterator::max_by selects the last equal maximum.
    peak = int(np.flatnonzero(row == row.max())[-1])
    weight = cents = 0.0
    for i in range(max(0, peak - 4), min(360, peak + 5)):
        weight += float(row[i])
        cents += float(row[i]) * (1997.379408437619 + 20 * i)
    hz = float(np.float32(10 * 2 ** (cents / weight / 1200))) if weight > 0 else 0.0
    return hz, float(row[peak]), peak


def accepted(hz, confidence, reference):
    if (
        confidence < float(np.float32(0.8))
        or not 32.7 <= hz <= 2006
        or not 32.7 <= reference <= 2006
    ):
        return False
    cents = np.float32(1200) * np.log2(np.float32(hz / np.float32(reference)))
    return bool(abs(cents) <= 100)


def parity(reference, candidate, labels):
    if (
        reference.shape != candidate.shape
        or reference.shape != (len(labels), 360)
        or not np.isfinite(reference).all()
        or np.any((reference < 0) | (reference > 1))
        or not np.isfinite(candidate).all()
        or np.any((candidate < 0) | (candidate > 1))
    ):
        raise ValueError("Invalid matched activation arrays")
    rows = []
    for a, b, label in zip(reference, candidate, labels, strict=True):
        ah, ac, ap = decode(a)
        bh, bc, bp = decode(b)
        ad, bd = (
            accepted(ah, ac, label["reference_hz"]),
            accepted(bh, bc, label["reference_hz"]),
        )
        rows.append(
            {
                "maximum_activation_error": float(abs(a.astype(float) - b).max()),
                "reference_peak": ap,
                "candidate_peak": bp,
                "reference_confidence": ac,
                "candidate_confidence": bc,
                "policy_disagreement": ad != bd,
                "accepted_pitch_error_cents": abs(float(1200 * np.log2(bh / ah)))
                if (ad or bd) and min(ah, bh) > 0
                else None,
            }
        )
    maximum = max(r["maximum_activation_error"] for r in rows)
    decisions = sum(r["policy_disagreement"] for r in rows)
    pitch = max((r["accepted_pitch_error_cents"] or 0.0 for r in rows), default=0.0)
    return {
        "maximum_activation_error": maximum,
        "policy_disagreements": decisions,
        "maximum_accepted_pitch_error_cents": pitch,
        "peak_disagreements": sum(
            r["reference_peak"] != r["candidate_peak"] for r in rows
        ),
        "gate_pass": maximum <= 2e-5 and decisions == 0 and pitch <= 0.1,
        "frames": rows,
    }


def timing(native, request):
    values = native["timings_ns"]
    if (
        native["frames"] != request["frames"]
        or native["measured_cycles"] != request["measured_cycles"]
        or native["warmup_cycles"] != request["warmup_cycles"]
        or len(values) != request["frames"] * request["measured_cycles"]
    ):
        raise ValueError("Missing/mismatched measured-call timing rows")
    if (
        any(type(v) is not int or v <= 0 for v in values)
        or native["measurement_wall_ns"] < sum(values)
        or native["ready_from_entry_ns"] < native["model_load_ns"]
    ):
        raise ValueError("Invalid timing units/arithmetic")
    return {
        "measured_calls": len(values),
        "median_ms": float(np.median(values) / 1e6),
        "p99_ms": float(np.quantile(values, 0.99) / 1e6),
        "maximum_ms": max(values) / 1e6,
        "model_load_ms": native["model_load_ns"] / 1e6,
        "ready_from_entry_ms": native["ready_from_entry_ns"] / 1e6,
        "first_inference_ms": native["first_inference_ns"] / 1e6,
    }


def analyze(root):
    report = json.loads((root / "report.json").read_text())
    if (
        digest(root / "tract.frozen") != report["renderer_sha256"]
        or digest(root / "ort_worker.frozen.py") != report["ort_worker_sha256"]
        or digest(root / "crepe-tiny.frozen.onnx") != report["model_sha256"]
        or digest(root / "corpus.f32le") != report["input_sha256"]
    ):
        raise ValueError("Frozen runtime/model/input binding mismatch")
    frames, labels = corpus()
    if (
        labels != report["labels"]
        or hashlib.sha256(frames.astype("<f4").tobytes()).hexdigest()
        != report["input_sha256"]
    ):
        raise ValueError("Regenerated corpus mismatch")
    profiles = {"tract_native", "ort_cpu", "coreml_cpu", "coreml_gpu_allowed"}
    trials = 1 if report["smoke"] else 3
    expected = {(trial, p, False) for trial in range(trials) for p in profiles} | {
        (None, p, True) for p in profiles if p != "tract_native"
    }
    actual = [(r["trial"], r["profile"], r["placement_only"]) for r in report["runs"]]
    if len(actual) != len(expected) or set(actual) != expected:
        raise ValueError("Incomplete/duplicated runtime matrix")
    baseline = next(
        (
            r
            for r in report["runs"]
            if r["profile"] == "tract_native" and r["exit_code"] == 0
        ),
        None,
    )
    if baseline is None:
        raise ValueError("No valid native baseline")
    reference = np.fromfile(
        baseline["files"]["activations.f32le"]["path"], dtype="<f4"
    ).reshape(len(labels), 360)
    results, hashes = [], 0
    for row in report["runs"]:
        for entry in row["files"].values():
            if digest(entry["path"]) != entry["sha256"]:
                raise ValueError("Raw runtime evidence hash mismatch")
            hashes += 1
        if row["exit_code"]:
            results.append(
                {
                    "profile": row["profile"],
                    "trial": row["trial"],
                    "placement_only": row["placement_only"],
                    "exit_code": row["exit_code"],
                    "qualified_for_runtime_comparison": False,
                }
            )
            continue
        native = json.loads(Path(row["files"]["native.json"]["path"]).read_text())
        request = json.loads(Path(row["files"]["request.json"]["path"]).read_text())
        if (
            digest(request["input"]) != report["input_sha256"]
            or native["model_sha256"] != report["model_sha256"]
            or request["sha256"] != report["model_sha256"]
        ):
            raise ValueError("Backend corpus/model mismatch")
        if (
            native["backend"] != row["profile"]
            or bool(native.get("placement_only", False)) != row["placement_only"]
        ):
            raise ValueError("Backend label or profiling/timing mode mismatch")
        if row["profile"] != "tract_native" and (
            native["onnxruntime_version"],
            native["numpy_version"],
        ) != ("1.23.2", "2.5.4"):
            raise ValueError(
                "Runtime package versions differ from the frozen experiment"
            )
        if row["profile"] == "ort_cpu" and native["registered_providers"] != [
            "CPUExecutionProvider"
        ]:
            raise ValueError("CPU baseline registered an unexpected provider")
        if row["profile"].startswith("coreml_"):
            expected_units = (
                "CPUOnly" if row["profile"] == "coreml_cpu" else "CPUAndGPU"
            )
            if (
                "CoreMLExecutionProvider" not in native["registered_providers"]
                or native["requested_providers"][0][1]["MLComputeUnits"]
                != expected_units
            ):
                raise ValueError(
                    "CoreML request/registration does not match the profile"
                )
        activation = np.fromfile(
            row["files"]["activations.f32le"]["path"], dtype="<f4"
        ).reshape(len(labels), 360)
        checked = parity(reference, activation, labels)
        result = {
            "profile": row["profile"],
            "trial": row["trial"],
            "placement_only": row["placement_only"],
            "exit_code": 0,
            "parity": checked,
            "whole_process_peak_rss_bytes": row["whole_process_peak_rss_bytes"],
            "qualified_for_runtime_comparison": checked["gate_pass"],
        }
        if row["placement_only"]:
            profile = Path(native["profile_file"])
            if digest(profile) != native["profile_sha256"]:
                raise ValueError("Placement profile hash mismatch")
            events = json.loads(profile.read_text())
            counts = {}
            for event in events:
                provider = event.get("args", {}).get("provider")
                if event.get("cat") == "Node" and provider:
                    counts[provider] = counts.get(provider, 0) + 1
            if counts != native["node_kernel_events_by_provider"]:
                raise ValueError("Reported provider placement differs from raw profile")
            result["node_kernel_events_by_provider"] = counts
            result["actual_gpu_execution_proven"] = False
        else:
            result["timing"] = timing(native, request)
            if (
                row["process_total_wall_ns"]
                < native["ready_from_entry_ns"] + native["measurement_wall_ns"]
            ):
                raise ValueError("Whole-process timing arithmetic mismatch")
        results.append(result)
    return {
        "smoke": report["smoke"],
        "verified_raw_hashes": hashes,
        "activation_tolerance": 2e-5,
        "accepted_pitch_tolerance_cents": 0.1,
        "policy_disagreement_tolerance": 0,
        "results": results,
        "report_sha256": digest(root / "report.json"),
        "scope": "operational native/Python wrappers and whole-process memory; same graph/input; differing startup/validation/threading boundaries; no runtime winner or actual GPU-device proof",
        "deployment_admitted": False,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = analyze(args.root)
    with args.output.open("x") as stream:
        stream.write(json.dumps(result, indent=2, allow_nan=False) + "\n")
    print(
        json.dumps(
            {
                "runs": len(result["results"]),
                "failed_processes": sum(r["exit_code"] != 0 for r in result["results"]),
                "parity_failures": sum(
                    not r.get("parity", {}).get("gate_pass", False)
                    for r in result["results"]
                    if r["exit_code"] == 0
                ),
            }
        )
    )


if __name__ == "__main__":
    main()
