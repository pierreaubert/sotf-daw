#!/usr/bin/env python3
"""Record genuine FletcherMunson-to-LoudnessCompensation preset migration."""
import argparse
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
    assert sys.byteorder == "little", "Recorded audio format is f32le"
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

    config = {"enabled": True, "playback_volume_db": -25.0,
              "reference_level_db": -14.0, "smoothing_ms": 50.0}

    def create(stack, family, settings):
        handle = lib.plugin_create(family.encode(), json.dumps(settings).encode(), 48000, 2, 2)
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
        values = []
        for n in range(offset, offset + frames):
            values.extend((0.3 * math.sin(2 * math.pi * 73 * n / 48000),
                           0.17 * math.cos(2 * math.pi * 997 * n / 48000)))
        source = (c.c_float * len(values))(*values)
        output = (c.c_float * len(values))()
        code = lib.plugin_process(handle, source, output, frames)
        assert code == 0, (code, lib.plugin_get_last_error())
        assert all(math.isfinite(value) for value in output)
        return bytes(output), list(output)

    with ExitStack() as stack:
        source = create(stack, "FletcherMunson", config)
        migrated = create(stack, "LoudnessCompensation", {})
        default = create(stack, "LoudnessCompensation", {})
        payload = owned_bytes(lib.plugin_export_preset_json, source, b"Legacy FletcherMunson")
        document = json.loads(payload)
        assert document["plugin_type"] == "FletcherMunson"
        (args.output / "genuine-preset.json").write_bytes(payload)
        buffer = (c.c_uint8 * len(payload)).from_buffer_copy(payload)
        code = lib.plugin_import_preset_json(migrated, buffer, len(payload))
        error = lib.plugin_get_last_error()
        saved_states = {"source": state(source), "migrated": state(migrated), "default": state(default)}
        (args.output / "states.json").write_text(json.dumps(saved_states, indent=2) + "\n")
        blobs = {name: b"" for name in saved_states}
        vectors = {name: [] for name in saved_states}
        offset = 0
        for frames in (127, 509, 2048, 2048):
            for name, handle in (("source", source), ("migrated", migrated), ("default", default)):
                blob, values = process(handle, offset, frames)
                blobs[name] += blob
                vectors[name].extend(values)
            offset += frames
        hashes = {}
        for name, blob in blobs.items():
            path = args.output / f"{name}.f32le"
            path.write_bytes(blob)
            hashes[path.name] = hashlib.sha256(blob).hexdigest()
        result = {"library_sha256": hashlib.sha256(args.library.read_bytes()).hexdigest(),
                  "probe_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                  "config": config, "return_code": code,
                  "error": error.decode() if error else None,
                  "states_equal": saved_states["source"] == saved_states["migrated"],
                  "audio_equal": blobs["source"] == blobs["migrated"],
                  "max_residual": max(abs(a-b) for a,b in zip(vectors["source"], vectors["migrated"])),
                  "default_sensitivity": max(abs(a-b) for a,b in zip(vectors["source"], vectors["default"])),
                  "samples_per_vector": len(vectors["source"]), "audio_sha256": hashes}
        (args.output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result, indent=2))
        assert result["default_sensitivity"] > 1e-6, "Fixture must distinguish the preset from defaults"
        return 0 if code == 0 and result["states_equal"] and result["audio_equal"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
