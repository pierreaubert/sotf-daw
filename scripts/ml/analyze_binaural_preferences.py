#!/usr/bin/env python3
"""Independent native audition accounting and BFGS preference reconstruction."""

import argparse
import hashlib
import json
import math
import subprocess
import sys
import types
from pathlib import Path

import numpy as np
from scipy.optimize import minimize
from scipy.special import expit


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def solve(design, labels, weights, ridge):
    weights = weights / weights.sum()

    def objective(theta):
        z = design @ theta
        return float(
            weights @ (np.logaddexp(0, z) - labels * z)
            + ridge * np.dot(theta, theta) / 2
        )

    def gradient(theta):
        return design.T @ (weights * (expit(design @ theta) - labels)) + ridge * theta

    result = minimize(
        objective,
        np.zeros(design.shape[1]),
        jac=gradient,
        method="BFGS",
        options={"gtol": 1e-10, "maxiter": 500},
    )
    if np.linalg.norm(gradient(result.x)) > 1e-7:
        raise AssertionError("Independent optimizer failed")
    return result.x


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evaluation", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    cases = json.loads(args.evaluation.read_text())["cases"]
    max_metric = max_theta = max_bootstrap = max_reopen = 0.0
    auditions = bootstrap_fits = 0
    for case in cases:
        data = json.loads(Path(case["session"]).read_text())
        report = json.loads(Path(case["report"]).read_text())
        ids = [c["id"] for c in data["candidates"]]
        design = []
        labels = []
        groups = []
        for t in data["trials"]:
            row = [0.0] * (len(ids) + 1)
            row[ids.index(t["a"])] = 1
            row[ids.index(t["b"])] = -1
            row[-1] = 1
            design.append(row)
            labels.append({"a": 1.0, "b": 0.0, "tie": 0.5}[t["choice"]])
            groups.append(t["programme_id"])
        x = np.asarray(design)
        y = np.asarray(labels)
        unique = sorted(set(groups))
        index = np.array([unique.index(g) for g in groups])
        counts = np.bincount(index)
        weights = 1 / counts[index]
        ridge = report["policy"]["ridge"]
        theta = solve(x, y, weights, ridge)
        expected = np.array(
            report["model"]["utilities"] + [report["model"]["presentation_side_bias"]]
        )
        max_theta = max(max_theta, float(np.max(abs(theta - expected))))
        rng = np.random.default_rng(report["policy"]["bootstrap_seed"])
        for stored in report["bootstrap_utilities"]:
            multiplicities = np.bincount(
                rng.integers(len(unique), size=len(unique)), minlength=len(unique)
            )
            solved = solve(x, y, weights * multiplicities[index], ridge)
            max_bootstrap = max(max_bootstrap, float(np.max(abs(solved[:-1] - stored))))
            bootstrap_fits += 1
        by_programme = {p["id"]: p for p in data["programmes"]}
        by_candidate = {c["id"]: c for c in data["candidates"]}
        for row in report["native_validation"]["auditions"]:
            p = by_programme[row["programme_id"]]
            c = by_candidate[row["candidate_id"]]
            m = row["native_metadata"]
            pi = data["programmes"].index(p)
            ci = data["candidates"].index(c)
            folder = Path(case["report"]).parent / "native" / f"render-{pi}-{ci}"
            assert sha(p["audio_file"]) == p["audio_sha256"] == m["input_sha256"]
            assert (
                sha(folder / "candidate.sofa")
                == c["parameters"]["sofa_sha256"]
                == m["sofa_sha256"]
            )
            assert sha(folder / "output.f32le") == m["output_sha256"]
            raw = np.fromfile(folder / "output.f32le", dtype="<f4").reshape(-1, 2)
            scaled = raw[m["latency_samples"] :].astype(float) * 10 ** (
                row["post_gain_db"] / 20
            )
            power = math.fsum(float(v) * float(v) for v in scaled.flat) / scaled.size
            db = 10 * math.log10(max(power, 1e-30))
            peak = max(abs(float(v)) for v in scaled.flat)
            max_metric = max(
                max_metric, abs(db - row["rms_dbfs"]), abs(peak - row["peak"])
            )
            assert (
                m["requested_params"]["room_model"]["max_order"] == 0
                and not m["requested_params"]["late_reverb_enabled"]
            )
            auditions += 1
        if report["accepted"]:
            folder = Path(case["report"]).parent
            selected = report["selected_id"]
            assert (
                sha(folder / "selected.sofa")
                == by_candidate[selected]["parameters"]["sofa_sha256"]
            )
            preset = json.loads((folder / "preset.json").read_text())
            assert preset["hrtf_file"] == str((folder / "selected.sofa").resolve())
            for i in range(len(data["programmes"])):
                ci = ids.index(selected)
                a = np.fromfile(
                    folder / "native" / f"render-{i}-{ci}" / "output.f32le", dtype="<f4"
                )
                b = np.fromfile(folder / f"reopen-{i}" / "output.f32le", dtype="<f4")
                assert a.shape == b.shape
                error = float(np.max(abs(a.astype(float) - b)))
                assert error <= 1e-7
                max_reopen = max(max_reopen, error)
        else:
            assert not (Path(case["report"]).parent / "preset.json").exists()
    assert max_theta < 2e-6 and max_bootstrap < 2e-6 and max_metric < 1e-10
    # Reproduce prior crossfeed behavior directly from the committed source.
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import fit_crossfeed_preferences as current
    from test_crossfeed_preferences import session

    old = types.ModuleType("crossfeed_before_extraction")
    old_source = subprocess.run(
        ["git", "show", "33f983df:scripts/ml/fit_crossfeed_preferences.py"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    # Only load the fixed, trusted local revision above for numerical parity.
    exec(compile(old_source, "crossfeed-before-extraction", "exec"), old.__dict__)  # noqa: S102
    for kind in ["consistent", "ties", "heterogeneous", "side-bias"]:
        data = session(kind)
        assert old.fit(data) == current.fit(data), kind
    result = {
        "cases": len(cases),
        "native_auditions_verified": auditions,
        "independent_bfgs_bootstrap_fits": bootstrap_fits,
        "max_full_model_difference": max_theta,
        "max_bootstrap_difference": max_bootstrap,
        "max_native_metric_difference": max_metric,
        "max_reopen_error": max_reopen,
        "crossfeed_prior_reports_identical": 4,
        "scope": "Functional synthetic data only; no human preference, localization or calibrated confidence claim",
    }
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
