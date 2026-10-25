"""Independently verify frozen IR budgets, selection, response and clean-source loss."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from scipy.fft import fft


def read_json(path):
    return json.loads(path.read_text())


def quality(clean, noisy, output):
    clean, noisy, output = [x.astype(float) for x in [clean, noisy, output]]
    energy = max(float(np.sum(clean**2)), 1e-30)
    error = max(float(np.sum((output - clean) ** 2)), 1e-30)
    original = max(float(np.sum((noisy - clean) ** 2)), 1e-30)
    return {
        "snr_db": float(10 * np.log10(energy / error)),
        "improvement_db": float(10 * np.log10(original / error)),
        "projection_loss_db": float(
            -20 * np.log10(max(float(np.sum(output * clean)) / energy, 1e-15))
        ),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--previous-evidence", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        raise ValueError("preserve previous independent report")
    protocol = read_json(args.evidence / "protocol.json")
    for name, expected in protocol["source_sha256"].items():
        if (
            hashlib.sha256(
                (args.evidence / "evaluation-source" / name).read_bytes()
            ).hexdigest()
            != expected
        ):
            raise ValueError("frozen source hash mismatch")
    selection = read_json(args.evidence / "development-selection.json")
    summary = read_json(args.evidence / "summary.json")
    seen = {
        row["noisy_sha256"]
        for rows in read_json(args.previous_evidence / "training-sources.json").values()
        for row in rows
    }
    seen.update(
        read_json(p)["arrays"]["noisy"]["sha256"]
        for p in args.previous_evidence.glob("case-*.json")
    )
    previous = len(seen)
    for row in read_json(args.evidence / "development-sources.json"):
        if row["noisy_sha256"] in seen:
            raise ValueError("development overlap")
        seen.add(row["noisy_sha256"])
    for method in ["neural", "wiener", "decay"]:
        policies = []
        for row in selection["candidates"]:
            if row["method"] != method:
                continue
            scores = row["scores"]
            worst = min(s["improvement_db"] for s in scores)
            median = float(np.median([s["improvement_db"] for s in scores]))
            loss = max(s.get("added_structure_projection_loss_db", 0.0) for s in scores)
            if (
                len(scores) != 60
                or row["worst_improvement_db"] != worst
                or row["median_improvement_db"] != median
                or row["max_added_structure_loss_db"] != loss
                or row["qualified"] != (worst >= -0.25 and loss <= 0.1)
            ):
                raise ValueError("selection score mismatch")
            policies.append(row)
        qualified = [r for r in policies if r["qualified"]]
        chosen = (
            max(
                qualified,
                key=lambda r: (r["median_improvement_db"], r["worst_improvement_db"]),
            )
            if qualified
            else max(
                policies,
                key=lambda r: (r["worst_improvement_db"], r["median_improvement_db"]),
            )
        )
        raw = (args.evidence / (method + "-policy.json")).read_bytes()
        if (
            chosen["policy"] != selection["selected"][method]
            or json.loads(raw) != chosen["policy"]
            or hashlib.sha256(raw).hexdigest() != selection["policy_sha256"][method]
        ):
            raise ValueError("selected policy mismatch")
    metric_error, response_error, budget_max = 0.0, 0.0, 0.0
    rows, conditional, prefix_fallbacks = [], [], 0
    for index in range(104):
        row = read_json(args.evidence / f"case-{index:03d}.json")
        info = row["metadata"]
        arrays = {}
        for name, record in row["arrays"].items():
            path = Path(row["directory"]) / (
                name + (".f64le" if name.endswith("_mask") else ".f32le")
            )
            raw = path.read_bytes()
            if hashlib.sha256(raw).hexdigest() != record["sha256"]:
                raise ValueError("array hash mismatch")
            data = np.frombuffer(
                raw, dtype="<f8" if name.endswith("_mask") else "<f4"
            ).astype(float)
            if not np.isfinite(data).all() or len(data) != record.get(
                "floats", record.get("doubles")
            ):
                raise ValueError("array geometry/nonfinite")
            arrays[name] = (
                data
                if name.endswith("_mask")
                else data.reshape(info["frames"], info["channels"])
            )
        identity = row["arrays"]["noisy"]["sha256"]
        if identity in seen:
            raise ValueError("final/source overlap")
        seen.add(identity)
        clean, noisy = arrays["clean"], arrays["noisy"]
        protected = info["arrival"] + round(info["rate"] * 0.05)
        if info["family"] == "prefix_step":
            prefix_fallbacks += 1
        for method, details in row["methods"].items():
            output = noisy if method == "unchanged" else arrays[method]
            recomputed = quality(clean, noisy, output)
            if "structure_window" in info and method != "unchanged":
                first, last = info["structure_window"]
                baseline = float(np.sum(noisy[first:last] * clean[first:last]))
                changed = float(np.sum(output[first:last] * clean[first:last]))
                recomputed["added_structure_projection_loss_db"] = float(
                    -20 * np.log10(max(changed / max(baseline, 1e-30), 1e-15))
                )
            if set(recomputed) != set(details["quality"]):
                raise ValueError("missing or extra quality metric")
            for key, value in recomputed.items():
                error = abs(value - details["quality"][key])
                metric_error = max(metric_error, error)
                if error > 1e-8:
                    raise ValueError("quality metric mismatch")
            if method == "unchanged":
                continue
            checks = details["checks"]
            if info["family"] == "prefix_step" and (
                checks["accepted"]
                or not checks.get("fallback_error")
                or not np.array_equal(output, noisy)
            ):
                raise ValueError("nonstationary prefix did not retain exact original")
            if not np.array_equal(output[:protected], noisy[:protected]):
                raise ValueError("early output changed")
            if method.endswith("unprotected"):
                continue
            policy = selection["selected"][method]
            if details["policy_sha256"] != selection["policy_sha256"][method]:
                raise ValueError("applied policy mismatch")
            mask = arrays[method + "_mask"]
            if (
                mask.shape != (len(noisy),)
                or np.any(mask < 0.05 - 1e-12)
                or np.any(mask > 1 + 1e-12)
                or not np.array_equal(mask[:protected], np.ones(protected))
            ):
                raise ValueError("invalid linked mask")
            candidate = (noisy * mask[:, None]).astype("<f4").astype(float)
            if checks["accepted"]:
                if not np.array_equal(candidate, output):
                    raise ValueError("candidate reconstruction mismatch")
            elif not np.array_equal(output, noisy):
                raise ValueError("fallback was not exact original")
            if "fallback_error" in checks:
                if checks["accepted"] or not np.array_equal(mask, np.ones(len(mask))):
                    raise ValueError("invalid analysis-failure fallback")
                continue
            width = round(info["rate"] * policy["window_ms"] / 1000)
            starts = np.arange(protected, len(noisy), width)
            lengths = np.minimum(width, len(noisy) - starts)
            noise = (
                np.sum(noisy[: info["noise_frames"]] ** 2, axis=0)
                / info["noise_frames"]
            )
            removed = np.add.reduceat(
                (candidate[protected:] - noisy[protected:]) ** 2,
                starts - protected,
                axis=0,
            )
            budget = lengths[:, None] * noise[None, :] * policy["noise_fraction"]
            ratios = np.divide(
                removed, budget, out=np.zeros_like(removed), where=budget > 0
            )
            if np.any((budget == 0) & (removed > 0)):
                raise ValueError("changed a zero-noise channel")
            maximum = float(np.max(ratios))
            budget_max = max(budget_max, maximum)
            saved = checks["preservation"]
            if (
                not saved["within_budget"]
                or maximum > 1.00001
                or abs(maximum - saved["max_removed_noise_ratio"]) > 1e-9
            ):
                raise ValueError("local noise budget violated")
            nfft = 1 << (len(noisy) - 1).bit_length()
            grid = np.arange(nfft // 2 + 1) * info["rate"] / nfft
            points = np.unique(
                np.searchsorted(
                    grid, np.geomspace(40, min(16000, info["rate"] * 0.45), 192)
                )
            )
            raw = fft(noisy, n=nfft, axis=0)[points]
            changed = fft(candidate, n=nfft, axis=0)[points]
            energetic = np.abs(raw) >= np.max(np.abs(raw), axis=0, keepdims=True) * 0.1
            ratio = changed / np.where(np.abs(raw) > 1e-30, raw, 1)
            values = {
                "relative_ir_change": float(
                    np.sqrt(np.sum((candidate - noisy) ** 2) / np.sum(noisy**2))
                ),
                "max_magnitude_change_db": float(
                    np.max(
                        np.abs(
                            20 * np.log10(np.maximum(np.abs(ratio[energetic]), 1e-30))
                        )
                    )
                ),
                "max_phase_change_degrees": float(
                    np.max(np.abs(np.angle(ratio[energetic], deg=True)))
                ),
            }
            for key, value in values.items():
                error = abs(value - checks[key])
                response_error = max(response_error, error)
                if error > 1e-7:
                    raise ValueError("response mismatch")
            accepted = (
                values["relative_ir_change"] <= 0.05
                and values["max_magnitude_change_db"] <= 1
                and values["max_phase_change_degrees"] <= 10
            )
            if accepted != checks["accepted"]:
                raise ValueError("response admission mismatch")
            effective_mask = mask if checks["accepted"] else np.ones(len(mask))
            clean_projection = float(
                np.sum(clean**2 * effective_mask[:, None]) / np.sum(clean**2)
            )
            record = {
                "case": index,
                "method": method,
                "conditional_clean_projection_loss_db": float(
                    -20 * np.log10(max(clean_projection, 1e-15))
                ),
            }
            if "structure_window" in info:
                first, last = info["structure_window"]
                energy = clean[first:last] ** 2
                projection = float(
                    np.sum(energy * effective_mask[first:last, None]) / np.sum(energy)
                )
                record["conditional_structure_projection_loss_db"] = float(
                    -20 * np.log10(max(projection, 1e-15))
                )
            conditional.append(record)
        rows.append(row)
    if len(seen) != previous + 60 + 104 or prefix_fallbacks != 8:
        raise ValueError("source/count mismatch")
    for split in ["final", "shift"]:
        subset = [r for r in rows if r["split"] == split]
        for method in ["neural", "wiener", "decay"]:
            recorded = summary["splits"][split][method]
            if recorded["worst_improvement_db"] != min(
                r["methods"][method]["quality"]["improvement_db"] for r in subset
            ) or recorded["median_improvement_db"] != float(
                np.median(
                    [r["methods"][method]["quality"]["improvement_db"] for r in subset]
                )
            ):
                raise ValueError("summary mismatch")
        wins = sum(
            r["methods"]["neural"]["quality"]["snr_db"]
            > max(r["methods"][m]["quality"]["snr_db"] for m in ["wiener", "decay"])
            + 1e-6
            for r in subset
        )
        if wins != summary["splits"][split]["neural_better_than_best_classical"]:
            raise ValueError("classical comparison mismatch")
    report = {
        "cases": len(rows),
        "sources_disjoint": True,
        "prefix_failure_fallback_cases": prefix_fallbacks,
        "max_metric_error": metric_error,
        "max_response_error": response_error,
        "max_removed_noise_ratio": budget_max,
        "worst_conditional_clean_projection_loss_db": max(
            r["conditional_clean_projection_loss_db"]
            for r in conditional
            if r["method"] == "neural"
        ),
        "worst_conditional_structure_projection_loss_db": max(
            r.get("conditional_structure_projection_loss_db", 0.0)
            for r in conditional
            if r["method"] == "neural"
        ),
        "conditional_scope": "Mixture-derived masks held fixed and applied to known clean IR; not measured-device acceptance or inference of clean structure from PCM.",
        "splits": summary["splits"],
        "adoption_recommended": False,
        "conclusion": "Local budget addresses large synthetic late-structure losses, but learned advantage is limited and measured-room/noise validation remains open.",
    }
    args.output.write_text(json.dumps(report, indent=2, allow_nan=False) + "\n")
    print(json.dumps({k: v for k, v in report.items() if k != "splits"}))


if __name__ == "__main__":
    main()
