"""Independently score native outputs and a noise-span-calibrated classical blend."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from train_music_noise_mask import scene


def score(actual, target):
    a, t = np.asarray(actual, float), np.asarray(target, float)
    reference = max(float(np.linalg.norm(t.ravel())), 1e-10)
    error = max(float(np.linalg.norm((a - t).ravel())), 1e-15)
    return {
        "snr_db": float(20 * np.log10(reference / error)),
        "relative_rms": error / reference,
        "max_abs_error": float(np.max(abs(a - t))),
        "projection_gain": float(np.vdot(a, t).real / reference**2),
    }


def analyze(model, training, waveform, work):
    identity = hashlib.sha256(model.read_bytes()).hexdigest()
    assert identity == training["model_sha256"] == waveform["model_sha256"]
    data = json.loads(model.read_text())
    w = np.asarray(data["weights"]).T
    b = np.asarray(data["bias"])
    v = np.asarray(data["output_weights"])
    groups = {r["seed"] for r in training["training_groups"]}
    development = {r["seed"] for r in training["validation_groups"]}
    assert not groups & development
    mask_rows = []
    for kind, values in training["held"].items():
        error_sum = 0.0
        count = 0
        group_errors = []
        for metadata in values["groups"]:
            assert metadata["seed"] not in groups | development
            x, y, row = scene(metadata["seed"], kind)
            assert row == metadata
            z = v @ np.tanh(w @ (2 * x.T - 1) + b[:, None]) + data["output_bias"]
            probability = 1 / (1 + np.exp(-z))
            squared = float(np.dot(probability - y, probability - y))
            error_sum += squared
            count += len(y)
            group_errors.append(squared / len(y))
        error = error_sum / count
        assert abs(error - values["metrics"]["gain_mse"]) < 1e-12
        mask_rows.append(
            {
                "kind": kind,
                "pooled_gain_mse": error,
                "equal_source_gain_mse": float(np.mean(group_errors)),
                "sources": len(group_errors),
            }
        )
    work.mkdir(parents=True, exist_ok=False)
    rows = []
    maximum_error = 0.0
    for row in waveform["cases"]:
        channels = row["channels"]
        rate = row["sample_rate"]
        raw = Path(row["target_file"]).read_bytes()
        assert hashlib.sha256(raw).hexdigest() == row["target_sha256"]
        target = np.frombuffer(raw, dtype="<f4").astype(float).reshape(-1, channels)
        rendered = {}
        for method, result in row["methods"].items():
            raw = Path(result["output_file"]).read_bytes()
            metadata = result["metadata"]
            assert hashlib.sha256(raw).hexdigest() == metadata["output_sha256"]
            y = np.frombuffer(raw, dtype="<f4").astype(float).reshape(-1, channels)
            fft = metadata["latency_samples"]
            hop = fft // 2
            expected_tail = 2 * fft - hop + (hop - len(target) % hop) % hop
            assert (
                metadata["drain_frames"] == expected_tail
                and len(y) == len(target) + expected_tail
            )
            assert metadata["counts"]["frames"] == len(target) // hop
            assert np.all(y[:fft] == 0)
            for name, start in [("full", 0), ("steady_after_half_second", rate // 2)]:
                recomputed = score(y[fft + start : fft + len(target)], target[start:])
                for key, value in recomputed.items():
                    maximum_error = max(maximum_error, abs(value - result[name][key]))
            counts = metadata["counts"]
            assert (
                sum(
                    counts[k]
                    for k in [
                        "neural_bins",
                        "disabled_bins",
                        "unavailable_bins",
                        "invalid_bins",
                        "boundary_bins",
                    ]
                )
                == counts["frames"] * (fft // 2 + 1) * channels
            )
            rendered[method] = y
        result = row["methods"]["classical"]
        path = Path(result["output_file"]).with_name("input.f32le")
        raw = path.read_bytes()
        assert hashlib.sha256(raw).hexdigest() == result["metadata"]["input_sha256"]
        source = np.frombuffer(raw, dtype="<f4").astype(float).reshape(-1, channels)
        fft = result["metadata"]["latency_samples"]
        entry = {
            "case": row["case"],
            "kind": row["kind"],
            "input_reference": score(source[rate // 2 :], target[rate // 2 :]),
            "neural": row["methods"]["neural"]["steady_after_half_second"],
        }
        if row["kind"] != "clean_only":
            # Select one output-blend coefficient from a declared noise-only
            # calibration span, without inspecting subsequent musical target data.
            first, last = 3 * rate // 20, 19 * rate // 100
            classical = rendered["classical"][fft : fft + len(target)]
            neural = rendered["neural"][fft : fft + len(target)]
            old = classical[first:last].ravel()
            difference = (source[first:last] - classical[first:last]).ravel()
            desired = float(np.vdot(neural[first:last], neural[first:last]).real)
            a = float(np.dot(difference, difference))
            b = 2 * float(np.dot(old, difference))
            c = float(np.dot(old, old)) - desired
            roots = np.roots([a, b, c]) if a > 1e-30 else []
            candidates = [
                float(x.real) for x in roots if abs(x.imag) < 1e-10 and 0 <= x.real <= 1
            ]
            coefficient = (
                min(candidates)
                if candidates
                else min([0.0, 1.0], key=lambda t: abs(a * t * t + b * t + c))
            )
            dry = np.zeros_like(rendered["classical"])
            dry[fft : fft + len(source)] = source
            # A ten-ms ramp begins only after calibration ends, before music starts.
            ramp = (
                np.clip((np.arange(len(dry)) - fft - last) / (rate * 0.01), 0, 1)
                * coefficient
            )
            control = (
                rendered["classical"] + ramp[:, None] * (dry - rendered["classical"])
            ).astype("<f4")
            output = work / (row["case"] + "-matched-control.f32le")
            output.write_bytes(control.tobytes())
            aligned = control[fft : fft + len(target)].astype(float)
            validation = slice(rate // 5, 6 * rate // 25)
            observed = max(float(np.sum(aligned[validation] ** 2)), 1e-30)
            reference = max(float(np.sum(neural[validation] ** 2)), 1e-30)
            control_scores = score(aligned[rate // 2 :], target[rate // 2 :])
            entry.update(
                classical_dry_blend=coefficient,
                calibration_source_frames=[first, last],
                matched_control_output=str(output),
                matched_control_sha256=hashlib.sha256(output.read_bytes()).hexdigest(),
                calibration_energy_attainable=bool(candidates),
                later_noise_energy_mismatch_db=float(
                    10 * np.log10(observed / reference)
                ),
                matched_control=control_scores,
                steady_neural_minus_matched_control_snr_db=entry["neural"]["snr_db"]
                - control_scores["snr_db"],
            )
        rows.append(entry)
    assert maximum_error < 1e-9
    return {
        "model_sha256": identity,
        "mask_recomputation": mask_rows,
        "native_waveform_metric_max_error": maximum_error,
        "cases": rows,
        "deployment_admitted": False,
        "scope": "Independent generated-mask/native-PCM arithmetic, hashes, complete drain and source-counter accounting. Additional diagnostic control blends native classical output with latency-aligned input. Its coefficient uses only the declared noise-only0.15..0.19s span to match neural output energy; a10ms ramp then applies it before music, and0.20..0.24s checks noise transfer. It is an oracle noise-span reference, not a deployed parameter or independent model-selection gate. Clean-only cases have no usable noise calibration and retain an unfiltered-input reference.",
    }


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ["model", "training", "waveform", "work", "report"]:
        p.add_argument("--" + name, type=Path, required=True)
    args = p.parse_args()
    result = analyze(
        args.model,
        json.loads(args.training.read_text()),
        json.loads(args.waveform.read_text()),
        args.work,
    )
    with args.report.open("x") as out:
        out.write(json.dumps(result, indent=2, allow_nan=False) + "\n")
    print(
        json.dumps(
            {
                "native_metric_error": result["native_waveform_metric_max_error"],
                "matched_noise_control_deltas_db": [
                    r["steady_neural_minus_matched_control_snr_db"]
                    for r in result["cases"]
                    if "matched_control" in r
                ],
            }
        )
    )


if __name__ == "__main__":
    main()
