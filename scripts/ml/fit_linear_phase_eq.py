"""Offline learned EQ fitting followed by native, independently checked FIR export.

The existing learned initializer competes with classical seeds. No active plugin
is changed. A preset/WAV is exported only after final f32 FIR, target, phase,
latency and streaming-impulse checks. Rejected runs return a report alone.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import subprocess
import sys
from pathlib import Path

import numpy as np
from scipy.io import wavfile
from scipy.signal import freqz


def read_bounded(path, limit):
    with path.open("rb") as source:
        raw = source.read(limit + 1)
    if len(raw) > limit:
        raise ValueError("input exceeds offline size budget")
    return raw


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def validate_target(data):
    frequencies = np.asarray(data["frequencies_hz"], dtype=float)
    target = np.asarray(data["target_db"], dtype=float)
    rate = float(data["sample_rate"])
    if (
        frequencies.ndim != 1
        or frequencies.shape != target.shape
        or not 64 <= len(target) <= 8192
        or not np.isfinite(frequencies).all()
        or not np.isfinite(target).all()
        or not np.isfinite(rate)
        or rate != round(rate)
        or not 44100 <= rate <= 192000
        or np.any(np.diff(frequencies) <= 0)
        or frequencies[0] <= 0
        or frequencies[0] > 20
        or frequencies[-1] < 20000
        or frequencies[-1] >= rate / 2
        or np.max(np.abs(target)) > 24
    ):
        raise ValueError(
            "unsupported target geometry, rate, frequency coverage or level"
        )
    return frequencies, target, int(rate)


def convert(proposal, target_sha, manual_index):
    if (
        not isinstance(proposal, dict)
        or proposal.get("accepted") is not True
        or proposal.get("target_sha256") != target_sha
    ):
        raise ValueError("proposal is unqualified or refers to a different target")
    parameters = proposal["parameters"]
    if set(parameters) != {"filters", "auto_gain"} or parameters["auto_gain"] != {
        "enabled": False
    }:
        raise ValueError(
            "only explicit unity-level peaking/shelf proposals are supported"
        )
    filters = parameters["filters"]
    if not isinstance(filters, list) or len(filters) > 6:
        raise ValueError("at most six proposal bands are supported")
    converted = []
    kinds = {"peak": "Peak", "lowshelf": "Lowshelf", "highshelf": "Highshelf"}
    for band in filters:
        if (
            set(band) != {"filter_type", "freq", "q", "db_gain", "order"}
            or band["filter_type"] not in kinds
            or band["order"] != 2
        ):
            raise ValueError("unsupported proposal band schema")
        f, q, gain = [float(band[name]) for name in ["freq", "q", "db_gain"]]
        if (
            not all(math.isfinite(v) for v in [f, q, gain])
            or not 20 <= f <= 20000
            or not 0.35 <= q <= 8
            or not -12 <= gain <= 12
        ):
            raise ValueError("proposal band exceeds learned contract bounds")
        if band["filter_type"] != "peak" and abs(q - math.sqrt(0.5)) > 1e-9:
            raise ValueError("only S=1 shelf proposals are supported")
        converted.append(
            {
                "filter_type": kinds[band["filter_type"]],
                "frequency": f,
                "q": q,
                "gain_db": gain,
                "active": True,
            }
        )
    return {
        "num_filters": max(1, len(converted)),
        "fir_length_index": manual_index,
        "phase_mode_index": 0,
        "auto_gain": False,
        "accuracy_mode": True,
        "mix": 1.0,
        "filters": converted,
    }


def ideal_response(params, frequencies, rate):
    """Independent bilinear analog peak/shelf magnitude reference."""
    result = np.ones(len(frequencies), dtype=complex)
    for band in params["filters"]:
        s = (
            1j
            * np.tan(np.pi * frequencies / rate)
            / np.tan(np.pi * band["frequency"] / rate)
        )
        a = 10 ** (band["gain_db"] / 40)
        if band["filter_type"] == "Peak":
            value = (1 + s * s + s * a / band["q"]) / (1 + s * s + s / (a * band["q"]))
        elif band["filter_type"] == "Lowshelf":
            value = (
                a
                * (a + s * s + s * np.sqrt(2 * a))
                / (1 + a * s * s + s * np.sqrt(2 * a))
            )
        else:
            value = (
                a
                * (1 + a * s * s + s * np.sqrt(2 * a))
                / (a + s * s + s * np.sqrt(2 * a))
            )
        result *= value
    return result


def check_fir(native, params, frequencies, target, rate):
    directory = Path(native["directory"])
    raw = (directory / "fir.f32le").read_bytes()
    fir = np.frombuffer(raw, dtype="<f4").astype(float)
    impulse_raw = (directory / "impulse.f32le").read_bytes()
    impulse = np.frombuffer(impulse_raw, dtype="<f4").astype(float)
    taps = native["selected_taps"]
    if (
        taps not in [1024, 2048, 4096, 8192]
        or len(fir) != taps
        or not np.isfinite(fir).all()
        or not np.isfinite(impulse).all()
        or len(impulse) != native["impulse_frames"]
    ):
        raise ValueError("invalid native FIR/impulse geometry")
    design, partition = taps // 2, native["partition_delay_samples"]
    if (
        partition != 32
        or native["design_delay_samples"] != design
        or native["latency_samples"] != design + partition
    ):
        raise ValueError("native latency accounting mismatch")
    if native["manual_latency_samples"] != [1024, 2048, 4096, 8192][
        params["fir_length_index"]
    ] // 2 + 32 or native["additional_latency_samples"] != max(
        0, native["latency_samples"] - native["manual_latency_samples"]
    ):
        raise ValueError("additional latency accounting mismatch")
    stream_error = float(np.max(np.abs(impulse[partition : partition + taps] - fir)))
    outside = max(
        float(np.max(np.abs(impulse[:partition]))),
        float(np.max(np.abs(impulse[partition + taps :]))),
    )
    _, supplied = freqz(fir, worN=frequencies, fs=rate)
    supplied_db = 20 * np.log10(np.maximum(np.abs(supplied), 1e-30))
    residual = supplied_db - target
    centers = np.array([band["frequency"] for band in params["filters"]])
    dense = np.unique(
        np.r_[
            np.geomspace(20, 20000, 8193),
            frequencies,
            np.clip(centers[:, None] * [0.99, 1, 1.01], 20, 20000).ravel(),
        ]
    )
    _, response = freqz(fir, worN=dense, fs=rate)
    ideal = np.abs(ideal_response(params, dense, rate))
    approximation = float(
        np.max(
            np.abs(
                20
                * np.log10(
                    np.maximum(np.abs(response), 1e-30) / np.maximum(ideal, 1e-30)
                )
            )
        )
    )
    phase_error = float(
        np.max(
            np.abs(
                np.angle(
                    response * np.exp(2j * np.pi * dense * design / rate), deg=True
                )
            )
        )
    )
    peak = float(np.max(np.abs(20 * np.log10(np.maximum(np.abs(response), 1e-30)))))
    reported = np.asarray(native["response"], dtype=float)
    _, checked = freqz(fir, worN=reported[:, 0], fs=rate)
    checked *= np.exp(-2j * np.pi * reported[:, 0] * partition / rate)
    native_response_error = float(
        np.max(np.abs(checked - (reported[:, 1] + 1j * reported[:, 2])))
    )
    rms, maximum = float(np.sqrt(np.mean(residual**2))), float(np.max(np.abs(residual)))
    accepted = bool(
        rms <= 0.1
        and maximum <= 0.5
        and approximation <= 0.05001
        and phase_error <= 0.001
        and peak <= 24
        and stream_error <= 1e-6 * max(1, float(np.max(np.abs(fir))))
        and outside <= 1e-6
        and native_response_error <= 1e-7
        and native["native_accuracy_maximum_error_db"] <= 0.05
    )
    return fir.astype("<f4"), {
        "accepted": accepted,
        "target_rms_db": rms,
        "target_max_db": maximum,
        "dense_fir_vs_ideal_max_db": approximation,
        "dense_peak_absolute_db": peak,
        "linear_phase_residual_degrees": phase_error,
        "native_complex_response_max_error": native_response_error,
        "streaming_impulse_max_error": stream_error,
        "outside_support_max": outside,
        "fir_sha256": sha(raw),
        "impulse_sha256": sha(impulse_raw),
        "dense_frequency_points": len(dense),
        "scope": "Supplied target samples plus dense FIR-vs-ideal checks; not a continuous target, perceptual, calibration or speed guarantee.",
    }


def realize_candidates(
    proposal,
    target_sha,
    manual_index,
    rate,
    frequencies,
    target,
    renderer,
    work_dir,
    callback_frames,
):
    """Qualify every bounded candidate, then minimize actual support before error."""
    if (
        not isinstance(proposal, dict)
        or proposal.get("accepted") is not True
        or proposal.get("target_sha256") != target_sha
    ):
        raise ValueError("proposal is unqualified or refers to a different target")
    pool = proposal.get("qualified_proposals", [proposal])
    if not isinstance(pool, list) or not 1 <= len(pool) <= 3:
        raise ValueError("supply one to three qualified candidate proposals")
    records, eligible = [], []
    for index, item in enumerate(pool):
        record = {
            "index": index,
            "method": item.get("method") if isinstance(item, dict) else None,
            "accepted": False,
        }
        try:
            params = convert(item, target_sha, manual_index)
            request = {
                "sample_rate": rate,
                "params": params,
                "output_dir": str((work_dir / f"native-{index}").resolve()),
                "frequencies_hz": np.geomspace(20, 20000, 65).tolist(),
                "callback_frames": callback_frames,
            }
            request_path = work_dir / f"native-request-{index}.json"
            request_path.write_text(json.dumps(request))
            native_run = subprocess.run(
                [str(renderer.resolve()), str(request_path.resolve())],
                check=False,
                capture_output=True,
                text=True,
                timeout=120,
            )
            (work_dir / f"native-stdout-{index}.txt").write_text(native_run.stdout)
            (work_dir / f"native-stderr-{index}.txt").write_text(native_run.stderr)
            native = json.loads(native_run.stdout)
            record.update(
                native=native,
                request_path=str(request_path.resolve()),
                request_sha256=sha(request_path.read_bytes()),
            )
            if native_run.returncode != 0 or native.get("status") != "ok":
                raise ValueError(
                    "native FIR accuracy preparation rejected the proposal"
                )
            fir, checks = check_fir(native, params, frequencies, target, rate)
            record["checks"] = checks
            if not checks["accepted"]:
                raise ValueError("end-to-end native FIR checks did not qualify")
            record["accepted"] = True
            eligible.append(
                (
                    native["selected_taps"],
                    checks["target_rms_db"],
                    checks["target_max_db"],
                    index,
                    params,
                    fir,
                )
            )
        except (
            OSError,
            ValueError,
            KeyError,
            TypeError,
            subprocess.TimeoutExpired,
        ) as error:
            record["error"] = str(error)
        records.append(record)
    if not eligible:
        return None, records
    chosen = min(eligible, key=lambda row: row[:4])
    _, _, _, index, params, fir = chosen
    return {"params": params, "fir": fir, "record": records[index]}, records


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", type=Path, required=True)
    sources = parser.add_mutually_exclusive_group()
    sources.add_argument("--weights", type=Path)
    sources.add_argument("--proposal", type=Path)
    parser.add_argument("--weights-sha256")
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument(
        "--manual-fir-taps", type=int, choices=[1024, 2048, 4096, 8192], default=1024
    )
    parser.add_argument("--max-evaluations", type=int, default=200)
    parser.add_argument("--callback-frames", type=int, default=193)
    args = parser.parse_args()
    if (
        args.output.exists()
        or args.work_dir.exists()
        or args.output.resolve() == args.work_dir.resolve()
    ):
        raise ValueError("use separate new output and work directories")
    if (
        args.output.resolve() in args.work_dir.resolve().parents
        or args.work_dir.resolve() in args.output.resolve().parents
    ):
        raise ValueError("output and work directories must not contain one another")
    if (
        not 1 <= args.max_evaluations <= 1000
        or not 1 <= args.callback_frames <= 4096
        or bool(args.weights) != bool(args.weights_sha256)
    ):
        raise ValueError("invalid solver/callback bounds or weights identity")
    args.output.mkdir(parents=True)
    args.work_dir.mkdir(parents=True, mode=0o700)
    exported = []
    report = {
        "accepted": False,
        "parameters": None,
        "automatic_apply": False,
        "sample_rate": None,
    }
    try:
        target_raw = read_bounded(args.target, 1048576)
        frequencies, target, rate = validate_target(json.loads(target_raw))
        identity = sha(target_raw)
        report.update(target_sha256=identity, sample_rate=rate)
        if args.proposal is not None:
            proposal_raw = read_bounded(args.proposal, 1048576)
            proposal = json.loads(proposal_raw)
            report["proposal_sha256"] = sha(proposal_raw)
        elif np.max(np.abs(target)) < 0.01:
            proposal = {
                "accepted": True,
                "target_sha256": identity,
                "method": "canonical-flat",
                "parameters": {"filters": [], "auto_gain": {"enabled": False}},
            }
        else:
            if args.weights is None:
                raise ValueError(
                    "non-flat fitting needs pinned weights or a qualified proposal"
                )
            weights = read_bounded(args.weights, 8388608)
            if sha(weights) != args.weights_sha256:
                raise ValueError("weights identity mismatch")
            snapshot = args.work_dir / "model"
            snapshot.mkdir()
            (snapshot / "weights.json").write_bytes(weights)
            (snapshot / "training-report.json").write_text(
                json.dumps({"weights_sha256": args.weights_sha256})
            )
            target_path = args.work_dir / "target.json"
            target_path.write_bytes(target_raw)
            fit_path = args.work_dir / "fit.json"
            command = [
                sys.executable,
                str(Path(__file__).with_name("eq_multiband.py")),
                "fit",
                "--weights",
                str(snapshot / "weights.json"),
                "--target",
                str(target_path),
                "--output",
                str(fit_path),
                "--max-evaluations",
                str(args.max_evaluations),
                "--include-qualified-proposals",
            ]
            result = subprocess.run(
                command, check=False, capture_output=True, text=True, timeout=600
            )
            (args.work_dir / "fit-stdout.txt").write_text(result.stdout)
            (args.work_dir / "fit-stderr.txt").write_text(result.stderr)
            report.update(
                weights_sha256=args.weights_sha256, fit_exit_code=result.returncode
            )
            if not fit_path.is_file():
                raise ValueError("fitter failed before returning a proposal")
            proposal = json.loads(read_bounded(fit_path, 1048576))
            report["fit_candidates"] = proposal.get("candidates", [])
            if result.returncode != 0:
                raise ValueError("exact refined fit did not qualify")
        selected, records = realize_candidates(
            proposal,
            identity,
            [1024, 2048, 4096, 8192].index(args.manual_fir_taps),
            rate,
            frequencies,
            target,
            args.renderer,
            args.work_dir,
            args.callback_frames,
        )
        report.update(
            native_candidates=records,
            renderer_sha256=sha(args.renderer.read_bytes()),
            iir_selected_method=proposal.get("method")
            if isinstance(proposal, dict)
            else None,
            selection_policy="Qualified candidates only; shortest actual FIR, then target RMS, target maximum, stable input order.",
        )
        if selected is None:
            if len(records) == 1:
                report.update(
                    {
                        key: records[0][key]
                        for key in ["native", "checks"]
                        if key in records[0]
                    }
                )
            raise ValueError(
                "no candidate passed native FIR and final target qualification"
            )
        chosen = selected["record"]
        params, fir, native, checks = (
            selected["params"],
            selected["fir"],
            chosen["native"],
            chosen["checks"],
        )
        report.update(
            fit_method=chosen["method"],
            native=native,
            checks=checks,
            selected_candidate_index=chosen["index"],
            proposal_weights_sha256=proposal.get("weights_sha256"),
            model_sample_rate_shift=proposal.get("model_sample_rate_shift", False),
            neural_candidate_evaluated=any(
                row.get("method") == "neural" for row in proposal.get("candidates", [])
            ),
        )
        fir_path, preset_path = args.output / "fir.wav", args.output / "preset.json"
        file = fir_path.open("xb")
        exported.append(fir_path)
        with file:
            wavfile.write(file, rate, fir)
        file = preset_path.open("x")
        exported.append(preset_path)
        with file:
            file.write(json.dumps(params, indent=2) + "\n")
        report.update(
            accepted=True,
            parameters=params,
            fir_wav_sha256=sha((args.output / "fir.wav").read_bytes()),
            fir_design_delay_samples=native["design_delay_samples"],
            native_partition_delay_samples=native["partition_delay_samples"],
            latency_samples=native["latency_samples"],
            added_latency_samples=native["additional_latency_samples"],
            latency_ms=1000 * native["latency_samples"] / rate,
            added_latency_ms=1000 * native["additional_latency_samples"] / rate,
            review="Review target, selected support and latency before importing at this sample rate. A WAV convolver adds its own processing delay.",
        )
    except (
        OSError,
        ValueError,
        KeyError,
        TypeError,
        subprocess.TimeoutExpired,
    ) as error:
        for path in exported:
            path.unlink(missing_ok=True)
        report["error"] = str(error)
    (args.output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    print(json.dumps(report), flush=True)
    if not report["accepted"]:
        raise SystemExit(2)


if __name__ == "__main__":
    main()
