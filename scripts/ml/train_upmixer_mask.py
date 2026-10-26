"""Fit one frozen offline source-mask candidate, then measure disjoint stems."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from scipy.optimize import minimize
from upmixer_mask_cases import fixture
from upmixer_source_mask import (
    analyze,
    apply_mask,
    evidence,
    objective,
    save,
    select_masks,
    synthesize,
    targets,
)


def dataset(seeds):
    xs, ys, identities = [], [], []
    for seed in seeds:
        source, direct, ambient, identity = fixture(seed)
        x, _, valid = evidence(analyze(source))
        target = targets(analyze(direct), analyze(ambient))
        # Fixed deterministic thinning limits fitting cost; no random window split.
        keep = valid.copy()
        keep[1::2] = False
        xs.append(x[keep])
        ys.append(target[keep])
        identities.append(identity)
    return np.concatenate(xs), np.concatenate(ys), identities


def power(x):
    return float(np.mean(abs(x) ** 2))


def ratio_db(a, b):
    return None if b <= 1e-30 else float(10 * np.log10(max(a, 1e-30) / b))


def evaluate(theta, seeds, kind):
    records = []
    for seed in seeds:
        source, direct, ambient, identity = fixture(seed, kind)
        spectra, sd, sa = (analyze(v) for v in [source, direct, ambient])
        selected, baseline, valid = select_masks(theta, spectra)
        target = targets(sd, sa)
        methods = {}
        for name, mask in [
            ("learned", selected),
            ("analytic", baseline),
            ("oracle_band_power", target),
        ]:
            estimated = synthesize(apply_mask(spectra, mask), len(source))
            retained = synthesize(apply_mask(sd, mask), len(source))
            leaked = synthesize(apply_mask(sa, mask), len(source))
            residual = source - estimated
            methods[name] = {
                "valid_mask_mse": float(np.mean((mask[valid] - target[valid]) ** 2))
                if valid.any()
                else None,
                "direct_target_relative_rms_error": float(
                    np.sqrt(power(estimated - direct) / power(direct))
                )
                if power(direct) > 1e-30
                else None,
                "ambient_leakage_db": ratio_db(power(leaked), power(ambient)),
                "direct_retention_db": ratio_db(power(retained), power(direct)),
                "direct_only_relative_rms_damage": float(
                    np.sqrt(power(retained - direct) / power(direct))
                )
                if power(direct) > 1e-30
                else None,
                "mask_mean_abs_frame_difference": float(
                    np.mean(abs(np.diff(mask, axis=0)))
                ),
                "residual_plus_direct_max_error": float(
                    np.max(abs(estimated + residual - source))
                ),
                "output_sha256": hashlib.sha256(
                    estimated.astype("<f8").tobytes()
                ).hexdigest(),
            }
        records.append(
            {"identity": identity, "valid_rows": int(valid.sum()), "methods": methods}
        )
    return records


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    x, y, training = dataset(range(20295120, 20295152))
    vx, vy, validation = dataset(range(20296120, 20296128))
    rng = np.random.default_rng(20295119)
    fit = minimize(
        objective,
        rng.normal(0, 0.1, 65),
        args=(x, y),
        jac=True,
        method="L-BFGS-B",
        bounds=[(-16, 16)] * 65,
        options={"maxiter": 300, "ftol": 1e-10, "gtol": 1e-6},
    )
    sha = save(fit.x, args.output / "model.json")
    # Final source identities are not inspected until the sole candidate is frozen.
    held = {
        kind: evaluate(fit.x, range(20297120, 20297136), kind)
        for kind in [
            "matched",
            "hard_pan",
            "correlated_bed",
            "antiphase_bed",
            "direct_only",
            "ambient_only",
            "level_shift",
            "silence",
        ]
    }
    report = {
        "model_sha256": sha,
        "optimizer_success": bool(fit.success),
        "optimizer_message": str(fit.message),
        "iterations": int(fit.nit),
        "training": training,
        "validation": validation,
        "training_rows": len(x),
        "validation_rows": len(vx),
        "training_objective": objective(fit.x, x, y)[0],
        "validation_objective": objective(fit.x, vx, vy)[0],
        "held": held,
        "deployment_admitted": False,
        "scope": "offline Python source-mask reference, mono source panned into stereo plus generated bed; no native latency or recorded/listening acceptance",
    }
    (args.output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    print(
        json.dumps(
            {
                "model_sha256": sha,
                "optimizer_success": bool(fit.success),
                "final_cases": sum(map(len, held.values())),
            }
        )
    )


if __name__ == "__main__":
    main()
