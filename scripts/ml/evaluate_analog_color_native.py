"""Retain six-model native baseline probes without claiming hardware fidelity."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from analog_color_native import controls, render
from analog_hardware_capture import read_corpus
from test_analog_hardware import corpus


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    if args.report.exists():
        raise ValueError("Refuse to overwrite retained evaluation")
    args.work.mkdir(parents=True, exist_ok=False)
    binary = hashlib.sha256(args.renderer.read_bytes()).hexdigest()
    captures = args.work / "synthetic-captures"
    captures.mkdir()
    data = corpus(captures)
    source = captures / "manifest.json"
    source.write_text(json.dumps(data, indent=2) + "\n")
    _, summary = read_corpus(data)
    rows = []
    for rate in [44100, 48000, 96000, 192000]:
        rng = np.random.default_rng(20261035 + rate)
        t = np.arange(8193) / rate
        x = rng.normal(0, 0.01, (8193, 2)) + 0.02 * np.sin(
            2 * np.pi * 997 * t[:, None] + np.arange(2)
        )
        x[:16] = 0
        x[0, 0] = 0.1
        x = x.astype(np.float32)
        padded = np.pad(x, ((0, 4096), (0, 0)))
        for model in range(6):
            outputs = {}
            settings = {
                "wet": controls(model_id=model, drive_db=6, character=0.31),
                "trim": controls(
                    model_id=model, drive_db=6, character=0.31, output_trim_db=-6
                ),
                "blend": controls(
                    model_id=model,
                    drive_db=6,
                    character=0.31,
                    output_trim_db=-6,
                    color=0.37,
                ),
                "dry": controls(
                    model_id=model, drive_db=36, color=0, character=1, output_trim_db=24
                ),
            }
            for name, parameters in settings.items():
                key = f"{rate}-{model}-{name}"
                path = args.work / key
                y, m = render(x, args.renderer, path, parameters, rate=rate, zeros=4096)
                other, _ = render(
                    x,
                    args.renderer,
                    args.work / (key + "-reopen"),
                    json.loads(json.dumps(m["effective_controls"])),
                    rate=rate,
                    callback=17,
                    maximum=1,
                    zeros=4096,
                )
                outputs[name] = y
                expected = {
                    "wet": y,
                    "trim": outputs["wet"] * 10 ** (-6 / 20),
                    "blend": padded + 0.37 * (outputs.get("trim", y) - padded),
                    "dry": padded,
                }[name]
                error = float(np.max(abs(y.astype(float) - expected)))
                row = {
                    "rate": rate,
                    "model": model,
                    "setting": name,
                    "work": str(path.resolve()),
                    "input_sha256": m["input_sha256"],
                    "output_sha256": m["output_sha256"],
                    "independent_control_law_error": error if name != "wet" else None,
                    "callback_reopen_exact": bool(np.array_equal(y, other)),
                    "sample_peak": m["sample_peak"],
                    "qa_process_seconds": m["qa_process_seconds"],
                    "latency_samples": m["latency_samples"],
                }
                row["pass"] = bool(
                    error < 2e-7
                    and row["callback_reopen_exact"]
                    and row["latency_samples"] == 0
                )
                rows.append(row)
    if hashlib.sha256(args.renderer.read_bytes()).hexdigest() != binary:
        raise ValueError("Native binary changed during evaluation")
    report = {
        "renderer_sha256": binary,
        "seed": 20261035,
        "cases": rows,
        "capture_summary": summary,
        "all_pass": all(r["pass"] for r in rows),
        "scope": "Generated native functional probes and three synthetic paired captures; no training, measured hardware, learned advantage or production cost claim.",
    }
    args.report.write_text(json.dumps(report, indent=2, allow_nan=False) + "\n")
    if not report["all_pass"]:
        raise ValueError("Native baseline qualification failed; retain report")


if __name__ == "__main__":
    main()
