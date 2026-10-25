"""Fresh native hybrid/classical comparison after initial prototype development.

The policy and weights are frozen before these scenes are generated. This is
synthetic offline quality evidence; the realtime worker has separate lifecycle
tests and still needs device/domain validation.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np
from train_aec_residual import SCENES, fixture


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def score(target, error):
    return float(
        10
        * np.log10(
            max(float(np.dot(target, target)), 1e-30)
            / max(float(np.dot(error, error)), 1e-30)
        )
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--model-sha256", required=True)
    parser.add_argument("--previous-corpus-manifest", type=Path, required=True)
    parser.add_argument("--temporary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists() or args.temporary.exists():
        raise ValueError("preserve previous evaluation outputs")
    if sha(args.model) != args.model_sha256:
        raise ValueError("model hash mismatch")
    args.output.mkdir(parents=True)
    args.temporary.mkdir(parents=True)
    previous = json.loads(args.previous_corpus_manifest.read_text())
    previous_hashes = {
        row["input_sha256"] for group in previous["splits"].values() for row in group
    }
    config = {
        "model_path": str(args.model.resolve()),
        "sha256": args.model_sha256,
        "max_age_ms": 40.0,
        "max_extra_attenuation_db": 6.0,
    }
    protocol = {
        "seed": 20274025,
        "cases": 24,
        "rates": [16000, 44100, 48000],
        "model_sha256": args.model_sha256,
        "renderer_sha256": sha(args.renderer),
        "structural_config": config,
        "processing_mode": "offline",
        "selection": "Fixed policy developed after original prototype; new final scenes, no parameter/model tuning from these results.",
        "source_hashes_disjoint_from_initial_64": True,
        "extra_policy": "Keep classical post-filter; cap extra cut; near-end/ref-silent/missing/stale decisions retain classical.",
        "source_sha256": {
            name: sha(Path(__file__).parent / name)
            for name in [Path(__file__).name, "train_aec_residual.py"]
        },
    }
    (args.output / "protocol.json").write_text(json.dumps(protocol, indent=2) + "\n")
    worker = subprocess.Popen(
        [str(args.renderer.resolve()), str((args.temporary / "native").resolve())],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
    )
    rows, seen = [], set()
    try:
        for index in range(24):
            rate = protocol["rates"][index // len(SCENES)]
            scene = SCENES[index % len(SCENES)]
            input_array, near, echo, provenance = fixture(
                rate, scene, protocol["seed"] + index
            )
            path = args.temporary / f"source_{index:03d}"
            path.mkdir()
            for name, array in [("input", input_array), ("near", near), ("echo", echo)]:
                (path / (name + ".f32le")).write_bytes(array.tobytes())
                provenance[name + "_sha256"] = sha(path / (name + ".f32le"))
            identity = provenance["input_sha256"]
            if identity in previous_hashes or identity in seen:
                raise ValueError("overlap in fresh final targets")
            seen.add(identity)
            row = {
                "index": index,
                "provenance": provenance,
                "native": {},
                "quality": {},
            }
            audio = {}
            # Alternate order to avoid always measuring one after the other.
            for method in (
                ["classical", "hybrid"] if index % 2 == 0 else ["hybrid", "classical"]
            ):
                request = {
                    "id": f"{method}_{index:03d}",
                    "sample_rate": rate,
                    "frames": len(near),
                    "input": str((path / "input.f32le").resolve()),
                    "near": str((path / "near.f32le").resolve()),
                    "offline": True,
                }
                if method == "hybrid":
                    request["residual_suppression"] = config
                worker.stdin.write(json.dumps(request) + "\n")
                worker.stdin.flush()
                metadata = json.loads(worker.stdout.readline())
                if metadata.get("status") != "ok" or metadata["output_stage"] != method:
                    raise ValueError(f"unexpected native result: {metadata}")
                if method == "hybrid" and (
                    metadata["residual"]["availability"] != "prepared"
                    or not metadata["residual"]["offline"]
                    or metadata["residual"]["accepted_observations"] == 0
                ):
                    raise ValueError(
                        "requested learned stage was not actually exercised"
                    )
                output = Path(metadata["directory"]) / (method + ".f32le")
                if sha(output) != metadata[method]["sha256"]:
                    raise ValueError("native audio hash mismatch")
                audio[method] = np.fromfile(output, dtype="<f4").astype(float)
                if (
                    len(audio[method]) != len(near)
                    or not np.isfinite(audio[method]).all()
                ):
                    raise ValueError("native output geometry/nonfinite error")
                row["native"][method] = metadata
            # Any changed adaptation/reference/error observations would invalidate
            # this comparison: ML is downstream only, never an adaptive update.
            for name in ["adaptive", "error_spectra", "features", "targets"]:
                if (
                    row["native"]["classical"][name]["sha256"]
                    != row["native"]["hybrid"][name]["sha256"]
                ):
                    raise ValueError(
                        "learned stage modified adaptive/classical analysis state"
                    )
            row["max_output_difference"] = float(
                np.max(np.abs(audio["hybrid"] - audio["classical"]))
            )
            count = len(near)
            intervals = {
                "steady": (5 * rate, count),
                "onset": (3 * rate, 3 * rate + rate // 8),
            }
            if scene in ["double_talk", "quiet_double_talk"]:
                intervals["double_talk"] = (13 * rate // 4, 17 * rate // 4)
            elif scene == "near_only":
                intervals = {"near_only": (0, count)}
            elif scene == "near_after_echo":
                intervals["near_after_echo"] = (13 * rate // 4, count)
            near_double, echo_double = near.astype(float), echo.astype(float)
            for phase, (start, end) in intervals.items():
                target = near_double[start:end]
                has_near = np.dot(target, target) > 1e-20
                scores = {}
                for method, samples in audio.items():
                    output = samples[start:end]
                    scores[method] = (
                        score(target, output - target)
                        if has_near
                        else score(echo_double[start:end], output)
                    )
                row["quality"][phase] = {
                    "metric": "near_snr_db" if has_near else "erle_db",
                    **scores,
                    "hybrid_minus_classical_db": scores["hybrid"] - scores["classical"],
                }
            if scene == "near_only" and row["max_output_difference"] != 0:
                raise ValueError(
                    "reference-silent near-only path must be exactly classical"
                )
            (args.output / f"case-{index:03d}.json").write_text(
                json.dumps(row, indent=2, allow_nan=False) + "\n"
            )
            rows.append(row)
            print(
                json.dumps(
                    {
                        "case": index,
                        "scene": scene,
                        "rate": rate,
                        "applied_blocks": row["native"]["hybrid"]["residual"][
                            "applied_blocks"
                        ],
                        "quality": row["quality"],
                    }
                ),
                flush=True,
            )
    finally:
        worker.stdin.close()
        worker.wait(timeout=30)
    if worker.returncode:
        raise ValueError("native evaluation worker failed")
    summary = {
        "cases": len(rows),
        "source_hashes_disjoint": len(seen) == len(rows),
        "adaptive_analysis_hashes_identical": True,
        "scope": "Fresh synthetic native offline pipeline; no natural speech/device acceptance or realtime quality claim.",
        "applied_blocks": sum(
            row["native"]["hybrid"]["residual"]["applied_blocks"] for row in rows
        ),
        "rows": [
            {
                "index": row["index"],
                "scene": row["provenance"]["scene"],
                "rate": row["provenance"]["rate"],
                "quality": row["quality"],
            }
            for row in rows
        ],
    }
    (args.output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps({k: v for k, v in summary.items() if k != "rows"}), flush=True)


if __name__ == "__main__":
    main()
