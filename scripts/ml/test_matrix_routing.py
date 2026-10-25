"""Grouped-routing observability, weighting, uncertainty and export contracts."""

import copy
import hashlib
import json
import tempfile
import unittest
from pathlib import Path

import numpy as np
from fit_matrix_routing import read_examples
from matrix_routing import MAX_ERROR, MAX_GROUP_RMS, MAX_RMS, fit, path, validate
from matrix_routing_cases import example


class RoutingTests(unittest.TestCase):
    def test_noiseless_recovers_reviewable_bounded_matrix(self):
        data, truth = example(1, sources=8, noise=0)
        result = fit(data)
        self.assertTrue(result["accepted"], result["reasons"])
        np.testing.assert_allclose(result["matrix"], truth, atol=3e-8, rtol=0)
        self.assertEqual(result["selected_lambda"], 0)
        self.assertEqual(len(result["routing_table"]), 4)

    def test_source_repeats_do_not_inflate_support(self):
        data, _ = example(2, sources=8)
        repeated = copy.deepcopy(data)
        repeated["groups"] *= 2
        a, b = fit(data), fit(repeated)
        np.testing.assert_array_equal(a["matrix"], b["matrix"])
        np.testing.assert_allclose(a["interval_lower"], b["interval_lower"], atol=1e-12)
        self.assertEqual(a["source_ids"], b["source_ids"])

    def test_identical_stereo_is_not_identified_by_ridge(self):
        data, _ = example(3)
        for group in data["groups"]:
            x = np.array(group["input"])
            x[:, 1] = x[:, 0]
            group["input"] = x.tolist()
        result = fit(data)
        self.assertFalse(result["accepted"])
        self.assertIsNone(result["parameters"])
        self.assertIn("unregularized", result["reasons"][0])

    def test_insufficient_sources_and_false_independence_rejected(self):
        data, _ = example(4, sources=8)
        data["groups"][1] = copy.deepcopy(data["groups"][0])
        data["groups"][1]["source_id"] = "false-new-source"
        with self.assertRaises(ValueError):
            validate(data)
        data, _ = example(5, sources=8)
        for index, group in enumerate(data["groups"]):
            group["source_id"] = str(index % 4)
        with self.assertRaises(ValueError):
            validate(data)

    def test_nonfinite_maps_and_channel_labels_rejected(self):
        data, _ = example(6)
        for change in ["nan", "duplicate-map", "duplicate-label"]:
            bad = copy.deepcopy(data)
            if change == "nan":
                bad["groups"][0]["input"][0][0] = float("nan")
            elif change == "duplicate-map":
                bad["input_channel_map"] = [0, 0]
            else:
                bad["input_labels"] = ["L", "L"]
            with self.assertRaises(ValueError):
                validate(bad)

    def test_nested_holdout_does_not_train_on_excluded_source(self):
        data, _ = example(7, sources=8)
        first = fit(data)
        changed = copy.deepcopy(data)
        changed["groups"][0]["desired_output"] = (
            np.array(changed["groups"][0]["desired_output"]) * 0.7
        ).tolist()
        second = fit(changed)
        source = data["groups"][0]["source_id"]
        a = next(f for f in first["nested_holdouts"] if f["source_id"] == source)
        b = next(f for f in second["nested_holdouts"] if f["source_id"] == source)
        self.assertEqual(a["lambda"], b["lambda"])
        np.testing.assert_array_equal(a["matrix"], b["matrix"])
        self.assertNotEqual(a["errors"], b["errors"])

    def test_one_bad_output_cannot_hide_in_large_output_average(self):
        data, _ = example(20531026, inputs=2, outputs=128, sources=8, noise=0)
        for group in data["groups"]:
            y = np.array(group["desired_output"])
            y[:, -1] = np.where(np.arange(len(y)) % 2, 0.02, -0.02)
            group["desired_output"] = y.tolist()
        result = fit(data)
        # These are the former aggregate-only gates; all would have passed.
        self.assertLess(result["nested_rms"], MAX_RMS)
        self.assertLess(
            max(f["errors"]["rms"] for f in result["nested_holdouts"]), MAX_GROUP_RMS
        )
        self.assertLess(
            max(f["errors"]["max_error"] for f in result["nested_holdouts"]), MAX_ERROR
        )
        self.assertGreater(result["nested_output_rms"][-1], MAX_GROUP_RMS)
        self.assertFalse(result["accepted"])
        self.assertIsNone(result["parameters"])

    def test_wide_under_determined_design_is_rejected(self):
        rng = np.random.default_rng(20531027)
        group = ("one", rng.uniform(-1, 1, (8, 16)), np.zeros((8, 1)))
        with self.assertRaises(ValueError):
            path([group])

    def test_full_width_examples_larger_than_former_json_limit_load(self):
        data, _ = example(20531028, inputs=128, outputs=128, sources=8, noise=0)
        for group in data["groups"]:
            group["input"] *= 4
            group["desired_output"] *= 4
        raw = (json.dumps(data) + "\n").encode()
        self.assertGreater(len(raw), 8 * 1024 * 1024)
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / "examples.json"
            source.write_bytes(raw)
            loaded, digest = read_examples(source)
        self.assertEqual(len(loaded["input_labels"]), 128)
        self.assertEqual(digest, hashlib.sha256(raw).hexdigest())


if __name__ == "__main__":
    unittest.main()
