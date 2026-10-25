#!/usr/bin/env python3
"""Independently audit raw Dynamic EQ admission, including quiet channels."""

import argparse
import hashlib
import json
import math
from pathlib import Path

import numpy as np


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def power(values):
    return math.fsum(float(v) * float(v) for v in values.flat) / values.size


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evaluation", type=Path, action="append", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    rows = []
    clips_checked = renders = 0
    metric_error = reopen_error = 0.0
    legacy_changes = []
    for evaluation in args.evaluation:
        for entry in json.loads(evaluation.read_text())["cases"]:
            data = json.loads(Path(entry["target"]).read_text())
            report = json.loads(Path(entry["report"]).read_text())
            folder = Path(entry["report"]).parent
            reasons = set()
            channel_reasons = set()
            channel_rows = []
            has_channel_gate = all(
                "channel_metrics" in c for c in report["qualification"]
            )
            for i, (c, stored) in enumerate(
                zip(data["clips"], report["qualification"], strict=True)
            ):
                raw = Path(c["input_file"]).read_bytes()
                ref = Path(c["reference_file"]).read_bytes()
                assert (
                    sha(raw) == c["input_sha256"] and sha(ref) == c["reference_sha256"]
                )
                x = np.frombuffer(raw, dtype="<f4").reshape(-1, data["channels"])
                target = np.frombuffer(ref, dtype="<f4").reshape(x.shape)
                path = folder / "qualification" / str(i)
                metadata = json.loads((path / "metadata.json").read_text())
                out = (path / "output.f32le").read_bytes()
                assert (
                    sha(out) == metadata["output_sha256"]
                    and sha((path / "input.f32le").read_bytes())
                    == metadata["input_sha256"]
                    == c["input_sha256"]
                )
                y = np.frombuffer(out, dtype="<f4").reshape(x.shape)
                error = y.astype(float) - target
                rms = math.sqrt(power(error) / power(x))
                peak = float(np.max(abs(error)))
                baseline = math.sqrt(power(x.astype(float) - target) / power(x))
                metric_error = max(
                    metric_error,
                    abs(rms - stored["metrics"]["relative_rms"]),
                    abs(peak - stored["metrics"]["max_abs"]),
                )
                if rms > 0.002 or peak > 0.01:
                    reasons.add(c["role"] + "_waveform_mismatch")
                if baseline - rms < 0.001 or rms > 0.5 * baseline:
                    reasons.add("no_material_advantage_over_dry")
                individual = []
                for channel in range(data["channels"]):
                    scale = math.sqrt(
                        max(power(x[:, channel]), power(target[:, channel]), 1e-12)
                    )
                    relative = math.sqrt(power(error[:, channel])) / scale
                    maximum = float(np.max(abs(error[:, channel])))
                    individual.append(
                        {
                            "channel": channel,
                            "relative_rms": relative,
                            "max_abs": maximum,
                            "scale_rms": scale,
                        }
                    )
                    if relative > 0.002 or maximum > 0.01:
                        channel_reasons.add(c["role"] + "_channel_waveform_mismatch")
                    if has_channel_gate:
                        expected = stored["channel_metrics"][channel]
                        metric_error = max(
                            metric_error,
                            abs(relative - expected["relative_rms"]),
                            abs(maximum - expected["max_abs"]),
                            abs(scale - expected["scale_rms"]),
                        )
                reopen = folder / "reopen" / str(i)
                reopen_meta = json.loads((reopen / "metadata.json").read_text())
                reopen_raw = (reopen / "output.f32le").read_bytes()
                assert sha(reopen_raw) == reopen_meta["output_sha256"]
                assert (
                    sha((reopen / "input.f32le").read_bytes())
                    == reopen_meta["input_sha256"]
                    == c["input_sha256"]
                )
                assert reopen_meta["requested_params"] == metadata["requested_params"]
                if entry["case"] == "silent-channel-corruption":
                    assert (
                        np.count_nonzero(x[:, -1]) == 0
                        and np.count_nonzero(y[:, -1]) == 0
                    )
                    assert power(target[:, -1]) > 1e-12
                other = np.frombuffer(reopen_raw, dtype="<f4").reshape(x.shape)
                delta = float(np.max(abs(other.astype(float) - y)))
                reopen_error = max(reopen_error, delta)
                if delta > 1e-7:
                    reasons.add("preset_reopening_mismatch")
                channel_rows.append(
                    {
                        "clip_id": c["id"],
                        "role": c["role"],
                        "aggregate_relative_rms": rms,
                        "channels": individual,
                    }
                )
                clips_checked += 1
                renders += 2
            best = report["search"]["best"]
            bx = np.asarray(best["x"])
            loss = math.sqrt(best["loss"])
            for point in report["search"]["trace"] + report["sensitivity_probes"]:
                if (
                    np.max(abs(np.asarray(point["x"]) - bx)) >= 0.099
                    and math.sqrt(point["loss"]) <= loss + 0.0005
                ):
                    reasons.add("observed_parameter_ambiguity")
            expected = reasons | channel_reasons if has_channel_gate else reasons
            assert sorted(expected) == report["reasons"]
            assert report["accepted"] == (not expected)
            assert (folder / "preset.json").exists() == report["accepted"]
            corrected = not (reasons | channel_reasons)
            if report["accepted"] != corrected:
                legacy_changes.append(
                    {
                        "evaluation": str(evaluation),
                        "case": entry["case"],
                        "method": entry["method"],
                    }
                )
            rows.append(
                {
                    "evaluation": str(evaluation),
                    "case": entry["case"],
                    "method": entry["method"],
                    "legacy_report": not has_channel_gate,
                    "accepted": report["accepted"],
                    "channel_qualified_acceptance": corrected,
                    "channel_reasons": sorted(channel_reasons),
                    "clips": channel_rows,
                }
            )
    # Compare the corrected negative search with the initial accepted search:
    # only admission changed, not proposals, losses, sensitivity or controls.
    negatives = [r for r in rows if r["case"] == "silent-channel-corruption"]
    matched = 0
    for method in ["classical", "surrogate"]:
        initial = [
            e
            for ev in args.evaluation
            for e in json.loads(ev.read_text())["cases"]
            if e["case"] == "silent-channel-corruption" and e["method"] == method
        ]
        if len(initial) == 2:
            a, b = (json.loads(Path(e["report"]).read_text()) for e in initial)
            assert (
                a["search"] == b["search"]
                and a["sensitivity_probes"] == b["sensitivity_probes"]
            )
            matched += 1
    del negatives
    assert metric_error < 1e-10
    result = {
        "reports_checked": len(rows),
        "qualification_clips_checked": clips_checked,
        "raw_native_renders_verified": renders,
        "maximum_metric_difference": metric_error,
        "maximum_reopen_error": reopen_error,
        "legacy_acceptance_changes": legacy_changes,
        "negative_search_and_sensitivity_bit_identical_before_after": matched,
        "rows": rows,
        "scope": "Raw qualification/admission checks only; optimizer numerical algebra was separately verified. Synthetic rate/layout shifts are not real-material, perceptual or deadline acceptance.",
    }
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({k: v for k, v in result.items() if k != "rows"}, indent=2))


if __name__ == "__main__":
    main()
