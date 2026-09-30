#!/usr/bin/env python3
"""Probe preset-envelope refusal through the exported C ABI, without Rust hooks."""

import argparse
import copy
import ctypes as c
import hashlib
import json
import math
import sys
from contextlib import ExitStack
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("library", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    assert sys.byteorder == "little", "The recorded audio format is f32le"
    args.output.mkdir(parents=True, exist_ok=True)
    lib = c.CDLL(str(args.library.resolve()))
    byte_ptr = c.POINTER(c.c_uint8)
    signatures = {
        "plugin_create": ([c.c_char_p, c.c_char_p, c.c_uint32, c.c_size_t, c.c_size_t], c.c_void_p),
        "plugin_destroy": ([c.c_void_p], None),
        "plugin_process": ([c.c_void_p, c.POINTER(c.c_float), c.POINTER(c.c_float), c.c_size_t], c.c_int),
        "plugin_save_state": ([c.c_void_p, c.POINTER(c.c_size_t)], byte_ptr),
        "plugin_export_preset_json": ([c.c_void_p, c.c_char_p, c.POINTER(c.c_size_t)], byte_ptr),
        "plugin_import_preset_json": ([c.c_void_p, byte_ptr, c.c_size_t], c.c_int),
        "plugin_free_state": ([byte_ptr, c.c_size_t], None),
        "plugin_get_last_error": ([], c.c_char_p),
    }
    for name, (argtypes, restype) in signatures.items():
        getattr(lib, name).argtypes = argtypes
        getattr(lib, name).restype = restype

    config = {"num_bands": 4, "threshold": -24.0, "ratio": 3.0,
              "attack_ms": 2.0, "release_ms": 55.0, "knee": 3.0,
              "link_channels": True, "mix": 1.0, "bands": [{"gain": 6.0}]}

    def create(stack, family="DynamicEQ", settings=None):
        payload = config if settings is None else settings
        handle = lib.plugin_create(family.encode(), json.dumps(payload).encode(), 48000, 2, 2)
        assert handle, lib.plugin_get_last_error()
        stack.callback(lib.plugin_destroy, handle)
        return handle

    def owned_bytes(function, *call_args):
        length = c.c_size_t()
        pointer = function(*call_args, c.byref(length))
        assert pointer, lib.plugin_get_last_error()
        try:
            return c.string_at(pointer, length.value)
        finally:
            lib.plugin_free_state(pointer, length.value)

    def state(handle):
        return json.loads(owned_bytes(lib.plugin_save_state, handle))

    def process(handle, offset, frames):
        samples = []
        for n in range(offset, offset + frames):
            samples.extend((0.3 * math.sin(2 * math.pi * 440 * n / 48000),
                            0.17 * math.cos(2 * math.pi * 997 * n / 48000)))
        source = (c.c_float * len(samples))(*samples)
        output = (c.c_float * len(samples))()
        result = lib.plugin_process(handle, source, output, frames)
        assert result == 0, (result, lib.plugin_get_last_error())
        assert all(math.isfinite(value) for value in output)
        return bytes(output), list(output)

    def import_document(handle, document):
        payload = json.dumps(document, separators=(",", ":")).encode()
        buffer = (c.c_uint8 * len(payload)).from_buffer_copy(payload)
        return lib.plugin_import_preset_json(handle, buffer, len(payload))

    variants = [
        ("unchanged", None, None, True),
        ("editable_name", "preset_name", "A renamed preset", True),
        ("wrong_plugin", "plugin_type", "Gain", False),
        ("missing_plugin", "plugin_type", None, False),
        ("typed_plugin", "plugin_type", 42, False),
        ("future_schema", "schema_version", 2, False),
        ("missing_schema", "schema_version", None, False),
        ("typed_schema", "schema_version", "1", False),
        ("wrong_ut_type", "ut_type", "org.example.unrelated", False),
        ("missing_ut_type", "ut_type", None, False),
        ("typed_ut_type", "ut_type", False, False),
    ]
    results = []
    for name, key, value, should_accept in variants:
        with ExitStack() as stack:
            live, twin, cold = (create(stack) for _ in range(3))
            # Establish that the deliberately wrong family is a real public family.
            if name == "wrong_plugin":
                create(stack, "Gain", {})
            for block in range(3):
                assert process(live, block * 257, 257)[0] == process(twin, block * 257, 257)[0]
            before = state(live)
            document = json.loads(owned_bytes(lib.plugin_export_preset_json, live, b"AUD144"))
            original = copy.deepcopy(document)
            assert document["plugin_type"] == "DynamicEQ"
            if key is not None:
                if value is None:
                    del document[key]
                else:
                    document[key] = value
            code = import_document(live, document)
            after = state(live)
            audio = {"live": [], "twin": [], "cold": []}
            blobs = {label: b"" for label in audio}
            for offset, frames in ((771, 127), (898, 509)):
                for label, handle in (("live", live), ("twin", twin), ("cold", cold)):
                    raw, values = process(handle, offset, frames)
                    blobs[label] += raw
                    audio[label].extend(values)
            residual = max(abs(a - b) for a, b in zip(audio["live"], audio["twin"]))
            sensitivity = max(abs(a - b) for a, b in zip(audio["cold"], audio["twin"]))
            assert sensitivity > 1e-6, "Continuation must distinguish fresh and populated DSP"
            if should_accept:
                assert code == 0 and before == after
                assert blobs["live"] == blobs["cold"], "A valid import must restore fresh prepared audio"
            files = {}
            for label, blob in blobs.items():
                filename = f"{name}-{label}.f32le"
                (args.output / filename).write_bytes(blob)
                files[filename] = hashlib.sha256(blob).hexdigest()
            (args.output / f"{name}-preset.json").write_text(json.dumps(document, indent=2) + "\n")
            results.append({"case": name, "expected_accept": should_accept, "return_code": code,
                            "payload_bytes_unchanged": original["state"] == document["state"],
                            "saved_state_unchanged": before == after,
                            "continuation_exact": blobs["live"] == blobs["twin"],
                            "max_continuation_residual": residual,
                            "cold_sensitivity_max": sensitivity, "samples_per_vector": len(audio["live"]),
                            "audio_sha256": files})

    failures = [row["case"] for row in results if not row["expected_accept"] and
                (row["return_code"] == 0 or not row["saved_state_unchanged"] or not row["continuation_exact"])]
    report = {"library": str(args.library.resolve()),
              "library_sha256": hashlib.sha256(args.library.read_bytes()).hexdigest(),
              "probe_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "config": config, "results": results, "failed_refusal_cases": failures,
              "scope": "Control-thread C ABI import/state/audio continuation; no callback heap or AU runtime claim"}
    (args.output / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"failed_refusal_cases": failures, "results": results}, indent=2))
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
