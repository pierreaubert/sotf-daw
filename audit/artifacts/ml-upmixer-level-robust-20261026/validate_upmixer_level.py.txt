"""Development validation for an excitation-independent v2 mask candidate.

Model-level gates: <=1e-3 maximum shared-band probability drift and <=0.05dB
conditional direct-retention drift across -40/0/+20dB gain. Full-chain normalized
waveform changes are separate observations, since native routing is nonlinear.
"""

import argparse
import hashlib
import json
import shutil
from pathlib import Path

import numpy as np
from evaluate_upmixer_v2 import diagnostics, powers
from train_upmixer_native import KINDS
from upmixer_mask_cases import fixture
from upmixer_mask_native import render
from upmixer_source_mask import analyze, predict, targets
from upmixer_source_mask_v2 import load, observations


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--training", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    training = json.loads((args.training / "report.json").read_text())
    theta = load(args.training / "model.json", training["model_sha256"])
    if training["training_profile"] != "level_robust_v2" or np.any(theta[40:48] != 0):
        raise ValueError("Expected an excitation-independent candidate")
    args.output.mkdir(parents=True, exist_ok=False)
    renderer = args.output / "renderer.frozen"
    model = args.output / "model.frozen.json"
    shutil.copy2(args.training / "renderer.frozen", renderer)
    shutil.copy2(args.training / "model.json", model)
    sha = training["model_sha256"]
    if digest(renderer) != training["renderer_sha256"]:
        raise ValueError("Renderer binding mismatch")
    sources = {}
    for name in [
        Path(__file__).name,
        "evaluate_upmixer_v2.py",
        "train_upmixer_native.py",
        "upmixer_source_mask.py",
        "upmixer_source_mask_v2.py",
        "upmixer_mask_cases.py",
        "upmixer_mask_native.py",
    ]:
        target = args.output / (name + ".txt")
        shutil.copy2(Path(__file__).parent / name, target)
        sources[name] = digest(target)
    cases, comparisons = [], []
    maximum_probability_drift = maximum_retention_drift = maximum_network_error = 0.0
    for seed in range(20306120, 20306124):
        for kind in KINDS:
            source, direct, ambient, identity = fixture(seed, kind)
            records = {}
            for level in [-40, 0, 20]:
                gain = 10 ** (level / 20)
                work = args.output / str(len(cases))
                audio, meta = render(
                    source * gain, renderer, work / "selected", model=model, sha=sha
                )
                native_audio, nmeta = render(source * gain, renderer, work / "native")
                features, eligible, _ = observations(meta)
                masks = np.array(
                    [
                        [m if m is not None else 0.0 for m in row["state"]["masks"]]
                        for row in meta["rows"]
                    ]
                )
                selected = np.array(
                    [
                        [m is not None for m in row["state"]["masks"]]
                        for row in meta["rows"]
                    ]
                )
                if (
                    not meta["model_loaded"]
                    or nmeta["model_loaded"]
                    or not np.array_equal(selected, eligible)
                ):
                    raise ValueError("Native model selection differs from contract")
                network_error = (
                    float(
                        abs(predict(theta, features[eligible]) - masks[eligible]).max()
                    )
                    if eligible.any()
                    else 0.0
                )
                maximum_network_error = max(maximum_network_error, network_error)
                sd, sa = (
                    analyze(np.asarray(stem * gain, dtype="<f4"))[: len(features)]
                    for stem in [direct, ambient]
                )
                metric = diagnostics(
                    masks, targets(sd, sa), powers(sd), powers(sa), eligible
                )
                records[level] = {
                    "masks": masks,
                    "eligible": eligible,
                    "metrics": metric,
                    "selected": audio.astype(float) / gain,
                    "native": native_audio.astype(float) / gain,
                }
                cases.append(
                    {
                        "identity": identity,
                        "level_db": level,
                        "files": meta["files"],
                        "native_files": nmeta["files"],
                        "metrics": metric,
                        "network_error": network_error,
                    }
                )
            base = records[0]
            for level in [-40, 20]:
                other = records[level]
                shared = base["eligible"] & other["eligible"]
                probability = (
                    float(abs(base["masks"][shared] - other["masks"][shared]).max())
                    if shared.any()
                    else 0.0
                )
                retention = [
                    r["metrics"]["conditional_spectral_direct_retention_db"]
                    for r in [base, other]
                ]
                retention_drift = (
                    abs(retention[0] - retention[1])
                    if all(r is not None for r in retention)
                    else None
                )
                maximum_probability_drift = max(maximum_probability_drift, probability)
                maximum_retention_drift = max(
                    maximum_retention_drift, retention_drift or 0.0
                )
                comparisons.append(
                    {
                        "seed": seed,
                        "kind": kind,
                        "level_db": level,
                        "shared_eligible_band_frames": int(shared.sum()),
                        "eligibility_changes": int(
                            np.count_nonzero(base["eligible"] != other["eligible"])
                        ),
                        "maximum_probability_drift": probability,
                        "conditional_direct_retention_drift_db": retention_drift,
                        "normalized_waveform_relative_rms_change": {
                            method: float(
                                np.sqrt(
                                    np.sum((base[method] - other[method]) ** 2)
                                    / max(np.sum(base[method] ** 2), 1e-30)
                                )
                            )
                            for method in ["selected", "native"]
                        },
                    }
                )
    report = {
        "scope": "development validation on four validation seeds; not a final holdout or listener acceptance",
        "training_report_sha256": digest(args.training / "report.json"),
        "renderer_sha256": digest(renderer),
        "model_sha256": sha,
        "source_sha256": sources,
        "probability_drift_tolerance": 1e-3,
        "conditional_retention_drift_tolerance_db": 0.05,
        "maximum_probability_drift": maximum_probability_drift,
        "maximum_conditional_retention_drift_db": maximum_retention_drift,
        "maximum_native_network_error": maximum_network_error,
        "development_gate_pass": bool(
            maximum_probability_drift <= 1e-3
            and maximum_retention_drift <= 0.05
            and maximum_network_error <= 2e-7
            and all(r["shared_eligible_band_frames"] > 0 for r in comparisons)
        ),
        "cases": cases,
        "comparisons": comparisons,
        "deployment_admitted": False,
    }
    if digest(model) != sha or digest(renderer) != training["renderer_sha256"]:
        raise ValueError("Frozen model/renderer changed")
    (args.output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    print(
        json.dumps(
            {
                k: report[k]
                for k in [
                    "development_gate_pass",
                    "maximum_probability_drift",
                    "maximum_conditional_retention_drift_db",
                    "maximum_native_network_error",
                ]
            }
        )
    )


if __name__ == "__main__":
    main()
