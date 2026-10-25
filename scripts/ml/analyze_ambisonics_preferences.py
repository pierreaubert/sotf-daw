"""Independent BFGS fits, source accounting, native energy and export checks."""

import argparse
import hashlib
import json
import math
from pathlib import Path

import numpy as np
from analyze_binaural_preferences import solve


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evaluation", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        raise ValueError("Refuse to overwrite analysis")
    cases = json.loads(args.evaluation.read_text())["cases"]
    max_fit = max_boot = max_lopo = max_metrics = max_energy = 0.0
    auditions = bootstrap_fits = 0
    for case in cases:
        data = json.loads(Path(case["session"]).read_text())
        report = json.loads(Path(case["report"]).read_text())
        ids = [c["id"] for c in data["candidates"]]
        groups = sorted({t["programme_id"] for t in data["trials"]})
        x = np.zeros((len(data["trials"]), len(ids) + 1))
        labels, index = [], []
        for i, t in enumerate(data["trials"]):
            x[i, ids.index(t["a"])], x[i, ids.index(t["b"])], x[i, -1] = 1, -1, 1
            labels.append({"a": 1.0, "b": 0.0, "tie": 0.5}[t["choice"]])
            index.append(groups.index(t["programme_id"]))
        index = np.array(index)
        labels = np.array(labels)
        weights = 1 / np.bincount(index)[index]
        ridge = report["policy"]["ridge"]
        expected = np.r_[
            report["model"]["utilities"], report["model"]["presentation_side_bias"]
        ]
        max_fit = max(
            max_fit, float(np.max(abs(solve(x, labels, weights, ridge) - expected)))
        )
        rng = np.random.default_rng(report["policy"]["bootstrap_seed"])
        for stored in report["bootstrap_utilities"]:
            multiplicities = np.bincount(
                rng.integers(len(groups), size=len(groups)), minlength=len(groups)
            )
            fitted = solve(x, labels, weights * multiplicities[index], ridge)
            max_boot = max(max_boot, float(np.max(abs(fitted[:-1] - stored))))
            bootstrap_fits += 1
        losses = []
        for g in range(len(groups)):
            held = index == g
            fitted = solve(x, labels, weights * ~held, ridge)
            logits = x[held] @ fitted
            losses.append(
                float(np.mean(np.logaddexp(0, logits) - labels[held] * logits))
            )
        max_lopo = max(max_lopo, abs(float(np.mean(losses)) - report["lopo_log_loss"]))
        by_id = {p["id"]: p for p in data["programmes"]}
        for row in report["native_validation"]["auditions"]:
            mpath = Path(row["native_metadata_file"])
            m = json.loads(mpath.read_text())
            programme = by_id[row["programme_id"]]
            assert (
                sha(programme["audio_file"])
                == programme["audio_sha256"]
                == row["input_sha256"]
                == m["input_sha256"]
            )
            assert (
                sha(mpath.parent / "output.f32le")
                == row["output_sha256"]
                == m["output_sha256"]
            )
            raw = np.fromfile(mpath.parent / "output.f32le", dtype="<f4").astype(float)
            audition = raw * 10 ** (row["post_gain_db"] / 20)
            power = math.fsum(float(v) * float(v) for v in audition) / len(audition)
            level = 10 * math.log10(max(power, 1e-30))
            max_metrics = max(
                max_metrics,
                abs(level - row["rms_dbfs"]),
                abs(float(np.max(abs(audition))) - row["sample_peak"]),
            )
            names = ["decoder_matrix"] + (
                ["basic_matrix"] if m["effective_params"]["dual_band"] else []
            )
            matrices = [
                np.array(m[n]["matrix"]).reshape(
                    m["output_channels"], m["input_channels"]
                )
                for n in names
            ]
            degree = np.floor(np.sqrt(np.arange(m["input_channels"]))).astype(int)
            covariance = np.diag(1 / (2 * degree + 1))
            gains = np.array(m["speaker_gains"])
            energies = []
            for d in matrices:
                baseline = float(np.trace(d @ covariance @ d.T))
                corrected = float(
                    np.trace((gains[:, None] * d) @ covariance @ (gains[:, None] * d).T)
                )
                energies.append(baseline)
                band = m["energy"]["bands"][len(energies) - 1]
                max_energy = max(
                    max_energy,
                    abs(baseline - band["baseline"]),
                    abs(corrected - band["corrected"]),
                )
            for c in m["energy"]["cross_terms"]:
                i, j = c["bands"]
                baseline = float(np.trace(matrices[i] @ covariance @ matrices[j].T))
                corrected = float(
                    np.trace(
                        (gains[:, None] * matrices[i])
                        @ covariance
                        @ (gains[:, None] * matrices[j]).T
                    )
                )
                normalized = (corrected - baseline) / np.sqrt(energies[i] * energies[j])
                max_energy = max(max_energy, abs(normalized - c["normalized_error"]))
            auditions += 1
        folder = Path(case["report"]).parent
        assert (folder / "preset.json").exists() == report["accepted"]
        if report["accepted"]:
            selected = ids.index(report["selected_id"])
            preset = json.loads((folder / "preset.json").read_text())
            assert [p["plugin_type"] for p in preset["plugins"]] == [
                "ambisonics_decoder",
                "matrix",
            ]
            for i in range(len(data["programmes"])):
                assert (
                    folder / "native" / f"render-{i}-{selected}" / "output.f32le"
                ).read_bytes() == (folder / f"reopen-{i}" / "output.f32le").read_bytes()
        else:
            assert report["selected_id"] is None and report["parameters"] is None
    assert (
        max(max_fit, max_boot) < 2e-6
        and max_lopo < 1e-6
        and max_metrics < 1e-10
        and max_energy < 1e-12
    )
    result = {
        "cases": len(cases),
        "native_auditions_verified": auditions,
        "independent_bootstrap_fits": bootstrap_fits,
        "independent_lopo_fits": len(cases) * 8,
        "max_fit_error": max_fit,
        "max_bootstrap_error": max_boot,
        "max_lopo_error": max_lopo,
        "max_native_metric_error": max_metrics,
        "max_energy_error": max_energy,
        "scope": "Independent numerical and native/export accounting only; simulated labels provide no human preference, spatial quality or calibrated confidence evidence.",
    }
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
