"""Verify raw-power study evidence and unchanged v1 audio/decision exports."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from upmixer_mask_cases import fixture
from upmixer_source_mask import FFT, analyze, evidence


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def verify(before, after):
    old = json.loads((before / "report.json").read_text())
    new = json.loads((after / "report.json").read_text())
    if len(old["cases"]) != len(new["cases"]) or not new["cases"]:
        raise ValueError("Development case count mismatch")
    checked = 0
    maximum_relative_power_error = 0.0
    if (
        new["feature_tolerance"] != 1e-4
        or new["minimum_retained_reference_band_energy_fraction"] != 0.999
    ):
        raise ValueError("Development gates changed")
    for root, report in [(before, old), (after, new)]:
        if digest(root / "renderer.frozen") != report["renderer_sha256"]:
            raise ValueError("Frozen renderer hash mismatch")
        if digest(root / "model.v1.frozen.json") != report["model_sha256"]:
            raise ValueError("Frozen model hash mismatch")
        for row in report["cases"]:
            for entry in row["files"].values():
                if digest(entry["path"]) != entry["sha256"]:
                    raise ValueError("Raw evidence hash mismatch")
                checked += 1
    for a, b in zip(old["cases"], new["cases"], strict=True):
        if a["identity"] != b["identity"] or a["level_db"] != b["level_db"]:
            raise ValueError("Source identity/order mismatch")
        for key in ["input", "output"]:
            if a["files"][key]["sha256"] != b["files"][key]["sha256"]:
                raise ValueError("Raw-power telemetry changed v1 input/output")
        am = json.loads(Path(a["files"]["features_output"]["path"]).read_text())
        bm = json.loads(Path(b["files"]["features_output"]["path"]).read_text())
        for x, y in zip(am["rows"], bm["rows"], strict=True):
            ys = dict(y["state"])
            ys.pop("band_power")
            if x != (y | {"state": ys}):
                raise ValueError("Raw-power telemetry changed v1 decisions")
        af, bf = am["final_state"], dict(bm["final_state"])
        bf.pop("band_power")
        if af != bf:
            raise ValueError("Raw-power telemetry changed v1 final state")
        source, _, _, identity = fixture(b["identity"]["seed"], b["identity"]["kind"])
        if identity != b["identity"]:
            raise ValueError("Regenerated source identity mismatch")
        signal = np.asarray(source * 10 ** (b["level_db"] / 20), dtype="<f4")
        if (
            hashlib.sha256(signal.tobytes()).hexdigest()
            != b["files"]["input"]["sha256"]
        ):
            raise ValueError("Quantized source hash mismatch")
        spectra = analyze(signal)[: len(bm["rows"])]
        reference, _, reference_valid = evidence(spectra)
        power = np.stack(
            [
                np.sum(abs(spectra[:, s : min(s + 8, FFT // 2)]) ** 2, axis=(1, 2))
                for s in range(1, FFT // 2, 8)
            ],
            axis=1,
        )
        native = np.array(
            [row["state"]["band_power"] for row in bm["rows"]], dtype=float
        )
        if not np.isfinite(native).all() or np.any(native < 0):
            raise ValueError("Missing/invalid supported native power")
        significant = (power > 1e-16) & (
            power >= power.max(axis=1, keepdims=True) * 1e-6
        )
        error = (
            float(np.max(abs(native[significant] / power[significant] - 1)))
            if significant.any()
            else 0.0
        )
        maximum_relative_power_error = max(maximum_relative_power_error, error)
        if error > 1e-3:
            raise ValueError(
                "Raw power differs from independent FFT beyond the declared diagnostic gate"
            )
        features = np.array(
            [
                [f if f is not None else [0.0] * 6 for f in row["state"]["features"]]
                for row in bm["rows"]
            ]
        )
        valid = np.array(
            [[f is not None for f in row["state"]["features"]] for row in bm["rows"]]
        )
        for db in [40, 50, 60]:
            a_mask = reference_valid & (
                power >= power.max(axis=1, keepdims=True) * 10 ** (-db / 10)
            )
            b_mask = valid & (
                native >= native.max(axis=1, keepdims=True) * 10 ** (-db / 10)
            )
            shared = a_mask & b_mask
            max_error = (
                float(abs(features[shared] - reference[shared]).max())
                if shared.any()
                else 0.0
            )
            energy = float(power[shared].sum() / power.sum()) if power.sum() else None
            expected = {
                "selected_band_frames": int(shared.sum()),
                "validity_disagreements": int(np.count_nonzero(a_mask != b_mask)),
                "maximum_feature_error": max_error,
                "retained_reference_band_energy_fraction": energy,
                "pass": bool(
                    np.array_equal(a_mask, b_mask)
                    and max_error <= 1e-4
                    and (energy is None or (shared.any() and energy >= 0.999))
                ),
            }
            if b["candidates"][str(db)] != expected:
                raise ValueError(
                    "Reported development gate differs from independent recomputation"
                )
    passing = [
        db
        for db in [40, 50, 60]
        if all(row["candidates"][str(db)]["pass"] for row in new["cases"])
    ]
    chosen = max(passing) if passing else None
    proposal = new["proposed_policy"]
    if (
        None if proposal is None else proposal["dynamic_range_below_peak_band_db"]
    ) != chosen:
        raise ValueError("Proposal violates predeclared selection rule")
    return {
        "cases": len(new["cases"]),
        "verified_raw_hashes": checked,
        "v1_output_and_decisions_exactly_preserved": True,
        "all_raw_power_candidate_metrics_independently_recomputed": True,
        "maximum_relative_power_error_above_proposed_floor": maximum_relative_power_error,
        "relative_power_diagnostic_tolerance": 1e-3,
        "before_report_sha256": digest(before / "report.json"),
        "after_report_sha256": digest(after / "report.json"),
        "deployment_admitted": False,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--before", type=Path, required=True)
    parser.add_argument("--after", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = verify(args.before, args.after)
    with args.output.open("x") as stream:
        stream.write(json.dumps(result, indent=2, allow_nan=False) + "\n")
    print(json.dumps(result))


if __name__ == "__main__":
    main()
