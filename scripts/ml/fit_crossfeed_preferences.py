#!/usr/bin/env python3
"""Offline crossfeed preferences learned only over explicitly auditioned settings.

Regularized Bradley-Terry utilities include a presentation-side bias term.
Programme groups receive equal weight. Group bootstrap stability and held-out
programme prediction control abstention; these are not calibrated population
confidence guarantees. The audio plugin and active settings are never changed.
"""

import argparse
import hashlib
import json
import math
from pathlib import Path

import numpy as np
from preference_ranking import POLICY, connected, fit_preferences, learn, objective

__all__ = ["POLICY", "connected", "fit", "learn", "objective", "validate"]

FORMAT = "sotf-crossfeed-preferences-v1"

RANGES = {
    "mix": (0, 1),
    "bauer_fcut_hz": (400, 1000),
    "bauer_feed_db": (0, 15),
    "meier_level": (0, 100),
    "mb_low_freq_hz": (50, 500),
    "mb_mid_high_freq_hz": (2000, 15000),
    "mb_low_feed_db": (-60, 15),
    "mb_mid_feed_db": (-60, 15),
    "mb_high_feed_db": (-60, 15),
    "itd_delay_ms": (0, 1),
    "head_yaw_deg": (0, 0),
}


def identifier(value):
    if not isinstance(value, str) or not 1 <= len(value) <= 128:
        raise ValueError(
            "Identifiers must be nonempty strings of at most 128 characters"
        )
    return value


def parameters(value):
    required = set(RANGES) | {"mode", "enabled", "autogain_enabled"}
    if not isinstance(value, dict) or set(value) != required:
        raise ValueError("Supply the exact auditioned crossfeed parameter schema")
    if value["mode"] not in ("Off", "Bauer", "Meier", "Mb", "Hrtf"):
        raise ValueError("Unsupported crossfeed mode")
    if value["enabled"] is not True or value["autogain_enabled"] is not False:
        raise ValueError("Auditions require enabled processing and AutoGain off")
    for key, (lower, upper) in RANGES.items():
        number = value[key]
        if (
            isinstance(number, bool)
            or not isinstance(number, (int, float))
            or not math.isfinite(number)
            or not lower <= number <= upper
        ):
            raise ValueError("Invalid crossfeed parameter: " + key)
    return value


def audition_key(value):
    """Ignore inactive controls and compare the native float32 settings."""
    mode = value["mode"]
    if (
        mode == "Off"
        or value["mix"] == 0
        or (mode == "Bauer" and value["bauer_feed_db"] == 0)
        or (mode == "Meier" and value["meier_level"] == 0)
    ):
        return ("bypass",)
    active = {
        "Bauer": ["bauer_fcut_hz", "bauer_feed_db"],
        "Meier": ["meier_level"],
        "Mb": [
            "mb_low_freq_hz",
            "mb_mid_high_freq_hz",
            "mb_low_feed_db",
            "mb_mid_feed_db",
            "mb_high_feed_db",
        ],
        "Hrtf": [],
    }[mode] + ["mix", "itd_delay_ms"]
    return (mode, *(float(np.float32(value[key])) for key in active))


def validate(data):
    if not isinstance(data, dict) or data.get("format") != FORMAT:
        raise ValueError("Unsupported preference session format")
    if data.get("data_origin") not in ("listener_observations", "synthetic_validation"):
        raise ValueError("Declare listener observations or synthetic validation")
    context = data["context"]
    for key in ("listener_id", "headphones_id", "session_id"):
        identifier(context[key])
    rate = context["sample_rate"]
    if type(rate) is not int or not 44100 <= rate <= 192000:
        raise ValueError("Unsupported audition rate")
    protocol = data["protocol"]
    if (
        protocol.get("blinded") is not True
        or protocol.get("randomized_order") is not True
    ):
        raise ValueError("Record blinded, randomized auditions before fitting")
    match = protocol["maximum_level_mismatch_db"]
    if (
        isinstance(match, bool)
        or not isinstance(match, (int, float))
        or not math.isfinite(match)
        or not 0 <= match <= 0.25
    ):
        raise ValueError("Auditions require recorded level matching within 0.25 dB")
    candidates = data["candidates"]
    if not isinstance(candidates, list) or not 2 <= len(candidates) <= 12:
        raise ValueError("Supply 2 to 12 auditioned candidates")
    ids, signatures = [], set()
    for candidate in candidates:
        ids.append(identifier(candidate["id"]))
        signature = audition_key(parameters(candidate["parameters"]))
        if signature in signatures:
            raise ValueError("Duplicate auditioned settings")
        signatures.add(signature)
    if len(set(ids)) != len(ids) or data["baseline_id"] not in ids:
        raise ValueError("Candidate identifiers must be unique and include baseline")
    trials = data["trials"]
    if not isinstance(trials, list) or not 1 <= len(trials) <= 4096:
        raise ValueError("Supply 1 to 4096 trials")
    trial_ids, groups, design, labels = set(), [], [], []
    orientations = {}
    for trial in trials:
        trial_id = identifier(trial["id"])
        if trial_id in trial_ids:
            raise ValueError("Duplicate trial identifier")
        trial_ids.add(trial_id)
        groups.append(identifier(trial["programme_id"]))
        if trial["a"] not in ids or trial["b"] not in ids or trial["a"] == trial["b"]:
            raise ValueError("Trial must compare two declared candidates")
        a, b = ids.index(trial["a"]), ids.index(trial["b"])
        row = np.zeros(len(ids) + 1)
        row[a], row[b], row[-1] = 1, -1, 1
        design.append(row)
        if trial["choice"] not in ("a", "b", "tie"):
            raise ValueError("Choice must be a, b or tie")
        labels.append({"a": 1.0, "b": 0.0, "tie": 0.5}[trial["choice"]])
        orientations.setdefault(tuple(sorted((a, b))), set()).add(a < b)
    if len(set(groups)) > 128:
        raise ValueError("At most 128 programme groups are supported")
    return ids, np.asarray(design), np.asarray(labels), np.asarray(groups), orientations


def fit(data):
    return fit_preferences(data, validate, "sotf-bt-crossfeed-v1")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--session", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    with args.session.open("rb") as source:
        raw = source.read(2 * 1024 * 1024 + 1)
    if len(raw) > 2 * 1024 * 1024:
        raise ValueError("Preference session exceeds 2 MiB")
    data = json.loads(raw)
    result = fit(data)
    result.update(
        session_sha256=hashlib.sha256(raw).hexdigest(), context=data["context"]
    )
    args.output.mkdir(parents=True, exist_ok=False)
    (args.output / "report.json").write_text(
        json.dumps(result, indent=2, allow_nan=False) + "\n"
    )
    if result["accepted"]:
        (args.output / "preset.json").write_text(
            json.dumps(result["parameters"], indent=2, allow_nan=False) + "\n"
        )
    print(
        json.dumps(
            {
                "accepted": result["accepted"],
                "selected_id": result["selected_id"],
                "reasons": result["reasons"],
            }
        )
    )
    if not result["accepted"]:
        raise SystemExit(2)


if __name__ == "__main__":
    main()
