"""Guard deadline arithmetic and avoid treating model fallback as runtime success."""

import copy
import json
import unittest
from pathlib import Path

from analyze_runtime_contention import analyze


def fixture():
    path = (
        Path(__file__).resolve().parents[2]
        / "audit/artifacts/ml-runtime-contention-20261026/smoke-both-r2.json"
    )
    r = json.loads(path.read_text())
    return {
        "binary_sha256": "recorded native smoke",
        "environment": {},
        "cases": [{"case": 0, "exit_code": 0, "request": r["request"], "report": r}],
    }


class RuntimeAnalysisTests(unittest.TestCase):
    def test_recorded_trace_recomputes_native_summaries(self):
        report = fixture()
        result = analyze(report)["cases"][0]
        self.assertEqual(result["measured_callbacks"], 150)
        self.assertEqual(
            result["execution_overruns"],
            report["cases"][0]["report"]["execution_overruns"],
        )
        self.assertFalse(analyze(report)["runtime_choice_admitted"])

    def test_hidden_misses_or_wrong_time_units_are_rejected(self):
        original = fixture()
        changed = copy.deepcopy(original)
        changed["cases"][0]["report"]["response_deadline_misses"] += 1
        with self.assertRaises(ValueError):
            analyze(changed)
        changed = copy.deepcopy(original)
        changed["cases"][0]["report"]["trace"][0]["deadline_ns"] *= 1000
        with self.assertRaises(ValueError):
            analyze(changed)

    def test_timely_audio_can_still_have_no_served_neural_models(self):
        changed = fixture()
        r = changed["cases"][0]["report"]
        r["pitch_neural_selected_snapshots"] = [0]
        r["telemetry_after"][0]["deep_filter"]["timely_packets"] = r[
            "telemetry_before_measurement"
        ][0]["deep_filter"]["timely_packets"]
        result = analyze(changed)["cases"][0]
        self.assertEqual(result["execution_overruns"], 0)
        self.assertFalse(result["all_requested_models_observed_serving"])

    def test_worker_counter_regression_is_rejected(self):
        changed = fixture()
        changed["cases"][0]["report"]["telemetry_after"][0]["deep_filter"][
            "dropped_inputs"
        ] = -1
        with self.assertRaises(ValueError):
            analyze(changed)


if __name__ == "__main__":
    unittest.main()
