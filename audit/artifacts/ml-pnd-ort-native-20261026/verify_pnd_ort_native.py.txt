"""Verify actual-library parity matrix, native delegation and resource identities."""

import argparse
import hashlib
import json
from pathlib import Path


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def verify(root, library, providers):
    report = json.loads((root / "qualified-parity.json").read_text())
    tests = json.loads((root / "qualified-tests.json").read_text())
    if (
        tests["exit_code"] != 0
        or "actual_runtime_parity_workers_fallback_and_transactional_lifecycle ... ok"
        not in tests["stdout"]
    ):
        raise ValueError("Actual-library test did not complete successfully")
    if digest(library) != report["library_sha256"]:
        raise ValueError("Native library identity changed")
    expected = {
        (p, r, h, w)
        for p in providers
        for r in [16000, 44100, 48000, 96000, 192000]
        for h in [82.41, 220.0, 440.0, 880.0, 1318.51]
        for w in [False, True]
    }
    actual = [(x["provider"], x["rate"], x["hz"], x["weak"]) for x in report["rows"]]
    if len(actual) != len(expected) or set(actual) != expected:
        raise ValueError("Incomplete or duplicated source-rate parity matrix")
    pitch = max(x["pitch_difference_cents"] for x in report["rows"])
    confidence = max(x["confidence_difference"] for x in report["rows"])
    if (
        pitch != report["maximum_pitch_difference_cents"]
        or confidence != report["maximum_confidence_difference"]
        or not 0 <= pitch <= 0.1
        or not 0 <= confidence <= 2e-5
    ):
        raise ValueError("Source-rate numerical parity failed")
    if (
        report["activation_frames_per_provider"] != 32
        or not 0 <= report["maximum_activation_difference"] <= 2e-5
    ):
        raise ValueError("Activation contract test failed")
    placement = {}
    for row in report["placement"]:
        if row["provider"] in placement:
            raise ValueError("Duplicated provider profile")
        if digest(row["path"]) != row["sha256"]:
            raise ValueError("Native profile hash mismatch")
        counts = {}
        for event in json.loads(Path(row["path"]).read_text()):
            name = event.get("args", {}).get("provider")
            if event.get("cat") == "Node" and name:
                counts[name] = counts.get(name, 0) + 1
        expected_provider = (
            "CPUExecutionProvider"
            if row["provider"] == "cpu"
            else "CoreMLExecutionProvider"
        )
        if (
            counts != row["node_kernel_events_by_provider"]
            or counts.get(expected_provider, 0) < 1
        ):
            raise ValueError("Native provider delegation not observed")
        placement[row["provider"]] = counts
    if set(placement) != set(providers):
        raise ValueError("Missing native provider profile")
    repo = Path(__file__).resolve().parents[2]
    corpus = repo / "audit/artifacts/ml-crepe-runtime-20261026/corpus.f32le"
    model = repo / "crates/sotf-plugins/models/crepe-tiny/crepe-tiny.onnx"
    if (
        digest(corpus)
        != "b187c69ba92f255921abca3cbb6f5f42955da7a96b4fe7984b4b262255cc23f8"
        or digest(model)
        != "57209bf565bce9de445b31a175294dd2f297432864599c6aa237a895353918d3"
    ):
        raise ValueError("Model or activation corpus changed")
    return {
        "source_window_rows": len(actual),
        "activation_frames_per_provider": 32,
        "maximum_activation_difference": report["maximum_activation_difference"],
        "maximum_pitch_difference_cents": pitch,
        "maximum_confidence_difference": confidence,
        "native_provider_events": placement,
        "library_sha256": report["library_sha256"],
        "qualified_report_sha256": digest(root / "qualified-parity.json"),
        "test_log_sha256": digest(root / "qualified-tests.json"),
        "scope": "test-log and recorded matrix/resource/profile verification; numerical forward assertions execute in the native Rust test",
        "deployment_admitted": False,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--library", type=Path, required=True)
    parser.add_argument(
        "--providers", nargs="+", choices=["cpu", "core_ml_cpu_only"], required=True
    )
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = verify(args.root, args.library, args.providers)
    with args.output.open("x") as stream:
        stream.write(json.dumps(result, indent=2, allow_nan=False) + "\n")
    print(json.dumps(result))


if __name__ == "__main__":
    main()
