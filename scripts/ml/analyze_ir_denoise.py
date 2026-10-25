"""Independently check IR export metrics and response limits from frozen arrays."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from scipy.fft import fft


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        raise ValueError("preserve previous independent analysis")
    protocol = json.loads((args.evidence / "protocol.json").read_text())
    for name, expected in protocol["source_sha256"].items():
        if (
            hashlib.sha256(
                (args.evidence / "evaluation-source" / name).read_bytes()
            ).hexdigest()
            != expected
        ):
            raise ValueError("frozen source hash mismatch")
    sources = json.loads((args.evidence / "training-sources.json").read_text())
    seen = set()
    for split in ["train", "development"]:
        if (
            len(sources[split])
            != protocol[split + "_scenes" if split == "development" else "train_scenes"]
        ):
            raise ValueError("source split count mismatch")
        for row in sources[split]:
            if row["noisy_sha256"] in seen:
                raise ValueError("training/development overlap")
            seen.add(row["noisy_sha256"])
    training = json.loads((args.evidence / "training.json").read_text())
    if (
        hashlib.sha256((args.evidence / "model.json").read_bytes()).hexdigest()
        != training["model_sha256"]
    ):
        raise ValueError("model hash mismatch")
    metric_max, response_max = 0.0, 0.0
    totals, worst = {}, None
    for index in range(78):
        row = json.loads((args.evidence / f"case-{index:03d}.json").read_text())
        info = row["metadata"]
        arrays = {}
        for name, record in row["arrays"].items():
            raw = (Path(row["directory"]) / (name + ".f32le")).read_bytes()
            if hashlib.sha256(raw).hexdigest() != record["sha256"]:
                raise ValueError("array hash mismatch")
            data = np.frombuffer(raw, dtype="<f4").astype(float)
            if (
                data.size != info["frames"] * info["channels"]
                or data.size != record["floats"]
                or not np.isfinite(data).all()
            ):
                raise ValueError("array geometry/nonfinite")
            arrays[name] = data.reshape(info["frames"], info["channels"])
        identity = row["arrays"]["noisy"]["sha256"]
        if identity in seen:
            raise ValueError("final/source overlap")
        seen.add(identity)
        clean, noisy = arrays["clean"], arrays["noisy"]
        nfft = 1 << (len(noisy) - 1).bit_length()
        grid = np.arange(nfft // 2 + 1) * info["rate"] / nfft
        points = np.unique(
            np.searchsorted(
                grid, np.geomspace(40, min(16000, info["rate"] * 0.45), 192)
            )
        )
        original_spectrum = fft(noisy, n=nfft, axis=0)[points]
        for method in ["neural", "wiener", "decay", "unchanged"]:
            result = arrays[method] if method != "unchanged" else noisy
            residual_power = max(float(np.sum((result - clean) ** 2)), 1e-30)
            original_error = max(float(np.sum((noisy - clean) ** 2)), 1e-30)
            energy = max(float(np.sum(clean**2)), 1e-30)
            values = {
                "snr_db": float(10 * np.log10(energy / residual_power)),
                "improvement_db": float(10 * np.log10(original_error / residual_power)),
                "projection_loss_db": float(
                    -20 * np.log10(max(float(np.sum(result * clean)) / energy, 1e-15))
                ),
            }
            for key, value in values.items():
                difference = abs(value - row["methods"][method]["quality"][key])
                metric_max = max(metric_max, difference)
                if difference > 1e-8:
                    raise ValueError("quality metric mismatch")
            if method == "unchanged":
                continue
            checks = row["methods"][method]["checks"]
            if checks["accepted"]:
                if not np.array_equal(
                    result[: info["arrival"] + round(info["rate"] * 0.05)],
                    noisy[: info["arrival"] + round(info["rate"] * 0.05)],
                ):
                    raise ValueError("early response changed")
                modified = fft(result, n=nfft, axis=0)[points]
                energetic = (
                    np.abs(original_spectrum)
                    >= np.max(np.abs(original_spectrum), axis=0, keepdims=True) * 0.1
                )
                ratio = modified / np.where(
                    np.abs(original_spectrum) > 1e-30, original_spectrum, 1
                )
                actual = {
                    "relative_ir_change": float(
                        np.sqrt(np.sum((result - noisy) ** 2) / np.sum(noisy**2))
                    ),
                    "max_magnitude_change_db": float(
                        np.max(
                            np.abs(
                                20
                                * np.log10(np.maximum(np.abs(ratio[energetic]), 1e-30))
                            )
                        )
                    ),
                    "max_phase_change_degrees": float(
                        np.max(np.abs(np.angle(ratio[energetic], deg=True)))
                    ),
                }
                for key, value in actual.items():
                    error = abs(value - checks[key])
                    response_max = max(response_max, error)
                    # Frozen NumPy 2.5 candidate rFFT retained complex64; this
                    # reducer uses float64/full FFT for both operands.
                    if error > 1e-4:
                        raise ValueError(
                            f"response check mismatch: case={index} method={method} key={key} actual={value} stored={checks[key]} error={error}"
                        )
                if (
                    actual["relative_ir_change"] > 0.05
                    or actual["max_magnitude_change_db"] > 1
                    or actual["max_phase_change_degrees"] > 10
                ):
                    raise ValueError("accepted export exceeds response limits")
                # A linked mask preserves instantaneous inter-channel ratios.
                cross = result[:, :1] * noisy - result * noisy[:, :1]
                if np.max(np.abs(cross)) > 1e-7:
                    raise ValueError("channel-linked mask violated")
            elif not np.array_equal(result, noisy):
                raise ValueError("rejected candidate is not exact original f32")
            key = row["split"] + "/" + method
            totals.setdefault(key, []).append(
                {"index": index, "accepted": checks["accepted"], **values}
            )
            if method == "neural" and (
                worst is None or values["improvement_db"] < worst["improvement_db"]
            ):
                worst = {
                    "index": index,
                    "family": info["family"],
                    "checks": checks,
                    **values,
                }
    report = {
        "cases": 78,
        "train_development_final_unique_sources": len(seen),
        "model_sha256": training["model_sha256"],
        "max_metric_error": metric_max,
        "max_response_error": response_max,
        "worst_neural": worst,
        "summary": {
            key: {
                "accepted": sum(r["accepted"] for r in rows),
                "count": len(rows),
                "median_improvement_db": float(
                    np.median([r["improvement_db"] for r in rows])
                ),
                "worst_improvement_db": min(r["improvement_db"] for r in rows),
            }
            for key, rows in totals.items()
        },
        "adoption_recommended": False,
        "conclusion": "Response bounds can admit damage to late reflections. The neural mask fails the shift/structure-preservation gate; no general measured-IR acceptance.",
    }
    args.output.write_text(json.dumps(report, indent=2, allow_nan=False) + "\n")
    print(json.dumps({k: v for k, v in report.items() if k != "summary"}))


if __name__ == "__main__":
    main()
