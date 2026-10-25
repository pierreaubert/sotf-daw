"""Native Ambisonics auditions and explicit diffuse-energy correction constraints."""

import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np


def preset(**changes):
    controls = {
        "order": 1,
        "target_layout": "5.1",
        "max_re_weighting": True,
        "dual_band": False,
        "algorithm": "mode_matching",
    }
    if set(changes) - (set(controls) | {"custom_layout"}):
        raise ValueError("Unknown native decoder controls")
    return dict(controls, **changes)


def diffuse_weights(matrix):
    """Per-speaker mean energy for uniformly distributed unit SN3D plane waves.

    ACN degree l has ensemble covariance 1/(2*l+1). These are angular
    electrical-energy weights, not a room, loudness or localization model.
    """
    d = np.asarray(matrix, dtype=float)
    if d.ndim != 2 or not d.size or not np.isfinite(d).all():
        raise ValueError("Expected a finite decoder matrix")
    root = int(np.sqrt(d.shape[1]))
    if root**2 != d.shape[1] or not 2 <= root <= 8:
        raise ValueError("Expected order1..7 ACN columns")
    degree = np.floor(np.sqrt(np.arange(d.shape[1]))).astype(int)
    return np.sum(d**2 / (2 * degree + 1), axis=1)


def energy_report(matrices, gains):
    g = np.asarray(gains, dtype=float)
    if (
        g.ndim != 1
        or not g.size
        or not np.isfinite(g).all()
        or np.any((g < 0.8) | (g > 1.25))
    ):
        raise ValueError("Speaker gains must be finite within[0.8,1.25]")
    matrices = [np.asarray(d, dtype=float) for d in matrices]
    result = []
    for matrix in matrices:
        w = diffuse_weights(matrix)
        if w.shape != g.shape or w.sum() <= 0:
            raise ValueError("Correction and decoder geometry mismatch")
        result.append(
            {
                "baseline": float(w.sum()),
                "corrected": float(w @ g**2),
                "ratio": float(w @ g**2 / w.sum()),
                "per_speaker": w.tolist(),
            }
        )
    if not result:
        raise ValueError("At least one decoder matrix is required")
    cross_terms = []
    for i, a in enumerate(matrices):
        for j, b in enumerate(matrices[:i]):
            degree = np.floor(np.sqrt(np.arange(a.shape[1]))).astype(int)
            if a.shape != b.shape:
                raise ValueError("Decoder band geometry mismatch")
            w = np.sum(a * b / (2 * degree + 1), axis=1)
            scale = np.sqrt(result[i]["baseline"] * result[j]["baseline"])
            cross_terms.append(
                {
                    "bands": [i, j],
                    "baseline": float(w.sum()),
                    "corrected": float(w @ g**2),
                    "per_speaker": w.tolist(),
                    "normalized_error": float((w @ g**2 - w.sum()) / scale),
                }
            )
    return {
        "bands": result,
        "cross_terms": cross_terms,
        "arbitrary_output_energy_ratio_bounds": [
            float(g.min() ** 2),
            float(g.max() ** 2),
        ],
    }


def normalize_gains(matrices, desired_gains, lfe_channels=()):
    """Project squared gains onto exact diffuse-energy constraints for every band.

    This deterministic offline projection preserves LF, HF and their cross term
    for a dual-band decoder, including the crossover overlap. It rejects gains
    outside[0.8,1.25] after projection;
    it does not silently clip, infer preferences or change normative matrices.
    """
    g = np.asarray(desired_gains, dtype=float)
    if (
        g.ndim != 1
        or not g.size
        or not np.isfinite(g).all()
        or np.any((g < 0.8) | (g > 1.25))
    ):
        raise ValueError("Expected bounded desired speaker gains")
    w = np.array([diffuse_weights(d) for d in matrices])
    if w.ndim != 2 or w.shape[1] != len(g) or np.any(w.sum(axis=1) <= 0):
        raise ValueError("Invalid decoder energy constraints")
    if any(type(c) is not int or not 0 <= c < len(g) for c in lfe_channels):
        raise ValueError("Invalid LFE channel indices")
    if any(np.any(w[:, c] != 0) for c in lfe_channels):
        raise ValueError("LFE decoder rows must be silent")
    report = energy_report(matrices, np.ones(len(g)))
    constraints = list(w / w.sum(axis=1, keepdims=True))
    for cross in report["cross_terms"]:
        i, j = cross["bands"]
        scale = np.sqrt(w[i].sum() * w[j].sum())
        constraints.append(np.array(cross["per_speaker"]) / scale)
    w = np.array(constraints)
    h = g**2
    h -= np.linalg.pinv(w, rcond=1e-10) @ (w @ (h - 1))
    for c in lfe_channels:
        h[c] = 1
    if np.any(h <= 0):
        raise ValueError("Energy normalization requires nonpositive squared gains")
    normalized = np.sqrt(h).astype(np.float32)
    report = energy_report(matrices, normalized)
    if any(abs(b["ratio"] - 1) > 2e-6 for b in report["bands"]) or any(
        abs(c["normalized_error"]) > 2e-6 for c in report["cross_terms"]
    ):
        raise ValueError("Diffuse-energy normalization failed")
    return normalized


def render(signal, renderer, work, params, gains, rate=48000, callback=193, zeros=0):
    x = np.asarray(signal, dtype=np.float32)
    g = np.asarray(gains, dtype=np.float32)
    order = params.get("order")
    if (
        type(order) is not int
        or not 1 <= order <= 7
        or x.ndim != 2
        or x.shape[1] != (order + 1) ** 2
        or not len(x)
        or not np.isfinite(x).all()
        or np.max(abs(x)) > 32
        or g.ndim != 1
        or not 1 <= len(g) <= 64
        or not np.isfinite(g).all()
        or np.any((g < np.float32(0.8)) | (g > 1.25))
        or type(rate) is not int
        or not 44100 <= rate <= 192000
        or type(callback) is not int
        or not 1 <= callback <= 4096
        or type(zeros) is not int
        or not 0 <= zeros <= 65536
        or (len(x) + zeros) * (x.shape[1] + len(g)) > 16_000_000
    ):
        raise ValueError("Unsupported finite bounded Ambisonics render")
    work = Path(work)
    work.mkdir(parents=True, exist_ok=False)
    raw = x.astype("<f4").tobytes()
    (work / "input.f32le").write_bytes(raw)
    request = {
        "input": str((work / "input.f32le").resolve()),
        "output": str((work / "output.f32le").resolve()),
        "sample_rate": rate,
        "frames": len(x),
        "callback_frames": callback,
        "appended_zero_frames": zeros,
        "normalization": "ACN/SN3D",
        "params": params,
        "speaker_gains": g.tolist(),
    }
    path = work / "request.json"
    path.write_text(json.dumps(request, allow_nan=False) + "\n")
    result = subprocess.run(
        [str(Path(renderer).resolve()), str(path)],
        capture_output=True,
        text=True,
        check=False,
        timeout=120,
    )
    (work / "stderr.txt").write_text(result.stderr)
    if result.returncode:
        raise ValueError("Native Ambisonics render failed: " + result.stderr)
    m = json.loads(result.stdout)
    expected = {
        "status": "ok",
        "normalization": "ACN/SN3D",
        "sample_rate": rate,
        "input_channels": x.shape[1],
        "output_channels": len(g),
        "input_frames": len(x),
        "output_frames": len(x) + zeros,
        "callback_frames": callback,
        "appended_zero_frames": zeros,
        "latency_samples": 0,
        "tail": "unknown_recursive" if params["dual_band"] else "finite_zero",
    }

    def canonical_controls(controls):
        # Geometry uses f32 in the native preset, including JSON reopening.
        result = json.loads(json.dumps(controls, allow_nan=False))
        if "custom_layout" in result:
            for speaker in result["custom_layout"]["speakers"]:
                for angle in ["azimuth_deg", "elevation_deg"]:
                    speaker[angle] = float(np.float32(speaker[angle]))
        return result

    if any(m.get(k) != v for k, v in expected.items()) or canonical_controls(
        m.get("effective_params")
    ) != canonical_controls(params):
        raise ValueError("Native decoder context mismatch")
    if not np.array_equal(np.asarray(m["speaker_gains"], dtype=np.float32), g):
        raise ValueError("Native correction mismatch")
    matrices = []
    for name in ["decoder_matrix"] + (["basic_matrix"] if params["dual_band"] else []):
        d = m[name]
        if d["ambi_channels"] != x.shape[1] or d["speaker_count"] != len(g):
            raise ValueError("Native matrix geometry mismatch")
        matrices.append(
            np.asarray(d["matrix"], dtype=float).reshape(len(g), x.shape[1])
        )
    output = work / "output.f32le"
    if output.stat().st_size != (len(x) + zeros) * len(g) * 4:
        raise ValueError("Native PCM byte count mismatch")
    y = np.fromfile(output, dtype="<f4").reshape(-1, len(g))
    if not np.isfinite(y).all():
        raise ValueError("Native output must be finite")
    m.update(
        energy=energy_report(matrices, g),
        input_sha256=hashlib.sha256(raw).hexdigest(),
        output_sha256=hashlib.sha256(output.read_bytes()).hexdigest(),
    )
    (work / "metadata.json").write_text(json.dumps(m, indent=2, allow_nan=False) + "\n")
    return y, m
