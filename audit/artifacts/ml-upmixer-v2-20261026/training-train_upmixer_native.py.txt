"""Fit a v2 candidate on native features; final evaluation is a separate command."""

import argparse
import hashlib
import json
import shutil
from pathlib import Path

import numpy as np
from scipy.optimize import minimize
from upmixer_mask_cases import fixture
from upmixer_mask_native import render
from upmixer_source_mask import analyze, objective, targets
from upmixer_source_mask_v2 import observations, save

KINDS = ("matched", "hard_pan", "direct_only", "ambient_only")


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def dataset(seeds, renderer, model, sha, output):
    xs, ys, cases = [], [], []
    for seed in seeds:
        for kind in KINDS:
            source, direct, ambient, identity = fixture(seed, kind)
            _, meta = render(
                source, renderer, output / str(len(cases)), model=model, sha=sha
            )
            if not meta["model_loaded"]:
                raise RuntimeError("Native v2 observation model not loaded")
            features, eligible, _ = observations(meta)
            sd, sa = (
                analyze(np.asarray(stem, dtype="<f4")) for stem in [direct, ambient]
            )
            label = targets(sd, sa)[: len(features)]
            keep = eligible.copy()
            keep[1::2] = False
            xs.append(features[keep])
            ys.append(label[keep])
            cases.append(
                {
                    "identity": identity,
                    "selected_rows_after_thinning": int(keep.sum()),
                    "files": meta["files"],
                    "quantized_stem_sha256": {
                        name: hashlib.sha256(
                            np.asarray(stem, dtype="<f4").tobytes()
                        ).hexdigest()
                        for name, stem in [("direct", direct), ("ambient", ambient)]
                    },
                }
            )
    return np.concatenate(xs), np.concatenate(ys), cases


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    renderer = args.output / "renderer.frozen"
    shutil.copy2(args.renderer, renderer)
    renderer_sha = digest(renderer)
    source_sha = {}
    for name in [
        Path(__file__).name,
        "upmixer_source_mask.py",
        "upmixer_source_mask_v2.py",
        "upmixer_mask_cases.py",
        "upmixer_mask_native.py",
    ]:
        target = args.output / (name + ".txt")
        shutil.copy2(Path(__file__).parent / name, target)
        source_sha[name] = digest(target)
    bootstrap = args.output / "untrained-observation-model.json"
    sha = save(np.zeros(65), bootstrap)
    x, y, training = dataset(
        range(20302120, 20302136), renderer, bootstrap, sha, args.output / "training"
    )
    vx, vy, validation = dataset(
        range(20303120, 20303124), renderer, bootstrap, sha, args.output / "validation"
    )
    fit = minimize(
        objective,
        np.random.default_rng(20302119).normal(0, 0.1, 65),
        args=(x, y),
        jac=True,
        method="L-BFGS-B",
        bounds=[(-16, 16)] * 65,
        options={"maxiter": 1000, "ftol": 1e-10, "gtol": 1e-6},
    )
    model_sha = save(fit.x, args.output / "model.json")
    report = {
        "scope": "v2 native-feature training only; four generated conditions, no final cases inspected",
        "renderer_sha256": renderer_sha,
        "source_sha256": source_sha,
        "untrained_observation_model_sha256": sha,
        "model_sha256": model_sha,
        "training": training,
        "validation": validation,
        "training_rows": len(x),
        "validation_rows": len(vx),
        "training_objective": objective(fit.x, x, y)[0],
        "validation_objective": objective(fit.x, vx, vy)[0],
        "optimizer_success": bool(fit.success),
        "optimizer_message": str(fit.message),
        "iterations": int(fit.nit),
        "initializations": 1,
        "selection": "sole candidate; no validation search or final-data selection",
        "label_convention": "independent f64 FFT of separately quantized generated stems; target power fraction; mixture quantized separately",
        "deployment_admitted": False,
    }
    if digest(renderer) != renderer_sha or digest(bootstrap) != sha:
        raise RuntimeError("Frozen renderer/observation model changed during fitting")
    (args.output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    print(
        json.dumps(
            {
                k: report[k]
                for k in [
                    "model_sha256",
                    "training_rows",
                    "validation_rows",
                    "optimizer_success",
                    "iterations",
                ]
            }
        )
    )


if __name__ == "__main__":
    main()
