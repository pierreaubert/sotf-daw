"""Check refinement-label supervision without removing failed/hard targets."""

import unittest
from unittest.mock import patch

import eq_multiband as eq
import numpy as np
import train_eq_refined as experiment
from evaluate_eq_refined import independent_response


class RefinementTrainingTests(unittest.TestCase):
    def test_teacher_keeps_all_examples_and_uses_only_improved_bounded_labels(self):
        split = eq.dataset(3, 20270208)
        initial = split[0].copy()
        initial[:, 2] += 0.1
        outcomes = [
            (split[0][0], {"mse_db2": 0.0}),
            (split[0][1], {"mse_db2": 100.0}),
            (np.full(16, np.nan), {"mse_db2": 0.0}),
        ]
        with (
            patch.object(eq, "evaluate", return_value=(0, initial)),
            patch.object(experiment, "refine", side_effect=outcomes),
        ):
            labels, rows = experiment.teacher_labels(None, split, 10)
        self.assertEqual(len(labels), len(split[0]))
        self.assertEqual([r["improved_label"] for r in rows], [True, False, False])
        np.testing.assert_array_equal(labels[0], split[0][0])
        np.testing.assert_array_equal(labels[1:], initial[1:])

    def test_inactive_coordinates_do_not_supervise_unidentifiable_frequencies(self):
        parameters = np.tile(eq.canonical_parameters(), (2, 1))
        parameters[1, 2] = 0.02
        mask = experiment.label_mask(parameters)
        self.assertEqual(mask[0, :2].tolist(), [0, 0])
        self.assertEqual(mask[1, :2].tolist(), [1, 1])
        self.assertEqual(mask[:, 2].tolist(), [1, 1])
        self.assertEqual(mask[:, 12].tolist(), [0, 0])

    def test_development_selection_prioritizes_qualification_over_mse_and_cost(self):
        good = {
            "sampled_solver_qualified": 2,
            "mean_refined_mse_db2": 1,
            "median_residual_calls": 100,
        }
        bad = {
            "sampled_solver_qualified": 1,
            "mean_refined_mse_db2": 0,
            "median_residual_calls": 1,
        }
        self.assertLess(experiment.selection_key(good), experiment.selection_key(bad))

    def test_final_export_checker_matches_independent_bilinear_prototypes(self):
        parameters, _, _, _ = eq.dataset(8, 20270209)
        for rate in [44100, 48000, 96000, 192000]:
            for p in parameters:
                filters = eq.proposal_parameters(p)["filters"]
                actual = independent_response(filters, eq.GRID, rate)
                np.testing.assert_allclose(
                    actual, eq.response(p, rate=rate), atol=1e-6, rtol=0
                )

    def test_exclusions_collect_nested_prior_split_and_evaluation_identities(self):
        document = {
            "split_ids": {"train": ["train"]},
            "rows": [{"parameter_identity": "native"}],
            "fresh_holdout_ids": ["holdout"],
            "unique_parameter_identities": ["probe"],
            "irrelevant": "ignore",
        }
        self.assertEqual(
            experiment.collect_ids(document), {"train", "native", "holdout", "probe"}
        )


if __name__ == "__main__":
    unittest.main()
