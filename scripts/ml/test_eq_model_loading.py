"""Model substitution, rejection, and legacy inference parity checks."""

import hashlib
import io
import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import eq_assistant
import eq_multiband
import torch
from eq_model_loading import load_model, read_bounded_bytes

ROOT = Path(__file__).resolve().parents[2] / "crates/sotf-plugins/models"
CASES = [
    (eq_assistant, "eq-single-peak", "sotf-single-peak-v1"),
    (eq_multiband, "eq-multiband", eq_multiband.CONTRACT),
]


class ModelLoadingTests(unittest.TestCase):
    def test_committed_models_have_identical_parameters_and_predictions(self):
        for module, directory, contract in CASES:
            with self.subTest(model=directory):
                path = ROOT / directory / "weights.json"
                raw = path.read_bytes()
                previous = module.network()
                previous.load_state_dict(
                    {
                        k: torch.as_tensor(v)
                        for k, v in json.loads(raw)["weights"].items()
                    }
                )
                previous.eval()
                current, digest = load_model(path, contract, module.network)
                self.assertEqual(digest, hashlib.sha256(raw).hexdigest())
                for key, value in previous.state_dict().items():
                    self.assertTrue(torch.equal(value, current.state_dict()[key]))
                width = next(iter(previous.state_dict().values())).shape[1]
                inputs = torch.linspace(-1, 1, 3 * width).reshape(3, width)
                with torch.no_grad():
                    self.assertTrue(torch.equal(previous(inputs), current(inputs)))

    def test_replacing_path_after_read_cannot_substitute_weights(self):
        module, directory, contract = CASES[0]
        original = (ROOT / directory / "weights.json").read_bytes()
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "weights.json"
            path.write_bytes(original)
            (path.parent / "training-report.json").write_text(
                json.dumps({"weights_sha256": hashlib.sha256(original).hexdigest()})
            )
            real_open = Path.open

            class ReplacingReader(io.BytesIO):
                def read(self, size=-1):
                    data = super().read(size)
                    path.write_bytes(b"replacement")
                    return data

            def opening(candidate, mode="r", *args, **kwargs):
                if candidate == path and mode == "rb":
                    return ReplacingReader(original)
                return real_open(candidate, mode, *args, **kwargs)

            with patch.object(Path, "open", opening):
                model, digest = load_model(path, contract, module.network)
            self.assertEqual(path.read_bytes(), b"replacement")
            self.assertEqual(digest, hashlib.sha256(original).hexdigest())
            for key, value in json.loads(original)["weights"].items():
                self.assertTrue(
                    torch.equal(model.state_dict()[key], torch.tensor(value))
                )

    def test_malformed_models_rejected_even_with_matching_hash(self):
        for module, directory, contract in CASES:
            original = json.loads((ROOT / directory / "weights.json").read_bytes())
            key = next(iter(original["weights"]))
            bad = [[], {"contract": "wrong"}, {"contract": contract, "weights": {}}]
            for value in ([1], [[float("nan")]], [[1e300]]):
                changed = json.loads(json.dumps(original))
                changed["weights"][key] = value
                bad.append(changed)
            for number in [float("nan"), float("inf"), 1e300]:
                changed = json.loads(json.dumps(original))
                changed["weights"][key][0][0] = number
                bad.append(changed)
            with tempfile.TemporaryDirectory() as temp:
                path = Path(temp) / "weights.json"
                for document in bad:
                    raw = json.dumps(document).encode()
                    path.write_bytes(raw)
                    (path.parent / "training-report.json").write_text(
                        json.dumps({"weights_sha256": hashlib.sha256(raw).hexdigest()})
                    )
                    with (
                        self.subTest(model=directory, document=str(document)[:60]),
                        self.assertRaises(ValueError),
                    ):
                        load_model(path, contract, module.network)
                path.write_bytes(b"substitution")
                with self.assertRaisesRegex(ValueError, "SHA-256"):
                    load_model(path, contract, module.network)

    def test_failed_model_load_cannot_export_a_proposal(self):
        for module, _, _ in CASES:
            with tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                target = root / "target.json"
                target.write_text(
                    json.dumps(
                        {
                            "sample_rate": 48000,
                            "frequencies_hz": module.GRID.tolist(),
                            "target_db": [1.0] * len(module.GRID),
                        }
                    )
                )
                weights = root / "weights.json"
                weights.write_text("{}")
                (root / "training-report.json").write_text(
                    json.dumps({"weights_sha256": "0" * 64})
                )
                args = SimpleNamespace(
                    target=target, weights=weights, output=root / "proposal.json"
                )
                with self.assertRaisesRegex(ValueError, "SHA-256"):
                    module.fit(args)
                self.assertFalse(args.output.exists())

    def test_bounded_reader_stops_at_limit_plus_one(self):
        class CountingReader(io.BytesIO):
            count = 0

            def read(self, size=-1):
                result = super().read(size)
                self.count += len(result)
                return result

        source = CountingReader(b"x" * 200)
        with patch.object(Path, "open", return_value=source):
            with self.assertRaisesRegex(ValueError, "size limit"):
                read_bounded_bytes(Path("oversized"), 100)
            self.assertEqual(source.count, 101)


if __name__ == "__main__":
    unittest.main()
