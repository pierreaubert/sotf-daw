"""Recompute deadline tails and model-serving coverage from raw native traces."""

import argparse
import json
from pathlib import Path


def analyze(report):
    summaries = []
    for row in report["cases"]:
        if row["exit_code"]:
            summaries.append({"case": row["case"], "exit_code": row["exit_code"]})
            continue
        r, request = row["report"], row["request"]
        if r["request"] != request or r["sample_rate"] != 48000:
            raise ValueError("Case request or sample clock differs from native report")
        measured = [t for t in r["trace"] if not t["warmup"]]
        if len(measured) != r["measured_blocks"] or not measured:
            raise ValueError("Native measured trace extent mismatch")
        for t in r["trace"]:
            period = (
                (t["index"] + 1) * request["block_frames"] * 1_000_000_000 // 48000
            ) - (t["index"] * request["block_frames"] * 1_000_000_000 // 48000)
            if (
                t["deadline_ns"] != period
                or abs(t["response_ns"] - t["execution_ns"] - t["start_lateness_ns"])
                > 1
            ):
                raise ValueError("Native pacing arithmetic or trace units disagree")
            if t["execution_overrun"] != (t["execution_ns"] > period) or t[
                "response_miss"
            ] != (t["response_ns"] > period):
                raise ValueError("Native deadline flags disagree with raw timestamps")
        duration = sorted(t["execution_ns"] for t in measured)
        quantiles = {
            "p50": duration[(len(duration) - 1) // 2],
            "p95": duration[(len(duration) - 1) * 95 // 100],
            "p99": duration[(len(duration) - 1) * 99 // 100],
            "maximum": duration[-1],
        }
        if quantiles != r["execution_ns"]:
            raise ValueError("Native execution quantiles disagree with raw traces")
        overruns, misses = (
            sum(t["execution_overrun"] for t in measured),
            sum(t["response_miss"] for t in measured),
        )
        if (
            overruns != r["execution_overruns"]
            or misses != r["response_deadline_misses"]
        ):
            raise ValueError("Native summary hides deadline misses")
        if (
            r["track_render_paths"] != ["Graph"] * request["tracks"]
            or r["bus_render_path"] != "Graph"
        ):
            raise ValueError("Unexpected actual DAW host path")
        packets = []
        if request["deep_filter"]:
            before_rows = r["telemetry_before_measurement"]
            if before_rows is None:
                if r["warmup_blocks"] != 0:
                    raise ValueError("Missing worker baseline after a nonzero warmup")
                before_rows = [
                    {
                        "deep_filter": {
                            k: 0
                            for k in [
                                "timely_packets",
                                "fallback_packets",
                                "dropped_inputs",
                                "worker_errors",
                            ]
                        }
                    }
                    for _ in r["telemetry_after"]
                ]
            for before, after in zip(before_rows, r["telemetry_after"], strict=True):
                a, b = before["deep_filter"], after["deep_filter"]
                if a is None or b is None:
                    raise ValueError(
                        "Requested actual DeepFilter backend has no telemetry"
                    )
                delta = {
                    k: b[k] - a[k]
                    for k in [
                        "timely_packets",
                        "fallback_packets",
                        "dropped_inputs",
                        "worker_errors",
                    ]
                }
                if any(v < 0 for v in delta.values()):
                    raise ValueError("Native worker counters regressed")
                denominator = delta["timely_packets"] + delta["fallback_packets"]
                delta["timely_fraction"] = (
                    delta["timely_packets"] / denominator if denominator else None
                )
                packets.append(delta)
        summaries.append(
            {
                "case": row["case"],
                "tracks": request["tracks"],
                "cpu_workers": request["cpu_workers"],
                "deep_filter": request["deep_filter"],
                "neural_pitch": request["neural_pitch"],
                "denoise_first": request.get("denoise_first", True),
                "measured_callbacks": len(measured),
                "callback_period_ms": request["block_frames"] * 1000 / 48000,
                "execution_ms": {k: v / 1e6 for k, v in quantiles.items()},
                "execution_overruns": overruns,
                "response_deadline_misses": misses,
                "maximum_start_lateness_ms": max(
                    t["start_lateness_ns"] for t in measured
                )
                / 1e6,
                "deep_filter_packets": packets,
                "pitch_neural_selected_snapshots": r["pitch_neural_selected_snapshots"],
                "all_requested_models_observed_serving": (
                    not packets or all(p["timely_packets"] > 0 for p in packets)
                )
                and (
                    not request["neural_pitch"]
                    or all(n > 0 for n in r["pitch_neural_selected_snapshots"])
                ),
                "output_energy": r["output_energy"],
                "output_peak": r["output_peak"],
                "initialization_ms": r["initialization_ms"],
            }
        )
    return {
        "binary_sha256": report["binary_sha256"],
        "environment": report["environment"],
        "cases": summaries,
        "runtime_choice_admitted": False,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    result = analyze(json.loads(args.report.read_text()))
    args.output.write_text(json.dumps(result, indent=2, allow_nan=False) + "\n")
    print(json.dumps({"cases": len(result["cases"]), "runtime_choice_admitted": False}))


if __name__ == "__main__":
    main()
