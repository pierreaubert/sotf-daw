#!/usr/bin/env python3
"""Check Crossover C-ABI state routing; fresh DSP is the route reference, not a math oracle."""

import argparse
import ctypes as c
import hashlib
import json
import math
import sys
from contextlib import ExitStack
from pathlib import Path


def digest(data):
    return hashlib.sha256(data).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("library", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    assert sys.byteorder == "little", "Audio artifacts use f32le"
    args.output.mkdir(parents=True, exist_ok=False)
    lib = c.CDLL(str(args.library.resolve()))
    byte_ptr = c.POINTER(c.c_uint8)
    signatures = {
        "plugin_create": ([c.c_char_p, c.c_char_p, c.c_uint32, c.c_size_t, c.c_size_t], c.c_void_p),
        "plugin_destroy": ([c.c_void_p], None),
        "plugin_process": ([c.c_void_p, c.POINTER(c.c_float), c.POINTER(c.c_float), c.c_size_t], c.c_int),
        "plugin_save_state": ([c.c_void_p, c.POINTER(c.c_size_t)], byte_ptr),
        "plugin_load_state": ([c.c_void_p, byte_ptr, c.c_size_t], c.c_int),
        "plugin_free_state": ([byte_ptr, c.c_size_t], None),
        "plugin_get_last_error": ([], c.c_char_p),
    }
    for name, (argtypes, restype) in signatures.items():
        getattr(lib, name).argtypes, getattr(lib, name).restype = argtypes, restype

    def create(stack, config, outputs):
        handle = lib.plugin_create(b"Crossover", json.dumps(config).encode(), 48000, 2, outputs)
        assert handle, lib.plugin_get_last_error()
        stack.callback(lib.plugin_destroy, handle)
        return handle

    def state(handle):
        size = c.c_size_t()
        pointer = lib.plugin_save_state(handle, c.byref(size))
        assert pointer, lib.plugin_get_last_error()
        try:
            return c.string_at(pointer, size.value)
        finally:
            lib.plugin_free_state(pointer, size.value)

    def load(handle, payload):
        encoded = json.dumps(payload, separators=(",", ":"), allow_nan=False).encode()
        buffer = (c.c_uint8 * len(encoded)).from_buffer_copy(encoded)
        return lib.plugin_load_state(handle, buffer, len(encoded))

    def process(handle, outputs, offset, blocks):
        combined = bytearray()
        for frames in blocks:
            samples = []
            for frame in range(offset, offset + frames):
                samples.extend((0.31 * math.sin(frame * 0.073) + 0.09 * math.cos(frame * 0.011),
                                0.17 * math.cos(frame * 0.109) - 0.07 * math.sin(frame * 0.023)))
            source = (c.c_float * len(samples))(*samples)
            output = (c.c_float * (frames * outputs))()
            status = lib.plugin_process(handle, source, output, frames)
            assert status == 0, (status, lib.plugin_get_last_error())
            assert all(math.isfinite(sample) for sample in output)
            combined.extend(bytes(output))
            offset += frames
        assert len(combined) == sum(blocks) * outputs * 4
        return bytes(combined)

    def difference(left, right):
        assert len(left) == len(right) and len(left) > 0
        count = len(left) // 4
        lhs, rhs = (c.c_float * count).from_buffer_copy(left), (c.c_float * count).from_buffer_copy(right)
        return max(abs(a - b) for a, b in zip(lhs, rhs))

    def write_audio(folder, name, audio):
        path = folder / (name + ".f32le")
        path.write_bytes(audio)
        return {"file": str(path.relative_to(args.output)), "samples": len(audio) // 4,
                "sha256": digest(audio)}

    families = ["LR24", "LinearPhase", "LR12", "LR48", "BW6", "BW12", "BW18",
                "BW24", "BW30", "BW36", "BW42", "BW48", "Bessel12"]
    valid = [(family, family, {}, 4) for family in families]
    valid += [("fir_alias_" + alias, "LinearPhase", {"import_alias": alias}, 4)
              for alias in ["FIR", "linear_phase", "LPFIR"]]
    valid += [("per_channel", "Bessel12", {"output": "lowpass",
               "channel_frequencies_hz": [720.0, 1800.0],
               "channel_modes": ["lowpass", "highpass"]}, 2),
              ("multiway", "LR48", {"extra_frequencies": [2600.0, 7800.0]}, 8)]
    invalid = [
        ("unknown_family", {"type": "future_filter"}),
        ("family_boolean", {"type": True}),
        ("family_null", {"type": None}),
        ("mode_boolean", {"mode": False}),
        ("unknown_mode", {"mode": "future_mode"}),
        ("frequency_string", {"frequency": "1260"}),
        ("frequency_boolean", {"frequency": True}),
        ("frequency_below_range", {"frequency": 1.0}),
        ("frequency_above_range", {"frequency": 24000.0}),
        ("new_band", {"frequency_2": 2600.0}),
        ("per_channel_topology", {"channel_frequency_0": 720.0, "channel_mode_0": "lowpass"}),
        ("both_width", {"mode": "both"}),
    ]
    rows = []
    for name, family, extra, outputs in valid:
        folder = args.output / ("valid_" + name)
        folder.mkdir()
        row = {"case": name, "kind": "valid", "expected_status": 0, "outputs": outputs}
        try:
            alias = extra.get("import_alias")
            settings = {key: value for key, value in extra.items() if key != "import_alias"}
            source_config = {"type": family, "frequency": 930.0, "output": "both", "fir_taps": 63,
                             **settings}
            target_config = {**source_config, "type": "LR24"}
            with ExitStack() as stack:
                source = create(stack, source_config, outputs)
                target = create(stack, target_config, outputs)
                reference = create(stack, source_config, outputs)
                old_reference = create(stack, target_config, outputs)
                payload = json.loads(state(source))
                if alias:
                    payload["type"] = alias
                (folder / "input_state.json").write_text(json.dumps(payload, indent=2) + "\n")
                process(target, outputs, 0, [257, 128, 63])
                row["status"] = load(target, payload)
                row["state_matches_source"] = json.loads(state(target)) == json.loads(state(source))
                actual = process(target, outputs, 448, [127, 257, 509])
                expected = process(reference, outputs, 448, [127, 257, 509])
                old = process(old_reference, outputs, 448, [127, 257, 509])
                row["audio_matches_fresh"] = actual == expected
                row["fresh_max_residual"] = difference(actual, expected)
                row["old_family_sensitivity"] = difference(expected, old)
                row["audio"] = {"actual": write_audio(folder, "actual", actual),
                                "reference": write_audio(folder, "reference", expected),
                                "old_family": write_audio(folder, "old_family", old)}
                row["passed"] = row["status"] == 0 and row["state_matches_source"] and row["audio_matches_fresh"]
                if family != "LR24":
                    row["passed"] &= row["old_family_sensitivity"] > 1e-6
        except (AssertionError, ValueError) as error:
            row.update(passed=False, error=str(error))
        (folder / "result.json").write_text(json.dumps(row, indent=2) + "\n")
        rows.append(row)

    config = {"type": "LR48", "frequency": 1260.0, "output": "lowpass"}
    for name, payload in invalid:
        folder = args.output / ("invalid_" + name)
        folder.mkdir()
        row = {"case": name, "kind": "invalid", "expected_status": -8}
        try:
            with ExitStack() as stack:
                target, twin, cold = [create(stack, config, 2) for _ in range(3)]
                assert process(target, 2, 0, [257, 128, 63]) == process(twin, 2, 0, [257, 128, 63])
                before = state(target)
                (folder / "state_before.json").write_bytes(before)
                (folder / "input_state.json").write_text(json.dumps(payload, indent=2) + "\n")
                row["status"] = load(target, payload)
                after = state(target)
                (folder / "state_after.json").write_bytes(after)
                row["state_unchanged"] = after == before
                actual = process(target, 2, 448, [127, 509])
                expected = process(twin, 2, 448, [127, 509])
                cold_audio = process(cold, 2, 448, [127, 509])
                row["continuation_exact"] = actual == expected
                row["cold_sensitivity"] = difference(expected, cold_audio)
                row["audio"] = {"actual": write_audio(folder, "actual", actual),
                                "twin": write_audio(folder, "twin", expected),
                                "cold": write_audio(folder, "cold", cold_audio)}
                row["passed"] = (row["status"] == -8 and row["state_unchanged"]
                                 and row["continuation_exact"] and row["cold_sensitivity"] > 1e-6)
        except (AssertionError, ValueError) as error:
            row.update(passed=False, error=str(error))
        (folder / "result.json").write_text(json.dumps(row, indent=2) + "\n")
        rows.append(row)

    report = {"library_sha256": digest(args.library.read_bytes()),
              "probe_sha256": digest(Path(__file__).read_bytes()),
              "sample_rate": 48000, "input_channels": 2, "audio_format": "f32le",
              "scope": "C-ABI state routing and history; production DSP fresh reference, not independent filter math",
              "rows": rows, "passed": all(row["passed"] for row in rows)}
    (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"passed": report["passed"], "cases": len(rows),
                      "failed_cases": [row["case"] for row in rows if not row["passed"]]}))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
