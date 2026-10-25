"""Bounded, explicitly calibrated paired hardware data; no inferred calibration."""

import argparse
import hashlib
import json
import math
from pathlib import Path

import numpy as np
from fit_binaural_preferences import hash_value, number
from fit_crossfeed_preferences import identifier

FORMAT = "sotf-analog-hardware-capture-v1"
MAX_FRAMES = 4_000_000
MAX_TOTAL_FRAMES = 8_000_000


def rms_to_dbu(rms, full_scale_sine_dbu):
    """Convert digital RMS using a measured full-scale-sine RMS dBu reference."""
    number(rms, 0, 16)
    number(full_scale_sine_dbu, -100, 100)
    if rms <= 0:
        return None  # Silence has no finite logarithmic level.
    return float(full_scale_sine_dbu + 20 * math.log10(rms * math.sqrt(2)))


def validate_context(ctx):
    if ctx["data_origin"] not in {"measured_hardware", "synthetic_validation"}:
        raise ValueError("Declare measured hardware or synthetic validation")
    if (
        type(ctx["sample_rate"]) is not int
        or not 44100 <= ctx["sample_rate"] <= 192000
        or type(ctx["channels"]) is not int
        or ctx["channels"] != 1
    ):
        raise ValueError(
            "This capture format describes one fixed-rate hardware signal path"
        )
    hardware = ctx["hardware"]
    for k in ["manufacturer", "model", "unit_id", "configuration_id"]:
        identifier(hardware[k])
    settings = hardware["controls"]
    if not isinstance(settings, dict) or not 1 <= len(settings) <= 32:
        raise ValueError("Declare the actual hardware operating controls")
    for k, v in settings.items():
        identifier(k)
        if isinstance(v, str):
            identifier(v)
        elif not isinstance(v, bool):
            number(v, -1e6, 1e6)
        else:
            raise TypeError("Use explicit hardware control values")
    calibration = ctx["calibration"]
    for k in ["calibration_id", "measurement_reference"]:
        identifier(calibration[k])
    if (
        calibration["reference"] != "full_scale_sine_rms_dbu"
        or calibration["shared_clock"] is not True
    ):
        raise ValueError(
            "Declare measured sine RMS I/O calibration and a shared capture clock"
        )
    for k in ["input_full_scale_dbu", "output_full_scale_dbu"]:
        number(calibration[k], -100, 100)
    number(calibration["source_impedance_ohms"], 0, 1e9)
    number(calibration["load_impedance_ohms"], 1, 1e12)
    alignment = ctx["alignment"]
    if alignment["paired_sample_aligned"] is not True:
        raise ValueError(
            "Provide already aligned pairs; do not guess or hide hardware delay"
        )
    number(alignment["maximum_residual_delay_samples"], 0, 0.1)
    number(alignment["maximum_residual_drift_ppm"], 0, 1)
    identifier(alignment["measurement_reference"])
    rights = ctx["rights"]
    for k in ["license", "attribution", "reference"]:
        identifier(rights[k])
    if rights["training_authorized"] is not True:
        raise ValueError("Declare rights to train on the paired captures")


def validate(data):
    if not isinstance(data, dict) or data.get("format") != FORMAT:
        raise ValueError("Unsupported paired hardware corpus")
    ctx = data["context"]
    validate_context(ctx)
    records = data["captures"]
    if not isinstance(records, list) or not 1 <= len(records) <= 64:
        raise ValueError("Supply1..64 paired capture records")
    ids, groups, pairs, dry_groups, total = set(), {}, set(), {}, 0
    for r in records:
        identity, group = identifier(r["id"]), identifier(r["source_group"])
        split = r["split"]
        if identity in ids or split not in {"train", "validation", "test"}:
            raise ValueError(
                "Unique capture IDs and explicit train/validation/test splits required"
            )
        ids.add(identity)
        if group in groups and groups[group] != split:
            raise ValueError("Crops/levels of one source cannot cross dataset splits")
        groups[group] = split
        if (
            type(r["frames"]) is not int
            or not math.ceil(ctx["sample_rate"] / 4) <= r["frames"] <= MAX_FRAMES
        ):
            raise ValueError("Supply250ms..4M frames per aligned pair")
        total += r["frames"]
        pair = tuple(hash_value(r[k]) for k in ["dry_sha256", "wet_sha256"])
        if pair[0] in dry_groups and dry_groups[pair[0]] != group:
            raise ValueError("Identical declared dry PCM cannot cross source groups")
        dry_groups[pair[0]] = group
        if pair in pairs:
            raise ValueError("Duplicate paired PCM bytes")
        pairs.add(pair)
        for k in ["dry_file", "wet_file"]:
            if not isinstance(r[k], str) or not r[k]:
                raise ValueError("Use explicit paired PCM paths")
    if total > MAX_TOTAL_FRAMES:
        raise ValueError("Corpus exceeds8M aggregate paired frames")
    return groups


def load_manifest(path):
    with Path(path).open("rb") as source:
        raw = source.read(1024 * 1024 + 1)
    if len(raw) > 1024 * 1024:
        raise ValueError("Capture manifest exceeds1MiB")
    data = json.loads(raw)
    validate(data)
    return data, hashlib.sha256(raw).hexdigest()


def read_corpus(data):
    """Read exact declared raw pairs without alignment, normalization or resampling."""
    validate(data)
    loaded, summary, dry_groups = {}, [], {}
    calibration = data["context"]["calibration"]
    for r in data["captures"]:
        signals, metrics = [], {}
        for kind in ["dry", "wet"]:
            size = r["frames"] * 4
            with Path(r[kind + "_file"]).open("rb") as source:
                raw = source.read(size + 1)
            h = hashlib.sha256(raw).hexdigest()
            if len(raw) != size or h != r[kind + "_sha256"]:
                raise ValueError("Paired PCM length or SHA256 differs from manifest")
            x = np.frombuffer(raw, dtype="<f4")
            if not np.isfinite(x).all() or np.max(abs(x)) > 0.999:
                raise ValueError(
                    "Paired captures must have finite ADC/DAC sample headroom"
                )
            rms = float(np.sqrt(np.mean(x.astype(float) ** 2)))
            if rms < 1e-5:
                raise ValueError("Capture lacks adequate signal excitation")
            metrics[kind] = {
                "rms_dbfs": 20 * math.log10(rms),
                "peak": float(np.max(abs(x))),
                "rms_dbu": rms_to_dbu(
                    rms,
                    calibration[
                        ("input" if kind == "dry" else "output") + "_full_scale_dbu"
                    ],
                ),
            }
            signals.append(x)
            if kind == "dry":
                if h in dry_groups and dry_groups[h] != r["source_group"]:
                    raise ValueError(
                        "Identical dry PCM cannot masquerade as independent sources"
                    )
                dry_groups[h] = r["source_group"]
        loaded[r["id"]] = tuple(signals)
        summary.append(
            {
                "id": r["id"],
                "source_group": r["source_group"],
                "split": r["split"],
                "frames": r["frames"],
                "dry_sha256": r["dry_sha256"],
                "wet_sha256": r["wet_sha256"],
                "levels": metrics,
            }
        )
    return loaded, {
        "context": data["context"],
        "captures": summary,
        "hardware_origin_declared": data["context"]["data_origin"]
        == "measured_hardware",
        "scope": "Calibration, alignment, hardware identity and rights are declarations that require capture evidence. No alignment, gain fitting, resampling or hardware-model admission was performed.",
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        raise ValueError("Refuse to overwrite capture evidence")
    data, identity = load_manifest(args.manifest)
    _, summary = read_corpus(data)
    summary.update(manifest_sha256=identity, hardware_model_admitted=False)
    with args.output.open("x") as output:
        output.write(json.dumps(summary, indent=2, allow_nan=False) + "\n")
    print(
        json.dumps(
            {"captures": len(summary["captures"]), "hardware_model_admitted": False}
        )
    )


if __name__ == "__main__":
    main()
