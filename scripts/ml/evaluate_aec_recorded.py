"""Evaluate frozen AEC policy on three recorded English excerpts and simulated paths.

Sources previously served other plugin tests, but never AEC training. Each four
second excerpt repeats three times; this is neither 12 seconds of unique speech
nor a microphone/device recording. Conditional near-end measurements hold the
mixture-derived suppression decisions fixed and exclude adaptive-filter damage.
"""

from __future__ import annotations

import argparse
import json
import math
import subprocess
from pathlib import Path

import numpy as np
from evaluate_aec_hybrid import score, sha
from scipy.signal import resample_poly
from train_aec_residual import SCENES


def fixture(sources, pair, rate, scene, seed):
    """Build a deterministic recorded-source, simulated acoustic mixture."""
    count = 12 * rate // 256 * 256
    signals = []
    divisor = math.gcd(rate, 48000)
    for index in pair:
        excerpt = resample_poly(sources[index], rate // divisor, 48000 // divisor)
        tiled = np.tile(excerpt, 3)[:count]
        signals.append(tiled / max(float(np.sqrt(np.mean(tiled**2))), 1e-12))
    reference, clean = signals
    reference = reference * 0.05
    t = np.arange(count) / rate
    if scene == "near_after_echo":
        reference[t >= 6] = 0
    rng = np.random.default_rng(seed)
    drive = 5.0 if scene == "nonlinear" else 0.0
    playback = np.tanh(reference * drive) / drive if drive else reference
    position = np.arange(count, dtype=float)
    delay = np.full(count, rate * rng.uniform(0.004, 0.012))
    if scene == "delay_jump":
        delay[t >= 6] += rate * 0.012
    ppm = float(rng.choice([-100, 100])) if scene == "drift" else 0.0
    delay += position * ppm * 1e-6
    offsets = np.r_[0, np.sort(rng.uniform(0.001, 0.025, 7))] * rate
    taps = np.r_[0.5, rng.normal(size=7) * np.exp(-offsets[1:] / rate / 0.01) * 0.2]
    echo = np.zeros(count)
    for offset, tap in zip(offsets, taps, strict=True):
        echo += tap * np.interp(
            position - delay - offset, position, playback, left=0, right=0
        )
    active = np.zeros(count, dtype=bool)
    if scene == "near_only":
        echo.fill(0)
        reference.fill(0)
        active[:] = True
    elif scene in ["double_talk", "quiet_double_talk"]:
        active = (t >= 6) & (t < 9)
    elif scene == "near_after_echo":
        active = t >= 6
    near = clean * max(float(np.sqrt(np.mean(echo**2))), 0.015) * active
    if scene == "quiet_double_talk":
        near *= 0.25
    return {
        "input": np.column_stack([echo + near, reference]).astype("<f4"),
        "near": near.astype("<f4"),
        "echo": echo.astype("<f4"),
    }, {
        "rate": rate,
        "scene": scene,
        "seed": seed,
        "pair": list(pair),
        "frames": count,
        "delay_initial_samples": float(delay[0]),
        "drift_ppm": ppm,
        "tap_offsets_samples": offsets.tolist(),
        "tap_gains": taps.tolist(),
        "drive": drive,
    }


def read_native(metadata, name):
    path = Path(metadata["directory"]) / (name + ".f32le")
    if sha(path) != metadata[name]["sha256"]:
        raise ValueError(f"native hash mismatch: {path}")
    data = np.fromfile(path, dtype="<f4").astype(float)
    if len(data) != metadata[name]["floats"] or not np.isfinite(data).all():
        raise ValueError(f"native geometry/nonfinite: {path}")
    return data


def decompose(metadata, near):
    """Verify operator convention before applying fixed decisions to clean speech."""
    gains = read_native(metadata, "effective_gains").reshape(-1, 257)
    if np.any(gains < 0) or np.any(gains > 1.000001):
        raise ValueError("invalid effective gains")
    parts = read_native(metadata, "error_spectra").reshape(-1, 257, 2)
    spectrum = parts[:, :, 0] + 1j * parts[:, :, 1]
    reconstructed = np.fft.irfft(spectrum * gains, n=512, axis=1)[:, 256:].ravel()
    output = read_native(metadata, metadata["output_stage"])
    maximum = float(np.max(np.abs(reconstructed - output)))
    if maximum > 1e-6:
        raise ValueError(f"wet branch reconstruction failed: {maximum}")
    blocks = near.astype(float).reshape(-1, 256)
    near_spectrum = np.fft.rfft(np.pad(blocks, ((0, 0), (256, 0))), axis=1)
    conditional = np.fft.irfft(near_spectrum * gains, n=512, axis=1)[:, 256:].ravel()
    return output, conditional, maximum


def quality(near, echo, audio, conditional, rate, scene):
    count = len(near)
    windows = {"steady": (10 * rate, count), "onset": (6 * rate, 6 * rate + rate // 8)}
    if scene in ["double_talk", "quiet_double_talk"]:
        windows["double_talk"] = (25 * rate // 4, 35 * rate // 4)
    elif scene == "near_only":
        windows = {"near_only": (0, count)}
    elif scene == "near_after_echo":
        windows["near_after_echo"] = (25 * rate // 4, count)
    result = {}
    for name, (start, end) in windows.items():
        target = near[start:end].astype(float)
        energy = float(np.dot(target, target))
        metrics = {"near_snr_db" if energy > 1e-20 else "erle_db": {}}
        if energy > 1e-20:
            metrics.update(
                {
                    "conditional_near_snr_db": {},
                    "conditional_near_projection_loss_db": {},
                }
            )
        for method in ["classical", "hybrid"]:
            output = audio[method][start:end]
            if energy > 1e-20:
                component = conditional[method][start:end]
                metrics["near_snr_db"][method] = score(target, output - target)
                metrics["conditional_near_snr_db"][method] = score(
                    target, component - target
                )
                projection = float(np.dot(component, target)) / energy
                metrics["conditional_near_projection_loss_db"][method] = float(
                    -20 * np.log10(max(projection, 1e-15))
                )
            else:
                metrics["erle_db"][method] = score(
                    echo[start:end].astype(float), output
                )
        for values in metrics.values():
            values["hybrid_minus_classical_db"] = values["hybrid"] - values["classical"]
        result[name] = metrics
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--fixtures", type=Path, required=True)
    parser.add_argument("--temporary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists() or args.temporary.exists():
        raise ValueError("preserve previous outputs")
    model_hash = "2c39c23a5015cecbde5ea950381f80c18907558e897ce2a93ad4df8f72b42752"
    if sha(args.model) != model_hash:
        raise ValueError("frozen model hash mismatch")
    manifest = json.loads((args.fixtures / "manifest.json").read_text())
    sources = []
    for entry in manifest["fixtures"]:
        path = args.fixtures / entry["file"]
        if sha(path) != entry["pcm_sha256"]:
            raise ValueError("recorded source hash mismatch")
        source = np.fromfile(path, dtype="<i2").astype(float) / 32768
        if len(source) != 192000 or entry["sample_rate"] != 48000:
            raise ValueError("unexpected source geometry")
        sources.append(source)
    if len(sources) != 3:
        raise ValueError("expected three frozen sources")
    config = {
        "model_path": str(args.model.resolve()),
        "sha256": model_hash,
        "max_age_ms": 40.0,
        "max_extra_attenuation_db": 6.0,
    }
    args.output.mkdir(parents=True)
    args.temporary.mkdir(parents=True)
    protocol = {
        "cases": 72,
        "rates": [16000, 44100, 48000],
        "pairs": [[0, 1], [1, 2], [2, 0]],
        "scenes": list(SCENES),
        "seed": 20275025,
        "model_sha256": model_hash,
        "renderer_sha256": sha(args.renderer),
        "config": config,
        "processing_mode": "offline",
        "sources": manifest,
        "manifest_sha256": sha(args.fixtures / "manifest.json"),
        "source_sha256": {
            name: sha(Path(__file__).parent / name)
            for name in [
                Path(__file__).name,
                "evaluate_aec_hybrid.py",
                "train_aec_residual.py",
            ]
        },
        "policy_frozen_at": "2d59bd259681899d2fc6c64794dd830859603592",
        "selection": "Protocol written before inference; no model or policy tuning from these results.",
        "scope": "Three English four-second excerpts repeated three times through simulated paths, not measured physical-device audio. Previously used for other plugin evaluation; absent from synthetic AEC training.",
        "conditional_near_scope": "Apply mixture-derived suppression gains to known near FFT with decisions fixed; excludes adaptive-filter dependence on near-end history. Verify native wet output reconstruction <=1e-6 first.",
        "windows_seconds": {
            "steady": [10, 12],
            "onset": [6, 6.125],
            "double_talk": [6.25, 8.75],
            "near_after_echo": [6.25, 12],
        },
    }
    (args.output / "protocol.json").write_text(json.dumps(protocol, indent=2) + "\n")
    worker = subprocess.Popen(
        [str(args.renderer.resolve()), str((args.temporary / "native").resolve())],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
    )
    rows, seen = [], set()
    try:
        index = 0
        for pair in protocol["pairs"]:
            for rate in protocol["rates"]:
                for scene in SCENES:
                    arrays, provenance = fixture(
                        sources, pair, rate, scene, protocol["seed"] + index
                    )
                    path = args.temporary / f"source_{index:03d}"
                    path.mkdir()
                    for name, data in arrays.items():
                        file = path / (name + ".f32le")
                        file.write_bytes(data.tobytes())
                        provenance[name + "_sha256"] = sha(file)
                    identity = provenance["input_sha256"]
                    if identity in seen:
                        raise ValueError("duplicate source mixture")
                    seen.add(identity)
                    row = {
                        "index": index,
                        "provenance": provenance,
                        "native": {},
                        "reconstruction_max_abs": {},
                    }
                    audio, conditional = {}, {}
                    for method in (
                        ["classical", "hybrid"]
                        if index % 2 == 0
                        else ["hybrid", "classical"]
                    ):
                        request = {
                            "id": f"{method}_{index:03d}",
                            "sample_rate": rate,
                            "frames": len(arrays["near"]),
                            "input": str((path / "input.f32le").resolve()),
                            "near": str((path / "near.f32le").resolve()),
                            "offline": True,
                        }
                        if method == "hybrid":
                            request["residual_suppression"] = config
                        worker.stdin.write(json.dumps(request) + "\n")
                        worker.stdin.flush()
                        metadata = json.loads(worker.stdout.readline())
                        if (
                            metadata.get("status") != "ok"
                            or metadata["output_stage"] != method
                        ):
                            raise ValueError(f"native renderer failed: {metadata}")
                        if method == "hybrid" and (
                            metadata["residual"]["availability"] != "prepared"
                            or not metadata["residual"]["offline"]
                            or metadata["residual"]["accepted_observations"] == 0
                        ):
                            raise ValueError("learned model not exercised")
                        row["native"][method] = metadata
                        (
                            audio[method],
                            conditional[method],
                            row["reconstruction_max_abs"][method],
                        ) = decompose(metadata, arrays["near"])
                    for name in ["adaptive", "features", "targets", "error_spectra"]:
                        if (
                            row["native"]["classical"][name]["sha256"]
                            != row["native"]["hybrid"][name]["sha256"]
                        ):
                            raise ValueError(
                                "learned stage changed adaptive observations"
                            )
                    row["max_output_difference"] = float(
                        np.max(np.abs(audio["hybrid"] - audio["classical"]))
                    )
                    if scene == "near_only" and row["max_output_difference"] != 0:
                        raise ValueError("reference-silent path changed")
                    row["quality"] = quality(
                        arrays["near"], arrays["echo"], audio, conditional, rate, scene
                    )
                    (args.output / f"case-{index:03d}.json").write_text(
                        json.dumps(row, indent=2, allow_nan=False) + "\n"
                    )
                    rows.append(row)
                    print(
                        json.dumps(
                            {
                                "case": index,
                                "rate": rate,
                                "scene": scene,
                                "pair": pair,
                                "reconstruction_max_abs": row["reconstruction_max_abs"],
                            }
                        ),
                        flush=True,
                    )
                    index += 1
    finally:
        worker.stdin.close()
        worker.wait(timeout=30)
    if worker.returncode:
        raise ValueError("renderer failed")
    summary = {
        "cases": len(rows),
        "unique_mixtures": len(seen),
        "adaptive_analysis_hashes_identical": True,
        "max_reconstruction_error": max(
            max(r["reconstruction_max_abs"].values()) for r in rows
        ),
        "applied_blocks": sum(
            r["native"]["hybrid"]["residual"]["applied_blocks"] for r in rows
        ),
        "rows": [
            {
                "index": r["index"],
                "provenance": r["provenance"],
                "quality": r["quality"],
            }
            for r in rows
        ],
    }
    (args.output / "summary.json").write_text(
        json.dumps(summary, indent=2, allow_nan=False) + "\n"
    )
    print(json.dumps({k: v for k, v in summary.items() if k != "rows"}), flush=True)


if __name__ == "__main__":
    main()
