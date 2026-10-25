"""Analytic metric conventions and paired measured-source loading checks."""

import io
import unittest
import zipfile

import numpy as np
from evaluate_ir_measured import load_pair
from ir_measured_metrics import decay_error, transfer_error
from scipy.io import savemat


class MeasuredIrTests(unittest.TestCase):
    def reference(self):
        result = np.zeros((5000, 2))
        result[100] = [1.0, 0.8]
        result[1000] = [0.2, -0.1]
        return result

    def test_identical_response_has_zero_errors(self):
        reference = self.reference()
        self.assertEqual(decay_error(reference, reference, 48000, 100)["rmse_db"], 0)
        error = transfer_error(reference, reference, 48000)
        self.assertLess(error["magnitude_rmse_db"], 1e-12)
        self.assertLess(error["phase_rmse_degrees"], 1e-12)

    def test_gain_error_is_not_fitted_away(self):
        reference = self.reference()
        error = transfer_error(reference, reference * 0.5, 48000)
        self.assertAlmostEqual(error["magnitude_rmse_db"], 6.020599913279624, places=10)
        self.assertLess(error["phase_rmse_degrees"], 1e-12)
        # The decay profile is explicitly a normalized shape metric; absolute
        # gain is measured by transfer and waveform metrics separately.
        self.assertEqual(
            decay_error(reference, reference * 0.5, 48000, 100)["rmse_db"], 0
        )

    def test_polarity_change_is_a_phase_error(self):
        reference = self.reference()
        error = transfer_error(reference, -reference, 48000)
        self.assertLess(error["magnitude_rmse_db"], 1e-12)
        self.assertAlmostEqual(error["phase_rmse_degrees"], 180.0, places=10)

    def test_loss_of_late_reflection_changes_energy_profile(self):
        reference = self.reference()
        changed = reference.copy()
        changed[1000] = 0
        error = decay_error(reference, changed, 48000, 100)
        self.assertGreater(error["max_abs_db"], 20)
        self.assertTrue(np.isfinite(error["rmse_db"]))

    def test_channel_order_and_source_rate_are_checked(self):
        def archive(rate):
            buffer = io.BytesIO()
            with zipfile.ZipFile(buffer, "w") as archive:
                for channel in [0, 1]:
                    mat = io.BytesIO()
                    savemat(
                        mat,
                        {
                            "h_air": np.arange(32) * (1 if channel == 0 else -0.25),
                            "air_info": {
                                "fs": rate,
                                "channel": channel,
                                "room": "booth",
                            },
                        },
                    )
                    archive.writestr(
                        f"AIR_1_4/air_binaural_booth_{channel}_0_1.mat", mat.getvalue()
                    )
            buffer.seek(0)
            return zipfile.ZipFile(buffer)

        with archive(48000) as source:
            pair, metadata = load_pair(source, "booth_0_0_1")
            np.testing.assert_array_equal(pair[:, 0], np.arange(32))
            np.testing.assert_array_equal(pair[:, 1], np.arange(32) * -0.25)
            self.assertEqual([row["channel"] for row in metadata], [0, 1])
        with archive(44100) as source, self.assertRaisesRegex(ValueError, "geometry"):
            load_pair(source, "booth_0_0_1")


if __name__ == "__main__":
    unittest.main()
