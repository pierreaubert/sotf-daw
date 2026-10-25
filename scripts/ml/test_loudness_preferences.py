"""Posterior, grouping, calibration and abstention checks on synthetic sessions."""

import copy
import os
import tempfile
import unittest
from pathlib import Path

import numpy as np
from loudness_preference_cases import session
from loudness_preference_native import qualify
from loudness_preferences import POLICY, fit, kernel, regress, validate


class LoudnessPreferenceTests(unittest.TestCase):
    def test_conditioned_kernel_and_posterior_match_direct_block_covariance(self):
        x = np.array([50, 60, 70, 83.0])
        full = np.r_[x, 55.0, 83.0]
        base = 9 * np.exp(-0.5 * ((full[:, None] - full[None, :]) / 10) ** 2)
        conditional = (
            base[:-1, :-1] - np.outer(base[:-1, -1], base[-1, :-1]) / base[-1, -1]
        )
        np.testing.assert_allclose(
            kernel(full[:-1], full[:-1], 83), conditional, atol=2e-15, rtol=0
        )
        y = np.stack([np.arange(4), -np.arange(4), np.arange(4) / 2], axis=1)
        noise, weights = np.full(4, 0.25), np.ones(4)
        mean, variance, _ = regress(x, y, noise, weights, 55, 83)
        covariance = conditional[:4, :4] + np.eye(4) * 0.25**2
        expected = conditional[4, :4] @ np.linalg.solve(covariance, y)
        expected_variance = conditional[4, 4] - conditional[4, :4] @ np.linalg.solve(
            covariance, conditional[:4, 4]
        )
        np.testing.assert_allclose(mean, expected, atol=1e-12, rtol=0)
        self.assertAlmostEqual(variance, expected_variance, places=12)

    def test_consistent_preferences_return_bounded_explicit_manual_curve(self):
        data = session()
        result = fit(data, 55)
        self.assertTrue(result["accepted"], result["reasons"])
        self.assertEqual(result["parameters"]["mode"], 0)
        self.assertFalse(result["parameters"]["auto_calibrated"])
        self.assertFalse(result["parameters"]["auto_gain_enabled"])
        self.assertTrue(result["parameters"]["headroom_normalized"])
        self.assertLess(np.max(np.abs(result["predicted_gains_db"])), 6)

    def test_group_repetition_does_not_create_false_independence(self):
        data = session()
        original = fit(data, 55)
        repeated = copy.deepcopy(data)
        for repeat in range(5):
            for row in data["observations"]:
                row = dict(row, id=row["id"] + f"-repeat-{repeat}")
                repeated["observations"].append(row)
        result = fit(repeated, 55)
        for field in [
            "predicted_gains_db",
            "interval_low_db",
            "interval_high_db",
            "lopo_rmse_db",
        ]:
            np.testing.assert_allclose(
                result[field], original[field], atol=1e-12, rtol=0
            )
        self.assertEqual(result["programmes"], 8)
        self.assertEqual(len(result["cells"]), len(original["cells"]))

    def test_conflicting_preferences_fail_despite_same_latent_variance(self):
        good, bad = fit(session(), 55), fit(session(kind="contradictory"), 55)
        self.assertEqual(good["latent_variance_db2"], bad["latent_variance_db2"])
        self.assertFalse(bad["accepted"])
        self.assertIn("held_programme_preferences_inconsistent", bad["reasons"])
        self.assertIsNone(bad["parameters"])

    def test_uncertain_out_of_span_and_insufficient_groups_abstain(self):
        self.assertFalse(fit(session(kind="uncertain"), 55)["accepted"])
        self.assertFalse(fit(session(), 40)["accepted"])
        data = session()
        data["observations"] = [
            r for r in data["observations"] if r["programme_id"] == "programme-0"
        ]
        self.assertFalse(fit(data, 55)["accepted"])

    def test_inconsistent_repeats_and_gain_bound_intervals_abstain(self):
        data = session()
        original = data["observations"][0]
        for repeat in range(4):
            row = copy.deepcopy(original)
            row["id"] += f"-conflict-{repeat}"
            row["preferred_gains_db"] = [5 if repeat % 2 else -5, 0, 0]
            data["observations"].append(row)
        result = fit(data, 55)
        self.assertFalse(result["accepted"])
        self.assertIn("inconsistent_repeated_preferences", result["reasons"])
        data = session()
        for row in data["observations"]:
            if row["level_db_spl"] != 83:
                row["preferred_gains_db"][0] = 5.8
        result = fit(data, 60)
        self.assertFalse(result["accepted"])
        self.assertIn("uncertainty_interval_exceeds_gain_bounds", result["reasons"])

    def test_reference_only_programmes_cannot_supply_support(self):
        data = session()
        data["observations"] = [
            row
            for row in data["observations"]
            if row["programme_id"]
            in ("programme-0", "programme-1", "programme-2", "programme-3")
            or row["level_db_spl"] == 83
        ]
        result = fit(data, 55)
        self.assertFalse(result["accepted"])
        self.assertIn("insufficient_within_programme_level_coverage", result["reasons"])

    def test_reference_zero_is_imposed_and_calibration_is_required(self):
        result = fit(session(), 83)
        self.assertTrue(result["accepted"])
        self.assertEqual(result["predicted_gains_db"], [0, 0, 0])
        self.assertEqual(result["interval_high_db"], [0, 0, 0])
        for key, value in [
            ("confirmed", False),
            ("weighting", "A"),
            ("level_basis", "pcm_lufs"),
        ]:
            data = session()
            data["calibration"][key] = value
            with self.assertRaises(ValueError):
                validate(data)
        self.assertEqual(POLICY["maximum_abs_gain_db"], 6)


@unittest.skipUnless(
    os.environ.get("SOTF_LOUDNESS_PREFERENCE_RENDERER"),
    "native renderer not configured",
)
class NativeLoudnessPreferenceTests(unittest.TestCase):
    def test_actual_native_response_matches_reference_across_rates_and_callbacks(self):
        params = fit(session(), 55)["parameters"]
        renderer = os.environ["SOTF_LOUDNESS_PREFERENCE_RENDERER"]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for rate in [44100, 48000, 96000, 192000]:
                first = qualify(params, rate, renderer, root / f"rate-{rate}", 193)
                second = qualify(params, rate, renderer, root / f"other-{rate}", 257)
                self.assertLess(first["max_complex_error"], 1e-5)
                self.assertEqual(
                    first["native_sampled_peak_db"], second["native_sampled_peak_db"]
                )
                self.assertEqual(first["tail_peak"], second["tail_peak"])


if __name__ == "__main__":
    unittest.main()
