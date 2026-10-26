"""Verify frozen expander waveforms survive shared evaluator extraction exactly."""

import argparse
import hashlib
import json
import subprocess
from pathlib import Path


def identity(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    original = json.loads(args.report.read_text())
    args.output.mkdir(parents=True, exist_ok=False)
    renderer_hash = identity(args.renderer)
    records = []
    for case in original["cases"]:
        for method, entry in case["methods"].items():
            expected = entry["metadata"]
            files = expected["files"]
            for file in files.values():
                if identity(file["path"]) != file["sha256"]:
                    raise ValueError("Frozen waveform/trace input identity changed")
            request = json.loads(
                (Path(files["input"]["path"]).parent / "request.json").read_text()
            )
            if (
                request["parameters"] != expected["parameters"]
                or request["model"]["sha256"] != original["model_sha256"]
                or identity(request["model"]["path"]) != original["model_sha256"]
            ):
                raise ValueError(
                    "Frozen native request/model no longer matches evaluation"
                )
            work = args.output / case["case"] / method
            work.mkdir(parents=True)
            request.update(
                input=files["input"]["path"],
                output=str((work / "output.f32le").resolve()),
                gain_trace_output=str((work / "gains.f32le").resolve()),
            )
            path = work / "request.json"
            path.write_text(json.dumps(request, indent=2) + "\n")
            result = subprocess.run(
                [str(args.renderer.resolve()), str(path.resolve())],
                capture_output=True,
                text=True,
                timeout=120,
                check=False,
            )
            (work / "stderr.txt").write_text(result.stderr)
            if result.returncode:
                raise RuntimeError(result.stderr)
            actual = json.loads(result.stdout)
            for key, value in actual.items():
                if expected[key] != value:
                    raise RuntimeError(
                        f"Shared extraction changed native metadata {key}"
                    )
            for key in ["output", "gain_trace_output"]:
                if (
                    Path(request[key]).read_bytes()
                    != Path(files[key]["path"]).read_bytes()
                ):
                    raise RuntimeError("Shared extraction changed frozen native bytes")
            (work / "metadata.json").write_text(json.dumps(actual, indent=2) + "\n")
            records.append(
                {
                    "case": case["case"],
                    "method": method,
                    "output_sha256": identity(request["output"]),
                    "gains_sha256": identity(request["gain_trace_output"]),
                    "metadata_identical": True,
                    "output_and_trace_byte_identical": True,
                }
            )
    if identity(args.renderer) != renderer_hash:
        raise RuntimeError("Replay renderer changed during consumption")
    report = {
        "original_report_sha256": identity(args.report),
        "original_renderer_sha256": original["renderer_sha256"],
        "new_renderer_sha256": renderer_hash,
        "records": records,
        "deployment_admitted": False,
        "scope": "Exact compatibility replay of all frozen native expander outputs and per-hop gains after shared evaluator extraction. No new quality or admission claim.",
    }
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"outputs": len(records), "all_byte_identical": True}))


if __name__ == "__main__":
    main()
