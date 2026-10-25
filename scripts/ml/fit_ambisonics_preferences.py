"""Learn offline preferences among auditioned energy-constrained speaker corrections."""

import argparse
import json
import math
from pathlib import Path

import numpy as np
from ambisonics_native import render
from fit_binaural_preferences import bounded_file, digest, hash_value, number
from fit_crossfeed_preferences import identifier
from preference_ranking import fit_preferences

FORMAT = "sotf-ambisonics-layout-preferences-v1"
NAMED_OUTPUTS = {
    "5.1": 6,
    "7.1": 8,
    "5.1.2": 8,
    "5.1.4": 10,
    "7.1.2": 10,
    "7.1.4": 12,
    "9.1.4": 14,
    "9.1.6": 16,
}
ZEROS = 4096  # Observation input, not a claim of a finite dual-band tail.
ENERGY_TOLERANCE = 2e-6
REOPEN_TOLERANCE = 1e-7


def decoder_geometry(params):
    keys = {"order", "target_layout", "max_re_weighting", "dual_band", "algorithm"}
    if not isinstance(params, dict):
        raise TypeError("Supply complete native decoder controls")
    custom = params.get("target_layout") == "custom"
    if set(params) != keys | ({"custom_layout"} if custom else set()):
        raise ValueError("Supply exact native decoder controls")
    order = params["order"]
    if type(order) is not int or not 1 <= order <= 7:
        raise ValueError("Unsupported Ambisonics order")
    if any(type(params[k]) is not bool for k in ["max_re_weighting", "dual_band"]):
        raise ValueError("Decoder switches must be booleans")
    if params["algorithm"] not in {"mode_matching", "allrad"}:
        raise ValueError("Unsupported native decoder algorithm")
    if custom:
        layout = params["custom_layout"]
        if not isinstance(layout, dict) or set(layout) != {"name", "speakers"}:
            raise ValueError("Supply complete custom geometry")
        if not isinstance(layout["name"], str) or not 1 <= len(layout["name"]) <= 64:
            raise ValueError("Invalid custom layout name")
        speakers = layout["speakers"]
        if not isinstance(speakers, list) or not 1 <= len(speakers) <= 64:
            raise ValueError("Supply1..64 custom speakers")
        labels, lfe = set(), []
        for i, s in enumerate(speakers):
            if not isinstance(s, dict) or set(s) != {
                "label",
                "azimuth_deg",
                "elevation_deg",
                "is_lfe",
            }:
                raise ValueError("Supply exact native speaker fields")
            label = s["label"]
            if (
                not isinstance(label, str)
                or not 1 <= len(label) <= 16
                or label in labels
            ):
                raise ValueError("Supply unique nonempty speaker labels")
            labels.add(label)
            number(s["azimuth_deg"], -180, 180)
            number(s["elevation_deg"], -90, 90)
            if type(s["is_lfe"]) is not bool:
                raise ValueError("LFE flag must be boolean")
            if s["is_lfe"]:
                lfe.append(i)
        if len(lfe) == len(speakers):
            raise ValueError("At least one directional speaker is required")
        outputs = len(speakers)
    else:
        if params["target_layout"] not in NAMED_OUTPUTS:
            raise ValueError("Unsupported registered decoder layout")
        outputs, lfe = NAMED_OUTPUTS[params["target_layout"]], [3]
    return (order + 1) ** 2, outputs, lfe


def validate(data):
    if not isinstance(data, dict) or data.get("format") != FORMAT:
        raise ValueError("Unsupported Ambisonics preference session")
    if data.get("data_origin") not in {"listener_observations", "synthetic_validation"}:
        raise ValueError("Declare observed or synthetic preferences")
    ctx = data["context"]
    for k in [
        "listener_id",
        "playback_setup_id",
        "room_id",
        "calibration_id",
        "session_id",
    ]:
        identifier(ctx[k])
    if ctx["criterion"] != "overall_preference" or ctx["normalization"] != "ACN/SN3D":
        raise ValueError("Use fixed-setup overall preference and ACN/SN3D")
    rate = ctx["sample_rate"]
    if type(rate) is not int or not 44100 <= rate <= 192000:
        raise ValueError("Unsupported sample rate")
    hash_value(ctx["renderer_sha256"])
    inputs, outputs, lfe = decoder_geometry(ctx["decoder_parameters"])
    protocol = data["protocol"]
    if any(
        protocol.get(k) is not True
        for k in [
            "blinded",
            "randomized_order",
            "whole_source_grouping",
            "fixed_listener_position",
        ]
    ):
        raise ValueError(
            "Declare blinded randomized whole-source fixed-position auditions"
        )
    number(protocol["maximum_level_mismatch_db"], 0, 0.25)
    candidates = data["candidates"]
    if not isinstance(candidates, list) or not 2 <= len(candidates) <= 12:
        raise ValueError("Supply2..12 auditioned correction candidates")
    ids, settings = [], set()
    for c in candidates:
        ids.append(identifier(c["id"]))
        p = c["parameters"]
        if not isinstance(p, dict) or set(p) != {"speaker_gains"}:
            raise ValueError("Correction candidates contain only speaker_gains")
        gains = p["speaker_gains"]
        if not isinstance(gains, list) or len(gains) != outputs:
            raise ValueError("Correction gain count differs from speaker geometry")
        for gain in gains:
            number(gain, 0.8, 1.25)
        key = tuple(np.asarray(gains, dtype=np.float32))
        if key in settings or any(key[i] != 1 for i in lfe):
            raise ValueError("Duplicate native correction or nonunity LFE gain")
        settings.add(key)
    if len(set(ids)) != len(ids) or data["baseline_id"] not in ids:
        raise ValueError(
            "Unique candidates must include the unchanged decoder baseline"
        )
    baseline = candidates[ids.index(data["baseline_id"])]["parameters"]["speaker_gains"]
    if any(g != 1 for g in baseline):
        raise ValueError("Fallback must be the uncorrected native decoder")
    programmes = data["programmes"]
    if not isinstance(programmes, list) or not 1 <= len(programmes) <= 32:
        raise ValueError("Supply1..32 whole-source programmes")
    sources, hashes, total, budget = set(), set(), 0, 0
    for p in programmes:
        pid = identifier(p["id"])
        h = hash_value(p["audio_sha256"])
        if pid in sources or h in hashes:
            raise ValueError("Duplicate programme group or PCM bytes")
        sources.add(pid)
        hashes.add(h)
        frames = p["frames"]
        if type(frames) is not int or frames < math.ceil(rate / 4):
            raise ValueError("Auditions require at least250ms of actual source PCM")
        samples = (frames + ZEROS) * (inputs + outputs)
        if samples > 16_000_000:
            raise ValueError("Programme exceeds native combined sample bound")
        total += frames * inputs
        budget += samples * len(candidates)
        if not isinstance(p["audio_file"], str) or not p["audio_file"]:
            raise ValueError("Use explicit PCM paths")
        if set(p["post_gains_db"]) != set(ids):
            raise ValueError("Record every candidate actual audition trim")
        for gain in p["post_gains_db"].values():
            number(gain, -24, 24)
    if total > 32_000_000 or budget > 128_000_000:
        raise ValueError(
            "Session exceeds aggregate32M input or128M rendered sample budget"
        )
    trials = data["trials"]
    if not isinstance(trials, list) or not 1 <= len(trials) <= 4096:
        raise ValueError("Supply1..4096 preference trials")
    tids, groups, design, labels, orientations, used = set(), [], [], [], {}, set()
    for t in trials:
        tid = identifier(t["id"])
        if tid in tids or t["programme_id"] not in sources:
            raise ValueError("Duplicate trial or unknown programme")
        tids.add(tid)
        groups.append(t["programme_id"])
        used.add(t["programme_id"])
        if t["a"] not in ids or t["b"] not in ids or t["a"] == t["b"]:
            raise ValueError("Compare two declared correction candidates")
        a, b = ids.index(t["a"]), ids.index(t["b"])
        row = np.zeros(len(ids) + 1)
        row[a], row[b], row[-1] = 1, -1, 1
        design.append(row)
        if t["choice"] not in {"a", "b", "tie"}:
            raise ValueError("Choose a,b or tie")
        labels.append({"a": 1.0, "b": 0.0, "tie": 0.5}[t["choice"]])
        orientations.setdefault(tuple(sorted((a, b))), set()).add(a < b)
    if used != sources:
        raise ValueError("Every programme must have trials")
    return ids, np.asarray(design), np.asarray(labels), np.asarray(groups), orientations


def fit(data):
    return fit_preferences(data, validate, "sotf-bt-ambisonics-layout-v1")


def prepare(data, renderer):
    validate(data)
    bounded_file(renderer, 128 * 1024 * 1024, data["context"]["renderer_sha256"])
    inputs, _, _ = decoder_geometry(data["context"]["decoder_parameters"])
    result = {}
    for p in data["programmes"]:
        size = p["frames"] * inputs * 4
        raw = bounded_file(p["audio_file"], size, p["audio_sha256"])
        if len(raw) != size:
            raise ValueError("PCM bytes differ from programme geometry")
        x = np.frombuffer(raw, dtype="<f4").reshape(-1, inputs)
        if not np.isfinite(x).all() or np.max(abs(x)) > 32:
            raise ValueError("Supply finite bounded source PCM")
        result[p["id"]] = x
    return result


def qualify(data, renderer, work, snapshots):
    work = Path(work)
    work.mkdir(parents=True, exist_ok=False)
    ctx = data["context"]
    rows, settings, reasons = [], {}, []
    differences = {
        (a["id"], b["id"]): 0.0
        for i, a in enumerate(data["candidates"])
        for b in data["candidates"][i + 1 :]
    }
    for pi, p in enumerate(data["programmes"]):
        levels, paths = [], {}
        for ci, c in enumerate(data["candidates"]):
            path = work / f"render-{pi}-{ci}"
            y, m = render(
                snapshots[p["id"]],
                renderer,
                path,
                ctx["decoder_parameters"],
                c["parameters"]["speaker_gains"],
                rate=ctx["sample_rate"],
                zeros=ZEROS,
            )
            error = max(
                [abs(b["ratio"] - 1) for b in m["energy"]["bands"]]
                + [abs(t["normalized_error"]) for t in m["energy"]["cross_terms"]]
            )
            if error > ENERGY_TOLERANCE:
                reasons.append("candidate_diffuse_energy_not_normalized")
            raw_peak = float(np.max(abs(y)))
            audition = y.astype(float) * 10 ** (p["post_gains_db"][c["id"]] / 20)
            power = float(np.mean(audition**2))
            peak = float(np.max(abs(audition)))
            if power <= 1e-12:
                reasons.append("inaudible_or_silent_programme")
            if max(raw_peak, peak) > 0.999:
                reasons.append("candidate_or_audition_sample_headroom")
            levels.append(10 * math.log10(max(power, 1e-30)))
            rows.append(
                {
                    "programme_id": p["id"],
                    "candidate_id": c["id"],
                    "rms_dbfs": levels[-1],
                    "sample_peak": peak,
                    "raw_sample_peak": raw_peak,
                    "post_gain_db": p["post_gains_db"][c["id"]],
                    "energy_error": error,
                    "native_metadata_file": str((path / "metadata.json").resolve()),
                    "input_sha256": m["input_sha256"],
                    "output_sha256": m["output_sha256"],
                }
            )
            settings[c["id"]] = {
                "decoder": m["effective_params"],
                "matrix": m["correction_params"],
            }
            paths[c["id"]] = path / "output.f32le"
        if (
            max(levels) - min(levels)
            > data["protocol"]["maximum_level_mismatch_db"] + 1e-6
        ):
            reasons.append("native_auditions_not_rms_level_matched")
        # Read at most two candidate PCM arrays at a time, not an entire session.
        for pair, difference in differences.items():
            a, b = pair
            x = np.fromfile(paths[a], dtype="<f4").astype(float) * 10 ** (
                p["post_gains_db"][a] / 20
            )
            y = np.fromfile(paths[b], dtype="<f4").astype(float) * 10 ** (
                p["post_gains_db"][b] / 20
            )
            differences[pair] = max(difference, float(np.max(abs(x - y))))
    if any(d <= 1e-7 for d in differences.values()):
        reasons.append("effectively_duplicate_native_auditions")
    return {
        "accepted": not reasons,
        "reasons": sorted(set(reasons)),
        "auditions": rows,
        "settings": settings,
    }


def run(data, renderer, output):
    snapshots = prepare(data, renderer)
    output = Path(output)
    output.mkdir(parents=True, exist_ok=False)
    native = qualify(data, renderer, output / "native", snapshots)
    bounded_file(renderer, 128 * 1024 * 1024, data["context"]["renderer_sha256"])
    report = fit(data)
    if not native["accepted"]:
        report.update(accepted=False, selected_id=None, parameters=None)
        report["reasons"] += native["reasons"]
    report.update(
        native_validation=native,
        context=data["context"],
        automatic_apply=False,
        qualification="Conditional overall preference for this fixed listener/setup/decoder and auditioned candidates. Self-reported protocol; grouped bootstrap stability is not calibrated population confidence; RMS matching is not perceived loudness or acoustic calibration.",
    )
    if report["accepted"]:
        chosen = report["selected_id"]
        settings = native["settings"][chosen]
        chain = {
            "plugins": [
                {
                    "plugin_type": "ambisonics_decoder",
                    "parameters": settings["decoder"],
                },
                {"plugin_type": "matrix", "parameters": settings["matrix"]},
            ]
        }
        reopened = json.loads(json.dumps(chain, allow_nan=False))
        ci = [c["id"] for c in data["candidates"]].index(chosen)
        errors = []
        for i, p in enumerate(data["programmes"]):
            params = reopened["plugins"][0]["parameters"]
            matrix = reopened["plugins"][1]["parameters"]
            count = len(matrix["input_channel_map"])
            gains = np.array(matrix["matrix"]).reshape(count, count).diagonal()
            y, m = render(
                snapshots[p["id"]],
                renderer,
                output / f"reopen-{i}",
                params,
                gains,
                rate=data["context"]["sample_rate"],
                callback=17,
                zeros=ZEROS,
            )
            if m["correction_params"] != matrix or m["effective_params"] != params:
                raise ValueError(
                    "Reopened native controls differ from exported controls"
                )
            old = np.fromfile(
                output / "native" / f"render-{i}-{ci}" / "output.f32le", dtype="<f4"
            ).reshape(y.shape)
            error = float(np.max(abs(y.astype(float) - old)))
            if error > REOPEN_TOLERANCE:
                raise ValueError("JSON candidate reopening changed native audio")
            errors.append(error)
        report["reopen_max_errors"] = errors
        bounded_file(renderer, 128 * 1024 * 1024, data["context"]["renderer_sha256"])
        (output / "preset.json").write_text(
            json.dumps(chain, indent=2, allow_nan=False) + "\n"
        )
        (output / "audition_setup.json").write_text(
            json.dumps(
                {
                    "context": data["context"],
                    "programmes": data["programmes"],
                    "note": "Explicit review required. Per-programme audition trims are separate; this preset does not apply them or calibrate loudspeakers.",
                },
                indent=2,
                allow_nan=False,
            )
            + "\n"
        )
    bounded_file(renderer, 128 * 1024 * 1024, data["context"]["renderer_sha256"])
    report["session_sha256"] = digest(
        json.dumps(data, sort_keys=True, allow_nan=False).encode()
    )
    (output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    if report["model"] is not None:
        (output / "model.json").write_text(
            json.dumps(
                {
                    "model": report["model"],
                    "context": data["context"],
                    "candidates": data["candidates"],
                    "accepted": report["accepted"],
                    "session_sha256": report["session_sha256"],
                    "data_origin": data["data_origin"],
                },
                indent=2,
                allow_nan=False,
            )
            + "\n"
        )
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--session", type=Path, required=True)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    with args.session.open("rb") as source:
        raw = source.read(1024 * 1024 + 1)
    if len(raw) > 1024 * 1024:
        raise ValueError("Session exceeds1MiB")
    report = run(json.loads(raw), args.renderer, args.output)
    print(
        json.dumps(
            {
                "accepted": report["accepted"],
                "selected_id": report["selected_id"],
                "reasons": report["reasons"],
                "automatic_apply": False,
            }
        )
    )
    if not report["accepted"]:
        raise SystemExit(2)


if __name__ == "__main__":
    main()
