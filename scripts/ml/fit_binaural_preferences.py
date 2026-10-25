#!/usr/bin/env python3
"""Offline categorical HRTF preferences, qualified by actual native auditions."""

import argparse
import hashlib
import json
import math
from pathlib import Path

import numpy as np
from binaural_native import MAX_SAMPLES, MAX_SOFA_BYTES, SUPPORTED_CHANNELS, render
from fit_crossfeed_preferences import identifier
from preference_ranking import fit_preferences

FORMAT = "sotf-binaural-preferences-v1"
REOPEN_MAX_ABS_ERROR = 1e-7  # Matches the native renderer callback-invariance gate.


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def bounded_file(path, limit, expected):
    with Path(path).open("rb") as source:
        raw = source.read(limit + 1)
    if not 1 <= len(raw) <= limit or digest(raw) != expected:
        raise ValueError("File size or SHA256 mismatch: " + str(path))
    return raw


def hash_value(value):
    if (
        not isinstance(value, str)
        or len(value) != 64
        or any(c not in "0123456789abcdef" for c in value)
    ):
        raise ValueError("Use lowercase SHA256 identities")
    return value


def number(value, low, high):
    if (
        isinstance(value, bool)
        or not isinstance(value, (int, float))
        or not math.isfinite(value)
        or not low <= value <= high
    ):
        raise ValueError("Value outside finite declared range")
    return value


def validate(data):
    if not isinstance(data, dict) or data.get("format") != FORMAT:
        raise ValueError("Unsupported HRTF preference session")
    if data.get("data_origin") not in {"listener_observations", "synthetic_validation"}:
        raise ValueError("Declare listener observations or synthetic validation")
    context = data["context"]
    for key in ["listener_id", "headphones_id", "headphone_eq_id", "session_id"]:
        identifier(context[key])
    if context["criterion"] != "overall_preference" or context["head_pose_deg"] != [
        0,
        0,
        0,
    ]:
        raise ValueError("This session fits fixed-zero-pose overall preference only")
    if (
        type(context["sample_rate"]) is not int
        or not 44100 <= context["sample_rate"] <= 192000
    ):
        raise ValueError("Unsupported sample rate")
    if (
        type(context["input_channels"]) is not int
        or context["input_channels"] not in SUPPORTED_CHANNELS
    ):
        raise ValueError("Unsupported speaker layout")
    n = context["fft_size"]
    if (
        type(n) is not int
        or not 512 <= n <= 8192
        or n & (n - 1)
        or type(context["diffuse_field_eq"]) is not bool
    ):
        raise ValueError("Unsupported native HRTF design")
    hash_value(context["renderer_sha256"])
    protocol = data["protocol"]
    if any(
        protocol.get(k) is not True
        for k in [
            "blinded",
            "randomized_order",
            "fixed_head_pose",
            "whole_source_grouping",
        ]
    ):
        raise ValueError("Declare blinded randomized fixed-pose whole-source auditions")
    number(protocol["maximum_level_mismatch_db"], 0, 0.25)
    candidates = data["candidates"]
    if not isinstance(candidates, list) or not 2 <= len(candidates) <= 12:
        raise ValueError("Supply 2..12 auditioned SOFA candidates")
    ids, hashes = [], set()
    for c in candidates:
        ids.append(identifier(c["id"]))
        params = c["parameters"]
        if (
            set(params) != {"sofa_file", "sofa_sha256"}
            or not isinstance(params["sofa_file"], str)
            or not params["sofa_file"]
        ):
            raise ValueError("Use an explicit SOFA path and SHA256")
        h = hash_value(params["sofa_sha256"])
        if h in hashes:
            raise ValueError("Duplicate SOFA candidate bytes")
        hashes.add(h)
    if len(set(ids)) != len(ids) or data["baseline_id"] not in ids:
        raise ValueError("Unique candidates must include the fallback baseline")
    programmes = data["programmes"]
    if not isinstance(programmes, list) or not 1 <= len(programmes) <= 32:
        raise ValueError("Supply 1..32 independently grouped programmes")
    sources, source_hashes, total = set(), set(), 0
    for p in programmes:
        pid = identifier(p["id"])
        h = hash_value(p["audio_sha256"])
        if pid in sources or h in source_hashes:
            raise ValueError("Duplicate source group or source audio bytes")
        sources.add(pid)
        source_hashes.add(h)
        if type(p["frames"]) is not int or p["frames"] < context["sample_rate"] // 4:
            raise ValueError("Programmes must contain at least 250 ms")
        total += p["frames"] * context["input_channels"]
        if not isinstance(p["audio_file"], str) or not p["audio_file"]:
            raise ValueError("Use explicit raw PCM paths")
        if set(p["post_gains_db"]) != set(ids):
            raise ValueError(
                "Record each candidate's actual audition trim for every programme"
            )
        for gain in p["post_gains_db"].values():
            number(gain, -24, 24)
    if total > MAX_SAMPLES:
        raise ValueError("Session exceeds 4 million aggregate programme samples")
    trials = data["trials"]
    if not isinstance(trials, list) or not 1 <= len(trials) <= 4096:
        raise ValueError("Supply 1..4096 preference trials")
    trial_ids, groups, design, labels, orientations = set(), [], [], [], {}
    used_sources = set()
    for t in trials:
        tid = identifier(t["id"])
        if tid in trial_ids or t["programme_id"] not in sources:
            raise ValueError("Duplicate trial or unknown programme")
        trial_ids.add(tid)
        groups.append(t["programme_id"])
        used_sources.add(t["programme_id"])
        if t["a"] not in ids or t["b"] not in ids or t["a"] == t["b"]:
            raise ValueError("Compare two declared HRTFs")
        a, b = ids.index(t["a"]), ids.index(t["b"])
        row = np.zeros(len(ids) + 1)
        row[a], row[b], row[-1] = 1, -1, 1
        design.append(row)
        if t["choice"] not in {"a", "b", "tie"}:
            raise ValueError("Choose a, b or tie")
        labels.append({"a": 1.0, "b": 0.0, "tie": 0.5}[t["choice"]])
        orientations.setdefault(tuple(sorted((a, b))), set()).add(a < b)
    if used_sources != sources:
        raise ValueError("Every programme must have recorded trials")
    return ids, np.asarray(design), np.asarray(labels), np.asarray(groups), orientations


def fit(data):
    return fit_preferences(data, validate, "sotf-bt-binaural-v1")


def prepare(data, renderer):
    """Read and verify bounded candidate and audio snapshots before rendering."""
    validate(data)
    bounded_file(renderer, 128 * 1024 * 1024, data["context"]["renderer_sha256"])
    sofas = {
        c["id"]: bounded_file(
            c["parameters"]["sofa_file"], MAX_SOFA_BYTES, c["parameters"]["sofa_sha256"]
        )
        for c in data["candidates"]
    }
    programmes = {}
    channels = data["context"]["input_channels"]
    for p in data["programmes"]:
        size = p["frames"] * channels * 4
        raw = bounded_file(p["audio_file"], size, p["audio_sha256"])
        if len(raw) != size:
            raise ValueError("PCM size differs from programme geometry")
        x = np.frombuffer(raw, dtype="<f4").reshape(-1, channels)
        if not np.isfinite(x).all() or np.max(np.abs(x)) > 32:
            raise ValueError("Programmes must be finite bounded PCM")
        programmes[p["id"]] = x
    return sofas, programmes


def qualify(data, renderer, work, snapshots):
    """Render all declared auditions; reject clipping, silence and level mismatches."""
    sofas, programmes = snapshots
    work = Path(work)
    work.mkdir(parents=True, exist_ok=False)
    paths = {}
    for i, c in enumerate(data["candidates"]):
        path = work / f"candidate-{i}.sofa"
        path.write_bytes(sofas[c["id"]])
        paths[c["id"]] = path
    ctx = data["context"]
    rows, outputs, settings = [], {}, {}
    reasons = []
    for pi, p in enumerate(data["programmes"]):
        rms = []
        for ci, c in enumerate(data["candidates"]):
            y, meta = render(
                programmes[p["id"]],
                renderer,
                paths[c["id"]],
                work / f"render-{pi}-{ci}",
                rate=ctx["sample_rate"],
                fft_size=ctx["fft_size"],
                eq=ctx["diffuse_field_eq"],
            )
            y = y.astype(float) * 10 ** (p["post_gains_db"][c["id"]] / 20)
            power = float(np.mean(y * y))
            peak = float(np.max(np.abs(y)))
            if power <= 1e-12:
                reasons.append("inaudible_or_silent_programme")
            if peak > 1:
                reasons.append("audition_sample_clipping_risk")
            rms.append(10 * math.log10(max(power, 1e-30)))
            rows.append(
                {
                    "programme_id": p["id"],
                    "candidate_id": c["id"],
                    "rms_dbfs": rms[-1],
                    "peak": peak,
                    "post_gain_db": p["post_gains_db"][c["id"]],
                    "native_metadata": meta,
                }
            )
            outputs[(p["id"], c["id"])] = y
            settings[c["id"]] = meta["requested_params"]
        if max(rms) - min(rms) > data["protocol"]["maximum_level_mismatch_db"] + 1e-6:
            reasons.append("native_auditions_not_rms_level_matched")
    # Different SOFA bytes are insufficient if all actual auditions coincide.
    for i, a in enumerate(data["candidates"]):
        for b in data["candidates"][i + 1 :]:
            if all(
                outputs[(p["id"], a["id"])].shape == outputs[(p["id"], b["id"])].shape
                and np.max(
                    np.abs(outputs[(p["id"], a["id"])] - outputs[(p["id"], b["id"])])
                )
                <= 1e-7
                for p in data["programmes"]
            ):
                reasons.append("effectively_duplicate_native_auditions")
    return {
        "accepted": not reasons,
        "reasons": sorted(set(reasons)),
        "auditions": rows,
        "settings": settings,
    }, paths


def run(data, renderer, output):
    snapshots = prepare(data, renderer)
    output = Path(output)
    output.mkdir(parents=True, exist_ok=False)
    native, _paths = qualify(data, renderer, output / "native", snapshots)
    report = fit(data)
    if not native["accepted"]:
        report.update(accepted=False, selected_id=None, parameters=None)
        report["reasons"] += native["reasons"]
    report.update(
        native_validation=native,
        context=data["context"],
        qualification="Conditional fixed-pose overall preference only; self-reported playback protocol; bootstrap stability is not calibrated population confidence; RMS matching is not perceived-loudness matching",
    )
    if report["accepted"]:
        chosen = report["selected_id"]
        selected = output / "selected.sofa"
        selected.write_bytes(snapshots[0][chosen])
        preset = dict(native["settings"][chosen], hrtf_file=str(selected.resolve()))
        # Reopen the exported preset's SOFA at another callback partition.
        errors = []
        for i, p in enumerate(data["programmes"]):
            y, meta = render(
                snapshots[1][p["id"]],
                renderer,
                selected,
                output / f"reopen-{i}",
                rate=data["context"]["sample_rate"],
                fft_size=data["context"]["fft_size"],
                callback=17,
                eq=data["context"]["diffuse_field_eq"],
            )
            reopened_params = dict(
                meta["requested_params"], hrtf_file=preset["hrtf_file"]
            )
            if reopened_params != preset:
                raise ValueError(
                    "Reopened native parameters differ from exported preset"
                )
            old = np.fromfile(
                output
                / "native"
                / f"render-{i}-{[c['id'] for c in data['candidates']].index(chosen)}"
                / "output.f32le",
                dtype="<f4",
            ).reshape(-1, 2)[meta["latency_samples"] :]
            error = (
                float(np.max(np.abs(y.astype(float) - old)))
                if y.shape == old.shape
                else math.inf
            )
            if error > REOPEN_MAX_ABS_ERROR:
                raise ValueError("Selected HRTF reopening changed native audio")
            errors.append(error)
        report["reopen_max_errors"] = errors
        (output / "preset.json").write_text(
            json.dumps(preset, indent=2, allow_nan=False) + "\n"
        )
        (output / "audition_setup.json").write_text(
            json.dumps(
                {
                    "context": data["context"],
                    "programmes": data["programmes"],
                    "selected_id": chosen,
                    "note": "Per-programme audition trims remain separate; preset alone does not apply them or calibrate headphone SPL.",
                },
                indent=2,
                allow_nan=False,
            )
            + "\n"
        )
    (output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--session", type=Path, required=True)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    with args.session.open("rb") as f:
        raw = f.read(2 * 1024 * 1024 + 1)
    if len(raw) > 2 * 1024 * 1024:
        raise ValueError("Session exceeds 2 MiB")
    data = json.loads(raw)
    report = run(data, args.renderer, args.output)
    report["session_sha256"] = digest(raw)
    (args.output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    print(
        json.dumps(
            {
                "accepted": report["accepted"],
                "selected_id": report["selected_id"],
                "reasons": report["reasons"],
            }
        )
    )
    if not report["accepted"]:
        raise SystemExit(2)


if __name__ == "__main__":
    main()
