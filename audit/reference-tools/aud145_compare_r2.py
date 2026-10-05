#!/usr/bin/env python3
"""Compare explicit Rust public-API captures with independent AUD145 r2 vectors."""
import argparse
import hashlib
import json
import math
import struct
from pathlib import Path

GENERATOR_SHA256 = "b595231e814123e8c5134a24edde15ecc303a5b55066af4960133c2f32c8dd76"
RATES = (44_100, 48_000, 96_000)
LABELS = {"stereo": ("stereo",), "left": ("left",), "right": ("right",),
          "mid": ("mid",), "side": ("side",), "left-mid": ("left", "mid"),
          "mid-left": ("mid", "left")}


def decode(path, width, count):
    data = path.read_bytes()
    assert len(data) == width * count, f"{path}: unexpected byte count"
    marker = "f" if width == 4 else "d"
    values = struct.unpack("<" + marker * count, data)
    assert all(math.isfinite(value) for value in values), f"{path}: nonfinite sample"
    return values


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--reference", type=Path, required=True)
    parser.add_argument("--capture", type=Path)
    parser.add_argument("--verify-reference-only", action="store_true")
    args = parser.parse_args()
    assert args.verify_reference_only != (args.capture is not None), "choose verification or capture comparison"
    manifest = json.loads((args.reference / "cases.json").read_text())
    assert manifest["schema"] == "aud145-analytical-reference-r2"
    assert manifest["generator_sha256"] == GENERATOR_SHA256
    assert manifest["seed"] is None
    assert manifest["mathematical_source"] == "https://www.w3.org/TR/2021/NOTE-audio-eq-cookbook-20210608/"
    assert manifest["filter"] == {"type": "peak", "frequency": 1379.0,
                                   "q": 0.83, "gain_db": 7.0, "order": 2}
    assert len(manifest["cases"]) == 42
    assert len(manifest["files"]) == 48
    expected_cases = []
    expected_names = set()
    for rate in RATES:
        for channels in (2, 5):
            input_name = f"{rate}-{channels}ch-input.f32le"
            expected_names.add(input_name)
            for label, placements in LABELS.items():
                reference = f"{rate}-{channels}ch-{label}.f64le"
                expected_names.add(reference)
                expected_cases.append({"sample_rate": rate, "channels": channels,
                                       "frames": 4096,
                                       "pairs": [[0, 1]] if channels == 2 else [[0, 1], [3, 2]],
                                       "placements": list(placements), "input": input_name,
                                       "reference": reference})
    assert manifest["cases"] == expected_cases, "case grid, order or routing differs"
    names = [record["name"] for record in manifest["files"]]
    assert len(names) == len(set(names)) and set(names) == expected_names
    assert sorted(path.name for path in args.reference.iterdir()) == sorted(expected_names | {"cases.json"})
    for record in manifest["files"]:
        data = (args.reference / record["name"]).read_bytes()
        channels = 2 if "-2ch-" in record["name"] else 5
        width = 4 if record["name"].endswith(".f32le") else 8
        assert len(data) == 4096 * channels * width
        assert len(data) == record["bytes"]
        assert hashlib.sha256(data).hexdigest() == record["sha256"]
    if args.verify_reference_only:
        print("AUD145 r2 reference: 42 cases, 48 verified binaries")
        return
    assert sorted(path.name for path in args.capture.iterdir()) == sorted(
        case["reference"].removesuffix(".f64le") + ".f32le"
        for case in manifest["cases"])
    for case in manifest["cases"]:
        count = case["frames"] * case["channels"]
        name = case["reference"]
        actual = decode(args.capture / (name.removesuffix(".f64le") + ".f32le"), 4, count)
        expected = decode(args.reference / name, 8, count)
        residuals = [float(x) - y for x, y in zip(actual, expected)]
        peak = max(map(abs, residuals))
        rms = math.sqrt(sum(value * value for value in residuals) / count)
        expected_peak = max(map(abs, expected))
        expected_rms = math.sqrt(sum(value * value for value in expected) / count)
        assert peak <= 2e-5 * max(expected_peak, 1.0), (name, "peak", peak)
        assert rms <= 2e-6 * max(expected_rms, 1.0), (name, "rms", rms)
        print(f"PASS {name} peak={peak:.9g} rms={rms:.9g}")
    print("AUD145 r2: 42 passed, 0 failed")


if __name__ == "__main__":
    main()
