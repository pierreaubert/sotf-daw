"""Model contracts and real native alignment without changing repair DSP."""

import hashlib
import io
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import numpy as np
from declick_cases import case
from declick_model import FORMAT, Model, qualify
from declick_native import features, render


def document():
    return {
        "format": FORMAT,
        "domain": {
            "sample_rate": 48000,
            "channels": 1,
            "native_params": "legacy-sensitivity5-linked-v1",
        },
        "mean": [0] * 16,
        "scale": [1] * 16,
        "w1": np.zeros((16, 16)).tolist(),
        "b1": [0] * 16,
        "w2": [0] * 16,
        "b2": 0.0,
        "threshold": 0.9,
    }


class DeclickModelTests(unittest.TestCase):
    def test_model_checks_hash_shapes_and_finite_values(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "model.json"
            path.write_text(json.dumps(document()))
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            model = Model(path, digest)
            self.assertEqual(model.probability(np.zeros(16)), 0.5)
            with self.assertRaises(ValueError):
                Model(path, "0" * 64)
            for key, value in [
                ("w1", []),
                ("scale", [0] * 16),
                ("b2", float("nan")),
                ("b2", 1e300),
                ("threshold", -1),
            ]:
                data = document()
                data[key] = value
                path.write_text(json.dumps(data))
                with self.subTest(key=key), self.assertRaises(ValueError):
                    Model(path, hashlib.sha256(path.read_bytes()).hexdigest())

    def test_model_hash_and_parse_use_same_bytes_after_path_replacement(self):
        original = json.dumps(document()).encode()
        replacement = document()
        replacement["b2"] = 2.0
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "model.json"
            path.write_bytes(original)
            real_open = Path.open

            class ReplacingReader(io.BytesIO):
                def read(self, size=-1):
                    value = super().read(size)
                    path.write_text(json.dumps(replacement))
                    return value

            def opening(candidate, mode="r", *args, **kwargs):
                if candidate == path and mode == "rb":
                    return ReplacingReader(original)
                return real_open(candidate, mode, *args, **kwargs)

            with patch.object(Path, "open", opening):
                model = Model(path, hashlib.sha256(original).hexdigest())
            self.assertEqual(model.probability(np.zeros(16)), 0.5)
            self.assertEqual(json.loads(path.read_bytes())["b2"], 2.0)

    def test_only_whole_native_groups_can_change(self):
        _, dry, _ = case(20341026, "click")
        native = dry.copy()
        native[4800:4803] = 0
        model = SimpleNamespace(threshold=0.9, probability=lambda _: 0.99)
        accepted, decisions = qualify(dry, native, 48000, model)
        np.testing.assert_array_equal(accepted, native)
        self.assertTrue(decisions[0]["accepted"])
        model.probability = lambda _: 0.5
        rejected, decisions = qualify(dry, native, 48000, model)
        np.testing.assert_array_equal(rejected, dry)
        self.assertFalse(decisions[0]["accepted"])

    def test_unavailable_model_unsupported_rates_stereo_and_edges_stay_dry(self):
        _, dry, _ = case(20341027, "click")
        native = dry.copy()
        native[1:3] = 0
        native[4800] = 0
        model = SimpleNamespace(threshold=0.9, probability=lambda _: 0.99)
        for rate, channels, active in [
            (48000, 1, None),
            (44100, 1, model),
            (48000, 2, model),
        ]:
            original, repaired = (
                np.tile(dry, (1, channels)),
                np.tile(native, (1, channels)),
            )
            output, _ = qualify(original, repaired, rate, active)
            np.testing.assert_array_equal(output, original)
        output, decisions = qualify(dry, native, 48000, model)
        np.testing.assert_array_equal(output[:3], dry[:3])
        self.assertEqual(decisions[0]["reason"], "unsupported_context_or_features")

    def test_semantically_opposite_impulses_have_identical_observations(self):
        clean_click, click, _ = case(20341028, "click")
        clean_programme, programme, _ = case(20341028, "ambiguous-programme")
        np.testing.assert_array_equal(click, programme)
        self.assertFalse(np.array_equal(clean_click, clean_programme))
        native = click.copy()
        native[4800:4804] = 0
        np.testing.assert_array_equal(
            features(click, native, 4800, 4804, 48000),
            features(programme, native, 4800, 4804, 48000),
        )


@unittest.skipUnless(
    os.environ.get("SOTF_DECLICK_RENDERER"), "native exporter not configured"
)
class NativeDeclickTests(unittest.TestCase):
    def test_partition_rate_channel_and_disabled_alignment(self):
        renderer = os.environ["SOTF_DECLICK_RENDERER"]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for rate in [44100, 48000, 96000, 192000]:
                for channels in [1, 2, 8]:
                    _, dry, _ = case(20341029, "click", rate, channels)
                    reference, info = render(
                        dry, rate, renderer, root / f"reference-{rate}-{channels}", 257
                    )
                    for block in [1, 31, 1024]:
                        actual, metadata = render(
                            dry,
                            rate,
                            renderer,
                            root / f"block-{rate}-{channels}-{block}",
                            block,
                        )
                        np.testing.assert_array_equal(actual, reference)
                        self.assertEqual(
                            metadata["drained_frames"], info["latency_samples"]
                        )
                    params = {"enabled": False}
                    disabled, _ = render(
                        dry,
                        rate,
                        renderer,
                        root / f"disabled-{rate}-{channels}",
                        params=params,
                    )
                    np.testing.assert_array_equal(disabled, dry)

    def test_owned_modes_preserve_declared_drain_and_partial_callbacks(self):
        renderer = os.environ["SOTF_DECLICK_RENDERER"]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for rate in [48000, 96000]:
                _, dry, _ = case(20341030, "click", rate, 2)
                for bands, width in [(0, 4), (2, 8)]:
                    params = {"mode": 1, "bands": bands, "repair_width": width}
                    reference, info = render(
                        dry,
                        rate,
                        renderer,
                        root / f"owned-{rate}-{bands}",
                        257,
                        params=params,
                    )
                    actual, _ = render(
                        dry,
                        rate,
                        renderer,
                        root / f"owned-small-{rate}-{bands}",
                        31,
                        params=params,
                    )
                    np.testing.assert_array_equal(actual, reference)
                    self.assertEqual(info["latency_samples"], 8 + width)
                    self.assertEqual(info["drained_frames"], 8 + width)
                    self.assertEqual(info["output_frames"], len(dry) + 8 + width)

    def test_invalid_geometry_unknown_params_and_existing_output_are_rejected(self):
        renderer = os.environ["SOTF_DECLICK_RENDERER"]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            input_path, output = root / "input.f32le", root / "output.f32le"
            np.zeros(100, dtype="<f4").tofile(input_path)
            request = {
                "input": str(input_path),
                "output": str(output),
                "sample_rate": 48000,
                "frames": 100,
                "channels": 1,
                "callback_frames": 31,
                "params": {},
            }
            for update in [
                {"frames": 101},
                {"callback_frames": 0},
                {"channels": 0},
                {"params": {"typo": 1}},
                {"params": {"sensitivity": 0}},
                {"params": {"audition_residual": True}},
            ]:
                path = root / "request.json"
                path.write_text(json.dumps(dict(request, **update)))
                result = subprocess.run(
                    [renderer, str(path)], capture_output=True, text=True, check=False
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(output.exists())
            output.write_bytes(b"preserve existing output")
            path.write_text(json.dumps(request))
            result = subprocess.run(
                [renderer, str(path)], capture_output=True, text=True, check=False
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(output.read_bytes(), b"preserve existing output")


if __name__ == "__main__":
    unittest.main()
