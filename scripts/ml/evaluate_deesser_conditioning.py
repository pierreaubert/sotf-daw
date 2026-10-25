#!/usr/bin/env python3
"""Development-only recorded de-esser comparisons, including neural VAD context."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from deesser_conditioning import HOP, PARAMS, RATE, key_policy, observe, render
from scipy.signal import fftconvolve, firwin

SILERO_SHA = "7d47a98acce5f4b688a7d541ee0d9061b64171fdb39581cbcf29fe72bfe57c51"


def attenuation(reference, output):
    before, after = (
        np.sum(reference.astype(np.float64) ** 2),
        np.sum(output.astype(np.float64) ** 2),
    )
    return float(10 * np.log10(before / max(after, 1e-30))) if before > 1e-12 else None


def regions(manifest, reference, output):
    bands = {
        "low": firwin(129, 2000, fs=RATE, window="hann"),
        "high": firwin(129, [4000, 12000], fs=RATE, window="hann", pass_zero=False),
    }
    filtered = {
        name: (
            fftconvolve(reference[:, 0], weights, mode="same"),
            fftconvolve(output[:, 0], weights, mode="same"),
        )
        for name, weights in bands.items()
    }
    rows = []
    for region in manifest:
        begin, end = (
            round(region["start_seconds"] * RATE),
            round(region["end_seconds"] * RATE),
        )
        a, b = reference[begin:end, 0], output[begin:end, 0]
        windows = [
            (start, start + HOP)
            for start in range(0, len(reference) - HOP + 1, HOP)
            if start >= begin
            and start + HOP <= end
            and np.mean(reference[start : start + HOP, 0].astype(np.float64) ** 2)
            >= 1e-8
        ]
        losses = [
            attenuation(reference[first:last, 0], output[first:last, 0])
            for first, last in windows
        ]
        rows.append(
            region
            | {
                "attenuation_db": attenuation(a, b),
                "low_band_attenuation_db": attenuation(
                    filtered["low"][0][begin:end], filtered["low"][1][begin:end]
                ),
                "high_band_attenuation_db": attenuation(
                    filtered["high"][0][begin:end], filtered["high"][1][begin:end]
                ),
                "active_10ms_windows": len(windows),
                "windows_over_1db_attenuation": sum(
                    value is not None and value > 1 for value in losses
                ),
            }
        )
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--observer", type=Path, required=True)
    parser.add_argument("--silero", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.work.exists() or args.output.exists():
        raise ValueError("Use new development paths")
    if hashlib.sha256(args.silero.read_bytes()).hexdigest() != SILERO_SHA:
        raise ValueError("Use the pinned Silero artifact")
    fixtures = Path(
        "crates/sotf-plugins/crates/sotf-plugin-de-esser/tests/fixtures/recorded"
    )
    manifest = json.loads((fixtures / "manifest.json").read_text())
    rows, prefixes = [], []
    for fixture in manifest["fixtures"]:
        raw = (fixtures / fixture["file"]).read_bytes()
        if hashlib.sha256(raw).hexdigest() != fixture["pcm_sha256"]:
            raise ValueError("Recorded fixture changed")
        clean = (np.frombuffer(raw, dtype="<i2").astype(np.float32) / 32768).copy()
        for gain in [1.0, 0.25]:
            x = (clean * gain).astype(np.float32)
            root = args.work / f"{fixture['id']}-{gain}"
            models = {
                "rnnoise": {"type": "rnnoise"},
                "silero": {
                    "type": "silero",
                    "model_path": str(args.silero.resolve()),
                    "sha256": SILERO_SHA,
                },
            }
            traces = {
                name: observe(x, args.observer, model, root / ("observe-" + name))
                for name, model in models.items()
            }
            keys = {"absolute": None}
            relative, decisions = key_policy(x)
            keys["relative"] = relative
            (root / "relative-decisions.json").write_text(json.dumps(decisions) + "\n")
            for name, trace in traces.items():
                key, decisions = key_policy(x, "speech-relative", trace)
                keys[name] = key
                (root / (name + "-decisions.json")).write_text(
                    json.dumps(decisions) + "\n"
                )
                # A real model prefix test includes non-packet-aligned EOF.
                if fixture["id"] in {"s", "violin"} and gain == 1.0:
                    length = 35001
                    prefix_trace = observe(
                        x[:length],
                        args.observer,
                        models[name],
                        root / ("prefix-" + name),
                    )
                    common = [
                        r
                        for r in prefix_trace["rows"]
                        if r["available_at_frame"] % HOP == 0
                    ]
                    if common != trace["rows"][: len(common)]:
                        raise ValueError(
                            "Neural observation prefix depends on future samples"
                        )
                    prefix_key, _ = key_policy(
                        x[:length], "speech-relative", prefix_trace
                    )
                    if not np.array_equal(prefix_key, key[:length]):
                        raise ValueError("Neural key prefix depends on future samples")
                    prefixes.append(
                        {
                            "fixture": fixture["id"],
                            "model": name,
                            "prefix_frames": length,
                            "matching_complete_packets": len(common),
                            "key_bit_identical": True,
                        }
                    )
            programme = np.stack([x, 0.25 * x], axis=1)
            for mode, topology in [
                ("Wideband", "Minimum-Phase"),
                ("Split-Band", "Minimum-Phase"),
                ("Split-Band", "Linear-Phase"),
            ]:
                settings = PARAMS | {"mode": mode, "split_topology": topology}
                variant = root / (mode + "-" + topology)
                reference, _ = render(
                    programme,
                    args.renderer,
                    variant / "reference",
                    params=settings | {"range_db": 0.0},
                )
                for name, mono_key in keys.items():
                    external = (
                        None
                        if mono_key is None
                        else np.stack([mono_key, 0.25 * mono_key], axis=1)
                    )
                    output, metadata = render(
                        programme,
                        args.renderer,
                        variant / name,
                        key=external,
                        params=settings,
                    )
                    if not np.allclose(
                        output[:, 1], 0.25 * output[:, 0], rtol=0, atol=1e-7
                    ):
                        raise ValueError("Linked stereo ratio changed")
                    if mode == "Wideband":
                        active = np.abs(reference[:, 0]) > 1e-5
                        ratio = (
                            output[active, 0].astype(np.float64) / reference[active, 0]
                        )
                        if (
                            ratio.min(initial=1)
                            < 10 ** (-settings["range_db"] / 20) - 2e-6
                            or ratio.max(initial=0) > 1 + 2e-6
                        ):
                            raise ValueError(
                                "Actual native Wideband range bound exceeded"
                            )
                    rows.append(
                        {
                            "fixture": fixture["id"],
                            "source_sha256": fixture["pcm_sha256"],
                            "input_gain": gain,
                            "mode": mode,
                            "topology": topology,
                            "method": name,
                            "work": str((variant / name).resolve()),
                            "reference_work": str((variant / "reference").resolve()),
                            "native": metadata,
                            "regions": regions(fixture["regions"], reference, output),
                        }
                    )
            print(
                json.dumps({"fixture": fixture["id"], "gain": gain, "comparisons": 12}),
                flush=True,
            )
    report = {
        "rows": rows,
        "model_prefix_checks": prefixes,
        "silero_sha256": SILERO_SHA,
        "renderer_sha256": hashlib.sha256(args.renderer.read_bytes()).hexdigest(),
        "observer_sha256": hashlib.sha256(args.observer.read_bytes()).hexdigest(),
        "qualification": "Development-only five existing recordings, one isolated-phoneme speaker. Correlated level/topology variants, no training or threshold tuning, no population/heldout classifier or realtime acceptance.",
    }
    args.output.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
