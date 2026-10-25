"""Retain native decoder/correction qualification before learning preferences."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from ambisonics_native import normalize_gains, preset, render
from test_ambisonics_native import custom_layout, mode_matching, reference


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    if args.report.exists():
        raise ValueError("Refuse to overwrite a report")
    args.work.mkdir(parents=True, exist_ok=False)
    binary = hashlib.sha256(args.renderer.read_bytes()).hexdigest()
    rows = []
    for rate in [44100, 48000, 96000, 192000]:
        for order, layout in [(1, "5.1"), (3, "7.1.4"), (7, "custom")]:
            for algorithm in ["mode_matching", "allrad"]:
                for dual in [False, True]:
                    key = f"{rate}-{order}-{algorithm}-{dual}"
                    p = preset(
                        order=order,
                        target_layout=layout,
                        algorithm=algorithm,
                        dual_band=dual,
                    )
                    if layout == "custom":
                        p["custom_layout"] = custom_layout(32)
                    outputs = {"5.1": 6, "7.1.4": 12, "custom": 32}[layout]
                    # Same deterministic source per rate/order across all decoder settings.
                    rng = np.random.default_rng(202610290 + rate + order)
                    x = rng.normal(0, 0.035, (4099, (order + 1) ** 2)).astype(
                        np.float32
                    )
                    x[:32] = 0
                    x[0, 0] = 0.5
                    x[10, :] = 0.1
                    baseline, m = render(
                        x,
                        args.renderer,
                        args.work / (key + "-base"),
                        p,
                        [1] * outputs,
                        rate=rate,
                        zeros=2048,
                    )
                    names = ["decoder_matrix"] + (["basic_matrix"] if dual else [])
                    matrices = [
                        np.array(m[name]["matrix"]).reshape(outputs, -1)
                        for name in names
                    ]
                    lfe = [
                        i
                        for i, s in enumerate(m["speaker_layout"]["speakers"])
                        if s["is_lfe"]
                    ]
                    gains = normalize_gains(
                        matrices, np.linspace(0.96, 1.04, outputs), lfe
                    )
                    y, cm = render(
                        x,
                        args.renderer,
                        args.work / key,
                        p,
                        gains,
                        rate=rate,
                        callback=17,
                        zeros=2048,
                    )
                    padded = np.pad(x, ((0, 2048), (0, 0)))
                    error = float(np.max(abs(y - reference(padded, cm))))
                    composition_error = float(
                        np.max(abs(y.astype(float) - baseline.astype(float) * gains))
                    )
                    matrix_error = None
                    if algorithm == "mode_matching":
                        matrix_error = max(
                            float(
                                np.max(
                                    abs(
                                        d
                                        - mode_matching(
                                            order,
                                            m["speaker_layout"]["speakers"],
                                            weighted,
                                        )
                                    )
                                )
                            )
                            for d, weighted in zip(
                                matrices, [True, False], strict=False
                            )
                        )
                    reopened, _ = render(
                        x,
                        args.renderer,
                        args.work / (key + "-reopen"),
                        json.loads(json.dumps(p)),
                        cm["speaker_gains"],
                        rate=rate,
                        callback=4096,
                        zeros=2048,
                    )
                    energy_error = max(
                        abs(band["ratio"] - 1) for band in cm["energy"]["bands"]
                    )
                    cross_error = max(
                        (
                            abs(c["normalized_error"])
                            for c in cm["energy"]["cross_terms"]
                        ),
                        default=0,
                    )
                    row = {
                        "key": key,
                        "rate": rate,
                        "order": order,
                        "algorithm": algorithm,
                        "dual_band": dual,
                        "input_sha256": cm["input_sha256"],
                        "output_sha256": cm["output_sha256"],
                        "work": str((args.work / key).resolve()),
                        "waveform_error": error,
                        "composition_error": composition_error,
                        "matrix_error": matrix_error,
                        "diffuse_energy_error": energy_error,
                        "diffuse_cross_error": cross_error,
                        "callback_reopen_exact": np.array_equal(y, reopened),
                        "lfe_silent": bool(np.all(y[:, lfe] == 0)),
                        "sample_peak": float(np.max(abs(y))),
                        "gain_energy_bounds": cm["energy"][
                            "arbitrary_output_energy_ratio_bounds"
                        ],
                    }
                    # Fixed before evaluation; f32 LR4 roundoff dominates at192kHz.
                    row["pass"] = bool(
                        error < 1e-5
                        and composition_error < 1e-7
                        and (matrix_error is None or matrix_error < 2e-6)
                        and energy_error < 2e-6
                        and cross_error < 2e-6
                        and row["callback_reopen_exact"]
                        and row["lfe_silent"]
                    )
                    rows.append(row)
    if hashlib.sha256(args.renderer.read_bytes()).hexdigest() != binary:
        raise ValueError("Native renderer changed during evaluation")
    report = {
        "renderer_sha256": binary,
        "seed": 202610290,
        "cases": rows,
        "qualification": "Procedural native functional checks, not learned preference or acoustic/listening evidence. Mode matching independently reconstructed; AllRAD composed audio checked against its native exported matrix only.",
        "all_pass": all(row["pass"] for row in rows),
    }
    args.report.write_text(json.dumps(report, indent=2, allow_nan=False) + "\n")
    if not report["all_pass"]:
        raise ValueError("Native decoder qualification failed; retain report")


if __name__ == "__main__":
    main()
