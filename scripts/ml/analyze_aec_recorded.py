"""Independently reduce recorded-source AEC evidence with full complex FFTs.

Recomputes metrics from hashed source/native arrays instead of trusting saved
scores. Conditional source estimates hold mixture-derived gains fixed; they do
not identify damage caused by adaptive cancellation or establish device quality.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from scipy.fft import fft, ifft


def read(path, record):
    raw = path.read_bytes()
    if hashlib.sha256(raw).hexdigest() != record["sha256"]:
        raise ValueError(f"SHA mismatch: {path}")
    values = np.frombuffer(raw, dtype="<f4").astype(float)
    if not np.isfinite(values).all() or len(values) != record["floats"]:
        raise ValueError(f"invalid array: {path}")
    return values


def snr(target, residual):
    return float(
        10
        * np.log10(
            max(float(np.sum(target**2)), 1e-30)
            / max(float(np.sum(residual**2)), 1e-30)
        )
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--sources", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        raise ValueError("preserve previous analysis")
    protocol = json.loads((args.evidence / "protocol.json").read_text())
    summary = json.loads((args.evidence / "summary.json").read_text())
    for name, expected in protocol["source_sha256"].items():
        if (
            hashlib.sha256((Path(__file__).parent / name).read_bytes()).hexdigest()
            != expected
        ):
            raise ValueError("evaluation source changed after protocol freeze")
    if protocol["cases"] != 72 or summary["cases"] != 72:
        raise ValueError("incomplete recorded evaluation")
    maxima = {"spectrum": 0.0, "reconstruction": 0.0, "metric": 0.0}
    aggregates, identities, applied = {}, set(), 0
    for index in range(72):
        row = json.loads((args.evidence / f"case-{index:03d}.json").read_text())
        if row["index"] != index or summary["rows"][index]["quality"] != row["quality"]:
            raise ValueError("case/summary disagreement")
        provenance = row["provenance"]
        count, rate = provenance["frames"], provenance["rate"]
        source = args.sources / f"source_{index:03d}"
        arrays = {
            name: read(
                source / (name + ".f32le"),
                {
                    "sha256": provenance[name + "_sha256"],
                    "floats": count * (2 if name == "input" else 1),
                },
            )
            for name in ["near", "echo", "input"]
        }
        if (
            np.max(
                np.abs(
                    arrays["input"].reshape(-1, 2)[:, 0]
                    - arrays["echo"]
                    - arrays["near"]
                )
            )
            > 1e-7
        ):
            raise ValueError("source mixture decomposition mismatch")
        identities.add(provenance["input_sha256"])
        audio, near_outputs = {}, {}
        for method in ["classical", "hybrid"]:
            metadata = row["native"][method]
            directory = Path(metadata["directory"])
            native = {
                name: read(directory / (name + ".f32le"), metadata[name])
                for name in [
                    method,
                    "effective_gains",
                    "adaptive",
                    "error_spectra",
                    "features",
                    "targets",
                ]
            }
            if (
                metadata["frames"] != count
                or metadata["blocks"] != count // 256
                or metadata["latency_samples"] != 256
            ):
                raise ValueError("native geometry mismatch")
            unique = native["effective_gains"].reshape(-1, 257)
            if np.any(unique < 0) or np.any(unique > 1.000001):
                raise ValueError("unbounded gains")
            full_gain = np.concatenate([unique, unique[:, 255:0:-1]], axis=1)
            padded = np.pad(native["adaptive"].reshape(-1, 256), ((0, 0), (256, 0)))
            spectrum = fft(padded, axis=1)
            recorded = native["error_spectra"].reshape(-1, 257, 2)
            spectral_error = float(
                np.max(
                    np.abs(
                        spectrum[:, :257] - (recorded[:, :, 0] + 1j * recorded[:, :, 1])
                    )
                )
            )
            maxima["spectrum"] = max(maxima["spectrum"], spectral_error)
            if spectral_error > 5e-5:
                raise ValueError("error spectrum disagrees with adaptive output")
            reconstructed = ifft(spectrum * full_gain, axis=1).real[:, 256:].ravel()
            error = float(np.max(np.abs(reconstructed - native[method])))
            maxima["reconstruction"] = max(maxima["reconstruction"], error)
            if error > 1e-6:
                raise ValueError("full FFT reconstruction mismatch")
            clean = np.pad(arrays["near"].reshape(-1, 256), ((0, 0), (256, 0)))
            near_outputs[method] = (
                ifft(fft(clean, axis=1) * full_gain, axis=1).real[:, 256:].ravel()
            )
            audio[method] = native[method]
            if method == "hybrid":
                state = metadata["residual"]
                if (
                    not state["offline"]
                    or state["availability"] != "prepared"
                    or state["accepted_observations"] == 0
                ):
                    raise ValueError("learned stage unavailable")
                if (
                    state["native_blocks"] != count // 256
                    or state["applied_blocks"] + state["fallback_blocks"]
                    != state["native_blocks"]
                ):
                    raise ValueError("counter accounting failed")
                applied += state["applied_blocks"]
        for name in ["adaptive", "error_spectra", "features", "targets"]:
            if (
                row["native"]["classical"][name]["sha256"]
                != row["native"]["hybrid"][name]["sha256"]
            ):
                raise ValueError("adaptive observation changed")
        scene = provenance["scene"]
        if scene == "near_only" and not np.array_equal(
            audio["hybrid"], audio["classical"]
        ):
            raise ValueError("near-only fallback changed")
        windows = {"steady": (10 * rate, count), "onset": (6 * rate, 49 * rate // 8)}
        if scene in ["double_talk", "quiet_double_talk"]:
            windows["double_talk"] = (25 * rate // 4, 35 * rate // 4)
        elif scene == "near_only":
            windows = {"near_only": (0, count)}
        elif scene == "near_after_echo":
            windows["near_after_echo"] = (25 * rate // 4, count)
        if set(windows) != set(row["quality"]):
            raise ValueError("scored interval mismatch")
        for phase, (first, last) in windows.items():
            target = arrays["near"][first:last]
            energy = float(np.sum(target**2))
            expected_metrics = (
                {"erle_db"}
                if energy <= 1e-20
                else {
                    "near_snr_db",
                    "conditional_near_snr_db",
                    "conditional_near_projection_loss_db",
                }
            )
            if set(row["quality"][phase]) != expected_metrics:
                raise ValueError("missing or unexpected metric")
            for metric, stored in row["quality"][phase].items():
                values = {}
                for method in ["classical", "hybrid"]:
                    component = near_outputs[method][first:last]
                    output = audio[method][first:last]
                    if metric == "erle_db" and energy <= 1e-20:
                        value = snr(arrays["echo"][first:last], output)
                    elif metric == "near_snr_db" and energy > 1e-20:
                        value = snr(target, output - target)
                    elif metric == "conditional_near_snr_db" and energy > 1e-20:
                        value = snr(target, component - target)
                    elif (
                        metric == "conditional_near_projection_loss_db"
                        and energy > 1e-20
                    ):
                        value = float(
                            -20
                            * np.log10(
                                max(float(np.sum(component * target)) / energy, 1e-15)
                            )
                        )
                    else:
                        raise ValueError("unexpected metric")
                    values[method] = value
                    # SNR at machine precision is implementation-dependent;
                    # compare the residual energy instead above 120 dB.
                    delta = abs(value - stored[method])
                    if not (
                        metric.endswith("snr_db")
                        and value > 120
                        and stored[method] > 120
                    ):
                        maxima["metric"] = max(maxima["metric"], delta)
                        if delta > 1e-5:
                            raise ValueError(
                                f"metric mismatch: {index} {phase} {metric} {delta}"
                            )
                if (
                    abs(
                        stored["hybrid"]
                        - stored["classical"]
                        - stored["hybrid_minus_classical_db"]
                    )
                    > 1e-10
                ):
                    raise ValueError("stored delta mismatch")
                # Keep saved double-precision metrics after independent agreement.
                aggregates.setdefault(phase + "/" + metric, []).append(
                    {"case": index, **stored}
                )
    if len(identities) != 72 or applied != summary["applied_blocks"]:
        raise ValueError("summary counts mismatch")
    report = {
        "cases": 72,
        "unique_mixtures": len(identities),
        "applied_blocks": applied,
        "max_abs_errors": maxima,
        "metrics": {
            key: {
                "count": len(rows),
                "worst_delta": min(rows, key=lambda r: r["hybrid_minus_classical_db"]),
                "largest_delta": max(
                    rows, key=lambda r: r["hybrid_minus_classical_db"]
                ),
                "negative_deltas": sum(
                    r["hybrid_minus_classical_db"] < -1e-5 for r in rows
                ),
            }
            for key, rows in aggregates.items()
        },
        "conclusion": "Experimental only: recorded-source onset and double-talk damage prevents general adoption despite echo reduction. Conditional near estimates exclude adaptive cancellation damage; physical-device and multilingual acceptance remain open.",
    }
    args.output.write_text(json.dumps(report, indent=2, allow_nan=False) + "\n")
    print(json.dumps({k: v for k, v in report.items() if k != "metrics"}))


if __name__ == "__main__":
    main()
