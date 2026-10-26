"""Run frozen matched PND probes sequentially with paced real-model workers."""

import argparse
import hashlib
import json
import shutil
import subprocess
from pathlib import Path

from pnd_compare_cases import KINDS, fixture


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--model-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--smoke", action="store_true")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    frozen = args.output / "renderer.frozen"
    shutil.copy2(args.renderer, frozen)
    renderer_sha = digest(frozen)
    model = args.output / "crepe-tiny.frozen.onnx"
    shutil.copy2(args.model_dir / "crepe-tiny.onnx", model)
    manifest = json.loads((args.model_dir / "manifest.json").read_text())
    if digest(model) != manifest["sha256"]:
        raise ValueError("Model manifest binding mismatch")
    sources = {}
    for name in [Path(__file__).name, "pnd_compare_cases.py"]:
        target = args.output / (name + ".txt")
        shutil.copy2(Path(__file__).parent / name, target)
        sources[name] = digest(target)
    cases = []
    seeds = [20308120] if args.smoke else [20308120, 20308121]
    kinds = ["offset"] if args.smoke else KINDS
    for seed in seeds:
        for kind in kinds:
            x, _, _, identity = fixture(seed, kind, seconds=0.25 if args.smoke else 3)
            root = args.output / str(len(cases))
            root.mkdir()
            (root / "input.f32le").write_bytes(x.tobytes())
            request = {
                "input": str((root / "input.f32le").resolve()),
                "classical_output": str((root / "classical.f32le").resolve()),
                "neural_output": str((root / "neural.f32le").resolve()),
                "report": str((root / "native.json").resolve()),
                "frames": len(x),
                "sample_rate": 48000,
                "reference_hz": identity["reference_hz"],
                "model": {
                    "model_path": str(model.resolve()),
                    "sha256": manifest["sha256"],
                    "min_confidence": 0.8,
                    "max_age_ms": 150.0,
                    "max_deviation_cents": 100.0,
                    "source_channel": 0,
                },
                "neural_first": bool(seed % 2),
            }
            path = root / "request.json"
            path.write_text(json.dumps(request, indent=2) + "\n")
            process = subprocess.run(
                [str(frozen.resolve()), str(path.resolve())],
                capture_output=True,
                text=True,
                timeout=120,
                check=False,
            )
            (root / "stderr.txt").write_text(process.stderr)
            if process.returncode:
                raise RuntimeError(f"Native PND case failed: {process.stderr}")
            cases.append(
                {
                    "identity": identity,
                    "neural_first": request["neural_first"],
                    "files": {
                        name: {
                            "path": str((root / name).resolve()),
                            "sha256": digest(root / name),
                        }
                        for name in [
                            "input.f32le",
                            "classical.f32le",
                            "neural.f32le",
                            "native.json",
                            "request.json",
                        ]
                    },
                }
            )
    if digest(frozen) != renderer_sha or digest(model) != manifest["sha256"]:
        raise ValueError("Frozen renderer or model changed")
    report = {
        "scope": "generated native matched estimator/worker comparison; not held-out music or a device/runtime winner",
        "smoke": args.smoke,
        "renderer_sha256": renderer_sha,
        "model_sha256": manifest["sha256"],
        "source_sha256": sources,
        "cases": cases,
        "deployment_admitted": False,
    }
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"cases": len(cases), "renderer_sha256": renderer_sha}))


if __name__ == "__main__":
    main()
