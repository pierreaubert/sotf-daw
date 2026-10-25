"""Freeze fresh procedural scene classification, shifts, native proposals and counterfactuals."""

import argparse
import json
from pathlib import Path

import numpy as np
from stereo_scene_cases import CONTEXT, session, signal
from stereo_scenes import digest, features, fit, propose


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    if args.report.exists():
        raise ValueError("Refuse report overwrite")
    args.work.mkdir(parents=True, exist_ok=False)
    data = session(args.work / "training", args.renderer, seed=20931026)
    (args.work / "session.json").write_text(json.dumps(data, indent=2) + "\n")
    fitted = fit(data)
    model = fitted.pop("model")
    if model is None:
        raise ValueError("Fresh training admission failed: " + str(fitted["reasons"]))
    raw = (json.dumps(model, indent=2, allow_nan=False) + "\n").encode()
    (args.work / "model.json").write_bytes(raw)
    (args.work / "fit-report.json").write_text(
        json.dumps(fitted, indent=2, allow_nan=False) + "\n"
    )
    (args.work / "context.json").write_text(json.dumps(CONTEXT) + "\n")
    rows = []
    main = model["parameters"]["main"]
    centers = np.asarray(main["means"])[:, 0] * main["scale"][0] + main["center"][0]
    for source in range(8):
        for index, scene in enumerate(model["class_names"]):
            original = signal(scene, 20941026 + 100 * source + index, seconds=1.003)
            for gain in [1.0, 0.125, 8.0]:
                x = (original * gain).astype(np.float32)
                name = f"held-{source}-{scene}-{gain}"
                raw = x.astype("<f4").tobytes()
                path = args.work / (name + ".f32le")
                path.write_bytes(raw)
                value = propose(x, CONTEXT, model, args.renderer, args.work / name)
                f = features(x, CONTEXT)
                rows.append(
                    {
                        "id": name,
                        "source_group": f"held-{source}",
                        "label": scene,
                        "gain": gain,
                        "audio_file": str(path.resolve()),
                        "audio_sha256": digest(raw),
                        "frames": len(x),
                        "context": dict(CONTEXT),
                        "baseline_scene": model["class_names"][
                            int(np.argmin(abs(centers - f[0])))
                        ],
                        "proposal": value,
                    }
                )
    original = signal("side_dominant", 20951026)
    special = {
        "silent": np.zeros_like(original),
        "one_silent": np.column_stack([original[:, 0], np.zeros(len(original))]),
        "antiphase": np.column_stack([original[:, 0], -original[:, 0]]),
        "desired_wide_effect": original,
        "unwanted_side_spill": original,
    }
    for name, x in special.items():
        raw = x.astype("<f4").tobytes()
        path = args.work / (name + ".f32le")
        path.write_bytes(raw)
        value = propose(x, CONTEXT, model, args.renderer, args.work / name)
        rows.append(
            {
                "id": name,
                "source_group": name
                if name not in {"desired_wide_effect", "unwanted_side_spill"}
                else "same-pcm-different-intent",
                "label": None,
                "audio_file": str(path.resolve()),
                "audio_sha256": digest(raw),
                "frames": len(x),
                "context": dict(CONTEXT),
                "proposal": value,
            }
        )
    args.report.write_text(
        json.dumps(
            {
                "training_session": str((args.work / "session.json").resolve()),
                "model_file": str((args.work / "model.json").resolve()),
                "model_sha256": digest((args.work / "model.json").read_bytes()),
                "fit_report": str((args.work / "fit-report.json").resolve()),
                "cases": rows,
                "qualification": "24 fresh procedural source/scene examples with three correlated gain variants, plus structural and intended-use probes. No recorded-scene, listening or preferred-width acceptance.",
            },
            indent=2,
            allow_nan=False,
        )
        + "\n"
    )


if __name__ == "__main__":
    main()
