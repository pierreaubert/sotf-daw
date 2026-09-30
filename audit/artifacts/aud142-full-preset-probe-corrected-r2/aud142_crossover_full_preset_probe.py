#!/usr/bin/env python3
"""Exercise full Crossover presets separately from partial C-ABI state updates."""

import argparse
import ctypes as c
import hashlib
import json
import math
import sys
from contextlib import ExitStack
from pathlib import Path


def sha(data):
    return hashlib.sha256(data).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("library", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    assert sys.byteorder == "little"
    args.output.mkdir(parents=True, exist_ok=False)
    lib = c.CDLL(str(args.library.resolve()))
    byte_ptr = c.POINTER(c.c_uint8)
    signatures = {
        "plugin_create": ([c.c_char_p, c.c_char_p, c.c_uint32, c.c_size_t, c.c_size_t], c.c_void_p),
        "plugin_destroy": ([c.c_void_p], None),
        "plugin_process": ([c.c_void_p, c.POINTER(c.c_float), c.POINTER(c.c_float), c.c_size_t], c.c_int),
        "plugin_save_state": ([c.c_void_p, c.POINTER(c.c_size_t)], byte_ptr),
        "plugin_load_state": ([c.c_void_p, byte_ptr, c.c_size_t], c.c_int),
        "plugin_export_preset_json": ([c.c_void_p, c.c_char_p, c.POINTER(c.c_size_t)], byte_ptr),
        "plugin_import_preset_json": ([c.c_void_p, byte_ptr, c.c_size_t], c.c_int),
        "plugin_free_state": ([byte_ptr, c.c_size_t], None),
        "plugin_get_last_error": ([], c.c_char_p),
    }
    for name, (arguments, result) in signatures.items():
        getattr(lib, name).argtypes, getattr(lib, name).restype = arguments, result

    def create(stack, config):
        handle = lib.plugin_create(b"Crossover", json.dumps(config).encode(), 48000, 2, 2)
        assert handle, lib.plugin_get_last_error()
        stack.callback(lib.plugin_destroy, handle)
        return handle

    def read(handle, preset=False):
        size = c.c_size_t()
        pointer = (lib.plugin_export_preset_json(handle, b"A descriptive external preset", c.byref(size))
                   if preset else lib.plugin_save_state(handle, c.byref(size)))
        assert pointer, lib.plugin_get_last_error()
        try:
            return c.string_at(pointer, size.value)
        finally:
            lib.plugin_free_state(pointer, size.value)

    def load(handle, payload, preset):
        buffer = (c.c_uint8 * len(payload)).from_buffer_copy(payload)
        function = lib.plugin_import_preset_json if preset else lib.plugin_load_state
        return function(handle, buffer, len(payload))

    def process(handle, offset, blocks):
        audio = bytearray()
        for frames in blocks:
            samples = []
            for frame in range(offset, offset + frames):
                samples.extend((0.31 * math.sin(frame * 0.073) + 0.09 * math.cos(frame * 0.011),
                                0.17 * math.cos(frame * 0.109) - 0.07 * math.sin(frame * 0.023)))
            source = (c.c_float * len(samples))(*samples)
            output = (c.c_float * (frames * 2))()
            status = lib.plugin_process(handle, source, output, frames)
            assert status == 0, (status, lib.plugin_get_last_error())
            assert all(math.isfinite(value) for value in output)
            audio.extend(bytes(output))
            offset += frames
        assert len(audio) == sum(blocks) * 2 * 4
        return bytes(audio)

    def difference(left, right):
        assert len(left) == len(right) and left
        count = len(left) // 4
        a = (c.c_float * count).from_buffer_copy(left)
        b = (c.c_float * count).from_buffer_copy(right)
        return max(abs(x - y) for x, y in zip(a, b))

    base = {"type": "LR48", "frequency": 1260.0, "output": "lowpass"}
    four = {**base, "extra_frequencies": [2600.0, 7800.0]}
    channels = {**base, "channel_frequencies_hz": [720.0, 1800.0],
                "channel_modes": ["lowpass", "highpass"]}
    cases = [
        ("partial_keeps_extra_cutoffs", {**four, "frequency": 1000.0}, four, False, True),
        ("full_two_way_into_four_way", four, base, True, False),
        ("full_four_way_into_two_way", base, four, True, False),
        ("full_same_four_way", {**four, "frequency": 1000.0}, four, True, True),
        ("full_shared_into_per_channel", channels, base, True, False),
        ("full_per_channel_into_shared", base, channels, True, False),
        ("full_enter_fir", base, {**base, "type": "LinearPhase", "fir_taps": 63}, True, True),
        ("full_leave_fir", {**base, "type": "LinearPhase", "fir_taps": 63}, base, True, True),
    ]
    rows = []
    for name, target_config, source_config, preset, success in cases:
        folder = args.output / name
        folder.mkdir()
        row = {"case": name, "preset": preset, "expected_status": 0 if success else -8}
        try:
            with ExitStack() as stack:
                target, twin, cold = [create(stack, target_config) for _ in range(3)]
                source, fresh = [create(stack, source_config) for _ in range(2)]
                assert process(target, 0, [257, 128, 63]) == process(twin, 0, [257, 128, 63])
                before = read(target)
                payload = read(source, preset=True) if preset else b'{"frequency":1260.0}'
                (folder / "input.json").write_bytes(payload)
                (folder / "state_before.json").write_bytes(before)
                row["status"] = load(target, payload, preset)
                row["error"] = (lib.plugin_get_last_error() or b"").decode(errors="replace") if row["status"] else ""
                after = read(target)
                (folder / "state_after.json").write_bytes(after)
                actual = process(target, 448, [127, 509])
                expected = process(fresh if success else twin, 448, [127, 509])
                control = process(cold, 448, [127, 509])
                row["state_matches"] = (json.loads(after) == json.loads(read(source)) if success else after == before)
                row["audio_exact"] = actual == expected
                row["max_residual"] = difference(actual, expected)
                row["control_sensitivity"] = difference(expected, control)
                row["audio"] = {}
                for label, audio in [("actual", actual), ("reference", expected), ("cold_target", control)]:
                    file = folder / (label + ".f32le")
                    file.write_bytes(audio)
                    row["audio"][label] = {"file": str(file.relative_to(args.output)),
                                           "samples": len(audio) // 4, "sha256": sha(audio)}
                row["passed"] = (row["status"] == row["expected_status"] and row["state_matches"]
                                 and row["audio_exact"] and row["control_sensitivity"] > 1e-6)
        except (AssertionError, ValueError) as error:
            row.update(passed=False, error=str(error))
        (folder / "result.json").write_text(json.dumps(row, indent=2) + "\n")
        rows.append(row)
    report = {"library_sha256": sha(args.library.read_bytes()),
              "probe_sha256": sha(Path(__file__).read_bytes()), "sample_rate": 48000,
              "channels": 2, "audio_format": "f32le",
              "scope": "Public full preset topology, partial state merge, populated refusal history and family changes; production DSP route oracle",
              "rows": rows, "passed": all(row["passed"] for row in rows)}
    (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"passed": report["passed"], "cases": len(rows),
                      "failed_cases": [row["case"] for row in rows if not row["passed"]]}))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
