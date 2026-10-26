"""Frozen native WOLA qualification and measured-plant rendered cancellation."""

import argparse
import hashlib
import json
import shutil
from pathlib import Path

import h5py
import numpy as np
from scipy.signal import fftconvolve
from xtc_proposal_native import coefficients, projected_transfer, render, wola_reference
from xtc_transfer_model import log_ratio_db


def complex_pairs(x):
    return np.stack([np.asarray(x).real, np.asarray(x).imag], axis=-1).tolist()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", required=True, type=Path)
    parser.add_argument("--proposal", required=True, type=Path)
    parser.add_argument("--sofa", required=True, type=Path)
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    manifest = json.loads(args.manifest.read_text())
    if hashlib.sha256(args.sofa.read_bytes()).hexdigest() != manifest["sha256"]:
        raise ValueError("Measured plant identity mismatch")
    with h5py.File(args.sofa) as f:
        positions, ir = f["SourcePosition"][()], f["Data.IR"][()]
    horizontal = {
        round(p[0], 6): i
        for i, p in enumerate(positions)
        if abs(p[1]) < 1e-8 and abs(p[2] - 1.4) < 1e-8
    }
    proposal = json.loads(args.proposal.read_text())
    sha = hashlib.sha256(args.proposal.read_bytes()).hexdigest()
    args.output.mkdir(parents=True, exist_ok=False)
    frozen = args.output / "renderer.frozen"
    shutil.copy2(args.renderer, frozen)
    methods = ["learned", "native"]
    bins = [5, 12, 23, 46, 93, 186, 372]
    yaws = [0, 5, -5, 10, -10, 20, -20, 90]
    frames, amplitude = 44117, 0.001
    stored = {}
    waveforms, tones, ear_projections = [], [], []

    def native_render(x, name, tag):
        y, metadata = render(
            x,
            frozen,
            args.output / tag / name,
            proposal=args.proposal if name == "learned" else None,
            sha=sha,
            model_sha=proposal["model_sha256"],
        )
        if (metadata["selected_proposal_sha256"] == sha) != (name == "learned"):
            raise RuntimeError("Unexpected native filter selection")
        reference = wola_reference(x, metadata)
        error = float(np.max(abs(y.astype(float) - reference)))
        if error > 2e-7:
            raise RuntimeError("Native WOLA exceeds the fixed2e-7 sample-error gate")
        waveforms.append(
            {
                "tag": tag,
                "method": name,
                "frames": len(x),
                "maximum_reference_sample_error": error,
                "output_peak": float(np.max(abs(y))),
                "files": metadata["files"],
            }
        )
        matrix = coefficients(metadata)
        if name in stored and not np.array_equal(matrix, stored[name]):
            raise RuntimeError("Prepared coefficients changed between native renders")
        stored[name] = matrix
        return y, metadata

    for b in bins:
        for cue in range(2):
            x = np.zeros((frames, 2), dtype="<f4")
            x[:, cue] = amplitude * np.cos(2 * np.pi * b * np.arange(frames) / 2048)
            for name in methods:
                speakers, _metadata = native_render(x, name, f"tone-{b}-{cue}")
                gain, residual = projected_transfer(speakers, b, amplitude, frames)
                effective = (
                    stored[name][b - 1] + 4 * stored[name][b] + stored[name][b + 1]
                ) / 6
                tones.append(
                    {
                        "bin": b,
                        "cue": cue,
                        "method": name,
                        "measured_speaker_transfer_real_imag": complex_pairs(gain),
                        "raw_bin_transfer_real_imag": complex_pairs(
                            stored[name][b, :, cue]
                        ),
                        "hann_effective_transfer_real_imag": complex_pairs(
                            effective[:, cue]
                        ),
                        "hann_prediction_max_error": float(
                            np.max(abs(gain - effective[:, cue]))
                        ),
                        "raw_bin_prediction_max_error": float(
                            np.max(abs(gain - stored[name][b, :, cue]))
                        ),
                        "speaker_stationary_residual_rms": residual,
                    }
                )
                for yaw in yaws:
                    source_indices = [
                        horizontal[float((30 + yaw) % 360)],
                        horizontal[float((-30 + yaw) % 360)],
                    ]
                    ears = np.column_stack(
                        [
                            sum(
                                fftconvolve(
                                    speakers[:, s].astype(float),
                                    ir[source_indices[s], ear],
                                )
                                for s in range(2)
                            )
                            for ear in range(2)
                        ]
                    )
                    response, ear_residual = projected_transfer(
                        ears, b, amplitude, frames
                    )
                    wave = np.exp(-2j * np.pi * b * np.arange(512) / 2048)
                    plant = np.array(
                        [
                            [np.dot(ir[index, ear], wave) for index in source_indices]
                            for ear in range(2)
                        ]
                    )
                    matrix_response = plant @ gain
                    if np.max(abs(response - matrix_response)) > 2e-5:
                        raise RuntimeError(
                            "Rendered measured ears disagree with independently measured speaker drives"
                        )
                    ideal_response = plant @ stored[name][b, :, cue]
                    ear_projections.append(
                        {
                            "method": name,
                            "bin": b,
                            "frequency_hz": b * 44100 / 2048,
                            "cue": cue,
                            "yaw_deg": yaw,
                            "measured_source_indices": source_indices,
                            "rendered_response_real_imag": complex_pairs(response),
                            "ideal_raw_bin_response_real_imag": complex_pairs(
                                ideal_response
                            ),
                            "ear_waveform_sha256": hashlib.sha256(
                                ears.astype("<f8").tobytes()
                            ).hexdigest(),
                            "measured_matrix_response_max_error": float(
                                np.max(abs(response - matrix_response))
                            ),
                            "ear_stationary_residual_rms": ear_residual,
                            "intended_gain_db": log_ratio_db(abs(response[cue]), 1.0),
                            "normalized_separation_db": log_ratio_db(
                                abs(response[cue]), abs(response[1 - cue])
                            ),
                            "raw_crosstalk_attenuation_db": log_ratio_db(
                                abs(plant[1 - cue, cue]), abs(response[1 - cue])
                            ),
                        }
                    )
    probes = {
        "broadband": np.random.default_rng(20299120)
        .normal(0, 0.003, (5101, 2))
        .astype("<f4")
    }
    for cue in range(2):
        for onset in [0, 77]:
            x = np.zeros((4101, 2), dtype="<f4")
            x[onset, cue] = 0.001
            probes[f"impulse-{cue}-{onset}"] = x
    for tag, x in probes.items():
        for name in methods:
            native_render(x, name, tag)
    report = {
        "model_sha256": proposal["model_sha256"],
        "proposal_sha256": sha,
        "renderer_sha256": hashlib.sha256(frozen.read_bytes()).hexdigest(),
        "dataset_sha256": manifest["sha256"],
        "sample_rate": 44100,
        "waveforms": waveforms,
        "tones": tones,
        "measured_ear_projections": ear_projections,
        "scope": "fixed centered filter; measured mannequin yaw mismatches replay existing directions; no model tuning or independent-person holdout",
        "deployment_admitted": False,
    }
    (args.output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    print(
        json.dumps(
            {
                "native_waveforms": len(waveforms),
                "measured_ear_projections": len(ear_projections),
                "maximum_wola_sample_error": max(
                    r["maximum_reference_sample_error"] for r in waveforms
                ),
                "maximum_hann_transfer_error": max(
                    r["hann_prediction_max_error"] for r in tones
                ),
                "deployment_admitted": False,
            }
        )
    )


if __name__ == "__main__":
    main()
