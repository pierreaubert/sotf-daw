"""Verify matched native pitch probes, correction response and waveform trajectories."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from pnd_compare_cases import KINDS, fixture


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def trajectory(audio, rate, center):
    """Independent positive-frequency phase probe with a 10ms phase average."""
    n = 1 << (2 * len(audio) - 1).bit_length()
    data = np.fft.fft(np.asarray(audio, float), n=n)
    frequency = np.fft.fftfreq(n, 1 / rate)
    distance = abs(frequency / center - 1)
    weight = np.where(
        distance <= 0.1,
        1.0,
        np.where(
            distance < 0.25, 0.5 + 0.5 * np.cos(np.pi * (distance - 0.1) / 0.15), 0
        ),
    )
    data *= 2 * weight
    analytic = np.fft.ifft(data)[: len(audio)]
    phase = np.unwrap(np.angle(analytic))
    hop = round(rate * 0.01)
    hz = (phase[hop:] - phase[:-hop]) * rate / (2 * np.pi * hop)
    frames = np.arange(len(hz)) + hop / 2
    cents = 1200 * np.log2(np.maximum(hz, 1e-30) / center)
    return frames, cents


def raw_metrics(rows, cents, rate, reference, supported, confidence):
    center = np.array(
        [r["source_end_frame"] - (r["window_frames"] + 1) / 2 for r in rows]
    )
    hz = np.array([r["frequency_hz"] for r in rows])
    conf = np.array([r["confidence"] for r in rows])
    settled = (center >= 0.5 * rate) & (center < len(cents) - 0.5 * rate)
    accepted = settled & (conf >= confidence) & (hz > 0)
    estimated = 1200 * np.log2(np.maximum(hz, 1e-30) / reference)
    truth = np.interp(center, np.arange(len(cents)), cents)
    errors = estimated[accepted] - truth[accepted]
    octaves = np.rint(errors / 1200)
    reference_octaves = np.rint(estimated / 1200)
    return {
        "settled_observations": int(settled.sum()),
        "confidence_accepted": int(accepted.sum()),
        "confidence_accepted_fraction": float(accepted.sum() / max(settled.sum(), 1)),
        "accepted_error_rms_cents": float(np.sqrt(np.mean(errors**2)))
        if supported and len(errors)
        else None,
        "accepted_error_p95_cents": float(np.quantile(abs(errors), 0.95))
        if supported and len(errors)
        else None,
        "accepted_octave_errors": int(
            np.count_nonzero(
                (abs(octaves) >= 1) & (abs(errors - 1200 * octaves) <= 100)
            )
        )
        if supported
        else None,
        "accepted_gross_errors_over_600_cents": int(
            np.count_nonzero(abs(errors) >= 600)
        )
        if supported
        else None,
        "confidence_accepted_reference_octaves": int(
            np.count_nonzero(
                accepted
                & (abs(reference_octaves) >= 1)
                & (abs(estimated - 1200 * reference_octaves) <= 100)
            )
        ),
        "reference_policy_accepted": int(
            np.count_nonzero(accepted & (abs(estimated) <= 100))
        ),
    }


def analyze(root, report=None):
    if report is None:
        report = json.loads((root / "report.json").read_text())
    if report["smoke"]:
        raise ValueError("Smoke output cannot establish the full comparison matrix")
    if (
        digest(root / "renderer.frozen") != report["renderer_sha256"]
        or digest(root / "crepe-tiny.frozen.onnx") != report["model_sha256"]
    ):
        raise ValueError("Frozen renderer/model identity mismatch")
    expected = {(s, k) for s in [20308120, 20308121] for k in KINDS}
    actual = [(r["identity"]["seed"], r["identity"]["kind"]) for r in report["cases"]]
    if len(actual) != len(expected) or set(actual) != expected:
        raise ValueError("Incomplete or duplicated pitch comparison matrix")
    cases, hashes = [], 0
    for row in report["cases"]:
        x, truth, intended, identity = fixture(
            row["identity"]["seed"], row["identity"]["kind"]
        )
        if identity != row["identity"]:
            raise ValueError("Regenerated source identity mismatch")
        for entry in row["files"].values():
            if digest(entry["path"]) != entry["sha256"]:
                raise ValueError("Raw pitch evidence hash mismatch")
            hashes += 1
        if identity["input_sha256"] != row["files"]["input.f32le"]["sha256"]:
            raise ValueError("Input identity mismatch")
        meta = json.loads(Path(row["files"]["native.json"]["path"]).read_text())
        rate, reference = identity["sample_rate"], identity["reference_hz"]
        if (
            meta["latency_samples"] != 2047
            or meta["sample_rate"] != rate
            or meta["source_frames"] != len(x)
            or meta["callback_frames"] != 128
        ):
            raise ValueError("Native geometry mismatch")
        if (
            meta["neural_config"]["sha256"] != report["model_sha256"]
            or meta["neural_first"] != row["neural_first"]
        ):
            raise ValueError("Native policy/order binding mismatch")
        if (
            meta["params"]["drift_smoothing"] != float(np.float32(0.1))
            or meta["params"]["correction_strength"] != 1.0
            or meta["params"]["confidence_threshold"] != 0.5
            or meta["params"]["multi_channel_analysis"]
            or meta["neural_config"]["min_confidence"] != float(np.float32(0.8))
            or meta["neural_config"]["max_deviation_cents"] != 100.0
            or meta["neural_config"]["max_age_ms"] != 150.0
        ):
            raise ValueError("Native comparison profile changed")
        trace = meta["trace"]
        previous = 0
        response_misses = 0
        execution_overruns = 0
        generations = {"classical": 0, "neural": 0}
        decisions = {"classical": [], "neural": []}
        for t in trace:
            end = t["source_end_frame"]
            count = end - previous
            release = previous * 1_000_000_000 // rate
            if (
                not 0 < count <= 128
                or t["release_ns"] != release
                or t["pair_response_ns"]
                < t["start_lateness_ns"]
                + t["classical_execution_ns"]
                + t["neural_execution_ns"]
            ):
                raise ValueError("Callback source clock/timing arithmetic mismatch")
            response_misses += t["pair_response_ns"] > count * 1_000_000_000 // rate
            execution_overruns += (
                t["classical_execution_ns"] + t["neural_execution_ns"]
                > count * 1_000_000_000 // rate
            )
            for method, previous_generation in generations.items():
                state = t[method]
                if not np.isfinite(
                    [state["drift_ratio"], state["correction_ratio"]]
                ).all():
                    raise ValueError("Non-finite native control decision")
                if (
                    state["analysis_generation"] < previous_generation
                    or state["analysis_generation"] > previous_generation + 1
                ):
                    raise ValueError("Missing or regressed control decision")
                if state["analysis_generation"] > previous_generation:
                    decisions[method].append((end, state))
                generations[method] = state["analysis_generation"]
            previous = end
        if previous != len(x):
            raise ValueError("Native trace does not cover the source")
        center = reference * identity["measurement_partial"]
        frames, observed = trajectory(x, rate, center)
        valid = (frames >= rate * 0.5) & (frames < len(x) - rate * 0.5)
        if identity["kind"] == "step":
            valid &= abs(frames - rate) >= rate * 0.15
        expected_cents = np.interp(frames, np.arange(len(truth)), truth)
        calibration = float(
            np.sqrt(np.mean((observed[valid] - expected_cents[valid]) ** 2))
        )
        methods = {}
        for method in ["classical", "neural"]:
            audio = np.fromfile(row["files"][method + ".f32le"]["path"], dtype="<f4")
            if len(audio) != meta["output_frames"] or not np.isfinite(audio).all():
                raise ValueError("Native audio shape/finite domain mismatch")
            aligned = audio[2047 : 2047 + len(x)]
            f, out = trajectory(aligned, rate, center)
            if not np.array_equal(f, frames):
                raise ValueError("Aligned output trajectory shape mismatch")
            desired = np.interp(f, np.arange(len(intended)), intended)
            error = out[valid] - desired[valid]
            ds = [(end, s) for end, s in decisions[method] if end >= rate * 0.5]
            selected = sum(s["neural_pitch_used"] for _, s in ds)
            t50 = None
            if identity["kind"] == "step":
                reached = [
                    end
                    for end, s in ds
                    if end >= rate and -1200 * np.log2(s["correction_ratio"]) >= 12.5
                ]
                t50 = 1000 * (min(reached) - rate) / rate if reached else None
            vib_amplitude = None
            if identity["kind"] == "vibrato":
                time = f[valid] / rate
                design = np.column_stack(
                    [
                        np.sin(2 * np.pi * 5 * time),
                        np.cos(2 * np.pi * 5 * time),
                        np.ones(len(time)),
                    ]
                )
                coef = np.linalg.lstsq(design, out[valid], rcond=None)[0]
                vib_amplitude = float(np.hypot(*coef[:2]))
            methods[method] = {
                "settled_control_decisions": len(ds),
                "selected_neural_decisions": selected,
                "first_neural_decision_ms": min(
                    [
                        1000 * end / rate
                        for end, s in decisions[method]
                        if s["neural_pitch_used"]
                    ],
                    default=None,
                ),
                "control_step_t50_ms": t50,
                "output_vs_intended_rms_cents": float(np.sqrt(np.mean(error**2))),
                "output_vs_intended_p95_cents": float(np.quantile(abs(error), 0.95)),
                "vibrato_amplitude_cents": vib_amplitude,
                "execution_p99_ms": float(
                    np.quantile([t[method + "_execution_ns"] for t in trace], 0.99)
                    / 1e6
                ),
            }
        cases.append(
            {
                "identity": identity,
                "raw_classical": raw_metrics(
                    meta["raw_classical"],
                    truth,
                    rate,
                    reference,
                    identity["supported_fundamental_truth"],
                    0.5,
                ),
                "raw_crepe": raw_metrics(
                    meta["raw_crepe"],
                    truth,
                    rate,
                    reference,
                    identity["supported_fundamental_truth"],
                    0.8,
                ),
                "input_probe_rms_error_cents": calibration,
                "audio_measurement_qualified": calibration <= 2.0,
                "methods": methods,
                "paired_callbacks": len(trace),
                "pair_response_misses": response_misses,
                "combined_execution_overruns": execution_overruns,
            }
        )
    return {
        "cases": cases,
        "verified_raw_hashes": hashes,
        "scope": "generated paired source trajectories; two base frequencies and phase/noise identities; raw estimates scored at window centers; controls and audio remain separate",
        "probe_tolerance_rms_cents": 2.0,
        "audio_scoring_mask": "exclude first/last0.5s and step transition+/-0.15s; latency alignment2047 frames; 10ms phase average",
        "worker_observation_age_measured": False,
        "report_sha256": digest(root / "report.json"),
        "deployment_admitted": False,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    result = analyze(args.root)
    with args.output.open("x") as stream:
        stream.write(json.dumps(result, indent=2, allow_nan=False) + "\n")
    print(
        json.dumps(
            {
                "cases": len(result["cases"]),
                "verified_raw_hashes": result["verified_raw_hashes"],
                "unqualified_audio_probes": sum(
                    not x["audio_measurement_qualified"] for x in result["cases"]
                ),
            }
        )
    )


if __name__ == "__main__":
    main()
