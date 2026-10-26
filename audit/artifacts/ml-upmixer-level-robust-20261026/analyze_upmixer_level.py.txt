"""Independently verify level drift from raw native masks and speaker waveforms."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from upmixer_mask_cases import fixture
from upmixer_source_mask import FFT, analyze, predict
from upmixer_source_mask_v2 import load


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def verify(root, split):
    report = json.loads((root / "report.json").read_text())
    theta = load(root / "model.frozen.json", report["model_sha256"])
    if (
        np.any(theta[40:48] != 0)
        or digest(root / "renderer.frozen") != report["renderer_sha256"]
    ):
        raise ValueError("Excitation independence or renderer binding mismatch")
    start, count = (20306120, 4) if split == "validation" else (20307120, 8)
    kinds = {"matched", "hard_pan", "direct_only", "ambient_only"}
    if split == "final":
        kinds |= {"correlated_bed", "antiphase_bed", "level_shift", "silence"}
    expected = {
        (seed, kind, level)
        for seed in range(start, start + count)
        for kind in kinds
        for level in [-40, 0, 20]
    }
    actual = [
        (r["identity"]["seed"], r["identity"]["kind"], r["level_db"])
        for r in report["cases"]
    ]
    if len(actual) != len(expected) or set(actual) != expected:
        raise ValueError("Missing, duplicated or unexpected level cases")
    groups, verified = {}, 0
    maximum_network_error = 0.0
    for row in report["cases"]:
        identity = row["identity"]
        source, direct, _, expected_identity = fixture(
            identity["seed"], identity["kind"]
        )
        gain = 10 ** (row["level_db"] / 20)
        if (
            identity != expected_identity
            or hashlib.sha256(
                np.asarray(source * gain, dtype="<f4").tobytes()
            ).hexdigest()
            != row["files"]["input"]["sha256"]
        ):
            raise ValueError("Quantized source mismatch")
        for key in ["files", "native_files"]:
            for entry in row[key].values():
                if digest(entry["path"]) != entry["sha256"]:
                    raise ValueError("Raw level evidence hash mismatch")
                verified += 1
        meta = json.loads(Path(row["files"]["features_output"]["path"]).read_text())
        states = [r["state"] for r in meta["rows"]]
        features = np.array(
            [[f if f is not None else [0.0] * 6 for f in s["features"]] for s in states]
        )
        selected = np.array([[m is not None for m in s["masks"]] for s in states])
        masks = np.array(
            [[m if m is not None else 0.0 for m in s["masks"]] for s in states]
        )
        network = (
            float(abs(predict(theta, features[selected]) - masks[selected]).max())
            if selected.any()
            else 0.0
        )
        maximum_network_error = max(maximum_network_error, network)
        sd = analyze(np.asarray(direct * gain, dtype="<f4"))[: len(states)]
        pd = np.stack(
            [
                np.sum(abs(sd[:, s : min(s + 8, FFT // 2)]) ** 2, axis=(1, 2))
                for s in range(1, FFT // 2, 8)
            ],
            axis=1,
        )
        denominator = pd[selected].sum()
        retention = (
            float(
                10
                * np.log10(
                    max(float((pd * masks**2)[selected].sum()), 1e-300) / denominator
                )
            )
            if denominator
            else None
        )
        metric = row["metrics"] if "metrics" in row else row["methods"]["learned"]
        if retention != metric["conditional_spectral_direct_retention_db"]:
            raise ValueError(
                "Reported conditional direct retention differs from known-stem reconstruction"
            )
        waveforms = {}
        for method, key in [("selected", "files"), ("native", "native_files")]:
            waveforms[method] = (
                np.fromfile(row[key]["output"]["path"], dtype="<f4").astype(float)
                / gain
            )
            if not np.isfinite(waveforms[method]).all():
                raise ValueError("Non-finite native waveform")
        groups.setdefault((identity["seed"], identity["kind"]), {})[row["level_db"]] = {
            "masks": masks,
            "selected": selected,
            "retention": retention,
            "waveforms": waveforms,
        }
    comparisons = []
    for (seed, kind), levels in sorted(groups.items()):
        base = levels[0]
        for level in [-40, 20]:
            other = levels[level]
            shared = base["selected"] & other["selected"]
            drift = (
                float(abs(base["masks"][shared] - other["masks"][shared]).max())
                if shared.any()
                else 0.0
            )
            retention_drift = (
                abs(base["retention"] - other["retention"])
                if base["retention"] is not None and other["retention"] is not None
                else None
            )
            comparisons.append(
                {
                    "seed": seed,
                    "kind": kind,
                    "level_db": level,
                    "shared_band_frames": int(shared.sum()),
                    "eligibility_changes": int(
                        np.count_nonzero(base["selected"] != other["selected"])
                    ),
                    "maximum_probability_drift": drift,
                    "conditional_retention_drift_db": retention_drift,
                    "normalized_waveform_relative_rms_change": {
                        method: float(
                            np.sqrt(
                                np.sum(
                                    (
                                        base["waveforms"][method]
                                        - other["waveforms"][method]
                                    )
                                    ** 2
                                )
                                / max(np.sum(base["waveforms"][method] ** 2), 1e-30)
                            )
                        )
                        for method in ["selected", "native"]
                    },
                }
            )
    probability = max(r["maximum_probability_drift"] for r in comparisons)
    retention = max(r["conditional_retention_drift_db"] or 0.0 for r in comparisons)
    passed = bool(
        probability <= 1e-3
        and retention <= 0.05
        and maximum_network_error <= 2e-7
        and all(
            r["shared_band_frames"] > 0 for r in comparisons if r["kind"] != "silence"
        )
    )
    if split == "validation" and (
        report["maximum_probability_drift"],
        report["maximum_conditional_retention_drift_db"],
        report["development_gate_pass"],
    ) != (probability, retention, passed):
        raise ValueError(
            "Development level gate differs from independent recomputation"
        )
    return {
        "scope": split,
        "cases": len(actual),
        "verified_raw_hashes": verified,
        "maximum_probability_drift": probability,
        "maximum_conditional_retention_drift_db": retention,
        "maximum_native_network_error": maximum_network_error,
        "model_level_gate_pass": passed,
        "comparisons": comparisons,
        "report_sha256": digest(root / "report.json"),
        "deployment_admitted": False,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--split", choices=["validation", "final"], required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = verify(args.root, args.split)
    with args.output.open("x") as stream:
        stream.write(json.dumps(result, indent=2, allow_nan=False) + "\n")
    print(json.dumps({k: v for k, v in result.items() if k != "comparisons"}))


if __name__ == "__main__":
    main()
