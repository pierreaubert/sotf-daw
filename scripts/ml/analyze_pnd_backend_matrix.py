"""Verify backend/load identities, parity, actual neural service and PND audio."""

import argparse
import json
from pathlib import Path

import numpy as np
from analyze_pnd_comparison import analyze, digest
from pnd_compare_cases import KINDS
from run_pnd_backend_matrix import BACKENDS, LOADS


def parity(reference, candidate):
    """Compare exact windows and confidence-qualified pitch, never rejected zeros."""
    clocks = lambda rows: [(r["source_end_frame"], r["window_frames"]) for r in rows]
    if clocks(reference) != clocks(candidate):
        raise ValueError("Raw backend windows differ")
    a, b = [
        np.array([[r["frequency_hz"], r["confidence"]] for r in rows])
        for rows in [reference, candidate]
    ]
    if not np.isfinite(a).all() or not np.isfinite(b).all():
        raise ValueError("Non-finite raw estimator evidence")
    accepted_a = (a[:, 1] >= 0.8) & (a[:, 0] > 0)
    accepted_b = (b[:, 1] >= 0.8) & (b[:, 0] > 0)
    both = accepted_a & accepted_b
    confidence = float(np.max(abs(a[:, 1] - b[:, 1])))
    cents = (
        float(np.max(abs(1200 * np.log2(a[both, 0] / b[both, 0]))))
        if both.any()
        else None
    )
    return {
        "max_confidence_difference": confidence,
        "jointly_accepted_windows": int(both.sum()),
        "acceptance_disagreements": int(np.count_nonzero(accepted_a != accepted_b)),
        "max_accepted_pitch_difference_cents": cents,
        "passed": confidence <= 2e-5
        and not np.any(accepted_a != accepted_b)
        and (cents is None or cents <= 0.1),
    }


def verify(root):
    report = json.loads((root / "report.json").read_text())
    if report["smoke"]:
        raise ValueError("Smoke results cannot establish backend matrix coverage")
    protocol = json.loads((root / "protocol.json").read_text())
    if digest(root / "protocol.json") != report["protocol_sha256"]:
        raise ValueError("Protocol hash mismatch")
    if (
        protocol["confidence_parity_tolerance"] != 2e-5
        or protocol["accepted_pitch_parity_cents"] != 0.1
    ):
        raise ValueError("Parity gates changed")
    for name, expected in report["source_sha256"].items():
        if digest(root / (name + ".txt")) != expected:
            raise ValueError("Frozen source hash mismatch")
        if digest(Path(__file__).parent / name) != expected:
            raise ValueError("Analysis/runner source changed since freeze")
    if digest(report["ort_library"]) != report["ort_sha256"]:
        raise ValueError("ORT library changed")
    plan = [tuple(p) for p in protocol["plan"]]
    actual = [
        (r["identity"]["seed"], r["identity"]["kind"], r["backend"], r["cpu_workers"])
        for r in report["cases"]
    ]
    expected = {
        (seed, kind, backend, load)
        for seed in [20308120, 20308121]
        for kind in KINDS
        for backend in BACKENDS
        for load in LOADS
    }
    if len(actual) != len(expected) or set(actual) != expected:
        raise ValueError("Incomplete backend/load/source matrix")
    if actual != plan or len(set(actual)) != len(actual):
        raise ValueError("Recorded execution order/coverage differs from plan")
    if any(r["process"]["exit_code"] != 0 for r in report["cases"]):
        raise ValueError("Failed process cannot receive a runtime score")
    if any(
        r["process"]["build_processes_before"] or r["process"]["build_processes_after"]
        for r in report["cases"]
    ):
        raise ValueError(
            "Build/test-contended cases cannot establish timing comparisons"
        )
    groups, raw = [], {}
    for backend in BACKENDS:
        for load in LOADS:
            rows = [
                r
                for r in report["cases"]
                if r["backend"] == backend and r["cpu_workers"] == load
            ]
            # The original analyzer enforces all16 source identities and every
            # original estimator, phase-probe, policy, timing and audio invariant.
            result = analyze(root, {**report, "cases": rows})
            for row, scored in zip(rows, result["cases"], strict=True):
                meta = json.loads(Path(row["files"]["native.json"]["path"]).read_text())
                request = json.loads(
                    Path(row["files"]["request.json"]["path"]).read_text()
                )
                if (
                    meta["cpu_workers"] != load
                    or request["cpu_workers"] != load
                    or meta["raw_crepe_backend"] != backend
                    or meta["prepared_worker_backend"] != backend
                    or meta["ort_runtime"] != request["ort_runtime"]
                ):
                    raise ValueError("Requested/prepared/raw backend or load differs")
                runtime = meta["ort_runtime"]
                if backend == "tract":
                    if runtime is not None:
                        raise ValueError("Tract selected ORT")
                elif (
                    runtime["library_sha256"] != report["ort_sha256"]
                    or Path(runtime["library_path"]).resolve()
                    != Path(report["ort_library"]).resolve()
                    or runtime["provider"]
                    != ("cpu" if backend == "ort_cpu" else "core_ml_cpu_only")
                ):
                    raise ValueError("ORT library/provider binding differs")
                if any(
                    t["neural"]["neural_backend"] != backend
                    or t["classical"]["neural_backend"] is not None
                    for t in meta["trace"]
                ):
                    raise ValueError("Worker backend identity changed during pacing")
                key = (row["identity"]["seed"], row["identity"]["kind"], backend, load)
                raw[key] = (row, meta)
                neural = scored["methods"]["neural"]
                selected = neural["selected_neural_decisions"]
                total = neural["settled_control_decisions"]
                scored["selected_neural_fraction"] = selected / total
                # Offset probes are deliberately dense and reference-compatible.
                # Other case confidence/fallback counts remain visible, unscored.
                scored["dense_service_probe"] = row["identity"]["kind"] == "offset"
                scored["dense_service_passed"] = (
                    (
                        scored["raw_crepe"]["confidence_accepted_fraction"] >= 0.9
                        and selected / total >= 0.8
                    )
                    if scored["dense_service_probe"]
                    else None
                )
                scored["whole_process_peak_rss_bytes"] = row["process"][
                    "whole_process_peak_rss_bytes"
                ]
            groups.append({"backend": backend, "cpu_workers": load, **result})
    comparisons = []
    for (seed, kind, backend, load), (row, meta) in raw.items():
        base, base_meta = raw[seed, kind, "tract", 0]
        baseline_equal = (
            row["files"]["classical.f32le"]["sha256"]
            == base["files"]["classical.f32le"]["sha256"]
        )
        if not baseline_equal or meta["raw_classical"] != base_meta["raw_classical"]:
            raise ValueError("Classical baseline changed across backend/load cases")
        comparisons.append(
            {
                "seed": seed,
                "kind": kind,
                "backend": backend,
                "cpu_workers": load,
                "classical_bytes_exact": baseline_equal,
                **parity(base_meta["raw_crepe"], meta["raw_crepe"]),
            }
        )
    return {
        "groups": groups,
        "raw_backend_parity": comparisons,
        "all_parity_passed": all(r["passed"] for r in comparisons),
        "all_dense_service_probes_passed": all(
            c["dense_service_passed"]
            for g in groups
            for c in g["cases"]
            if c["dense_service_probe"]
        ),
        "report_sha256": digest(root / "report.json"),
        "scope": "96 local fresh processes; actual selected control decisions are not distinct jobs; no inference completion age, worker-only memory, hardware audio or deployment admission",
        "deployment_admitted": False,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = verify(args.root)
    with args.output.open("x") as stream:
        stream.write(json.dumps(result, indent=2, allow_nan=False) + "\n")
    print(
        json.dumps(
            {
                k: result[k]
                for k in ["all_parity_passed", "all_dense_service_probes_passed"]
            }
        )
    )


if __name__ == "__main__":
    main()
