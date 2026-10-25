"""Native/Python qualification parity, fallback histories and conditional QA timing."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from analog_color_native import controls
from analog_color_native import render as classical_render
from analyze_analog_hardware_model import reference
from hardware_candidate_native import render


def oracle(x, model, params, classical, work, *, rate, loaded, enabled, ranges, resets):
    expected = np.empty(len(x), dtype=float)
    counts = {
        k: 0
        for k in ["neural", "disabled", "unavailable", "out_of_domain", "numerical"]
    }
    for epoch, (start, end) in enumerate(
        zip([0, *resets], [*resets, len(x)], strict=True)
    ):
        source = x[start:end]
        clean = np.where(np.isfinite(source), np.clip(source, -16, 16), 0).astype(
            np.float32
        )
        fallback, _ = classical_render(
            clean[:, None], classical, work / f"classical-{epoch}", params, rate=rate
        )
        eligible = np.isfinite(source) & (abs(source) <= model["input_peak_limit"])
        invalid = (
            np.convolve(
                (~eligible).astype(int),
                np.ones(model["architecture"]["taps"], dtype=int),
            )[: len(source)]
            > 0
        )
        active = np.full(len(source), enabled, dtype=bool)
        for lo, hi in ranges:
            active[max(0, lo - start) : max(0, min(len(source), hi - start))] = False
        neural, _ = reference(model, np.where(eligible, source, 0))
        use = active & ~invalid & loaded
        expected[start:end] = np.where(use, neural, fallback[:, 0])
        counts["disabled"] += int((~active).sum())
        if loaded:
            counts["neural"] += int(use.sum())
            counts["out_of_domain"] += int((active & invalid).sum())
        else:
            counts["unavailable"] += int(active.sum())
    return expected, counts


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--classical", type=Path, required=True)
    parser.add_argument("--learned-work", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    if args.report.exists():
        raise ValueError("Refuse to overwrite evaluation")
    args.work.mkdir(parents=True, exist_ok=False)
    binaries = {
        "candidate": hashlib.sha256(args.renderer.read_bytes()).hexdigest(),
        "classical": hashlib.sha256(args.classical.read_bytes()).hexdigest(),
    }
    rows = []
    for kind in ["cross-memory", "hammerstein"]:
        candidate = args.learned_work / kind / "candidate.json"
        model = json.loads(candidate.read_text())
        sha = hashlib.sha256(candidate.read_bytes()).hexdigest()
        data = json.loads(
            (args.learned_work / (kind + "-captures") / "manifest.json").read_text()
        )
        for record in data["captures"]:
            if record["split"] == "train":
                continue
            raw = np.fromfile(record["dry_file"], dtype="<f4")
            for gain in [0.5, 1.0, 2.0]:
                x = (raw * gain).astype(np.float32)
                for fallback_id in [0, 5]:
                    key = f"{kind}-{record['id']}-{gain}-{fallback_id}"
                    params = controls(
                        model_id=fallback_id,
                        drive_db=6,
                        color=0.73,
                        character=0.31,
                        output_trim_db=-3,
                    )
                    ranges = [[0, 101], [1300, 1513]]
                    resets = [777, 3000]
                    y, m = render(
                        x,
                        args.renderer,
                        args.work / key,
                        candidate,
                        sha,
                        model["context"],
                        fallback=params,
                        zeros=512,
                        disabled_ranges=ranges,
                        reset_frames=resets,
                    )
                    expected, counts = oracle(
                        np.pad(x, (0, 512)),
                        model,
                        params,
                        args.classical,
                        args.work / (key + "-oracle"),
                        rate=48000,
                        loaded=True,
                        enabled=True,
                        ranges=ranges,
                        resets=resets,
                    )
                    error = float(np.max(abs(y - expected)))
                    changed, cm = render(
                        x,
                        args.renderer,
                        args.work / (key + "-reopen"),
                        candidate,
                        sha,
                        model["context"],
                        fallback=params,
                        zeros=512,
                        disabled_ranges=ranges,
                        reset_frames=resets,
                        callback=4096,
                    )
                    row = {
                        "case": key,
                        "input_sha256": m["input_sha256"],
                        "candidate_sha256": sha,
                        "work": str((args.work / key).resolve()),
                        "max_error": error,
                        "counts": m["counts"],
                        "callback_exact": bool(np.array_equal(y, changed)),
                        "counts_match": m["counts"] == counts == cm["counts"],
                        "sample_peak": m["sample_peak"],
                    }
                    row["pass"] = bool(
                        error < 6e-8
                        and row["callback_exact"]
                        and row["counts_match"]
                        and m["candidate_loaded"]
                    )
                    rows.append(row)
    candidate = args.learned_work / "cross-memory/candidate.json"
    model = json.loads(candidate.read_text())
    sha = hashlib.sha256(candidate.read_bytes()).hexdigest()
    rng = np.random.default_rng(20261108)
    x = rng.normal(0, 0.06, 4099).astype(np.float32)
    x[[17, 103, 200]] = [np.nan, np.inf, 2]
    for kind in [
        "nonfinite",
        "missing",
        "bad-hash",
        "bad-context",
        "disabled",
        "rate-mismatch",
    ]:
        path = None if kind == "missing" else candidate
        expected_sha = "0" * 64 if kind == "bad-hash" else sha
        ctx = json.loads(json.dumps(model["context"]))
        if kind == "bad-context":
            ctx["hardware"]["unit_id"] = "different-unit"
        rate = 96000 if kind == "rate-mismatch" else 48000
        enabled = kind != "disabled"
        loaded = kind in {"nonfinite", "disabled"}
        params = controls(
            model_id=4, drive_db=6, color=0.73, character=0.31, output_trim_db=-3
        )
        y, m = render(
            x,
            args.renderer,
            args.work / kind,
            path,
            expected_sha,
            ctx,
            fallback=params,
            enabled=enabled,
            rate=rate,
        )
        expected, counts = oracle(
            x,
            model,
            params,
            args.classical,
            args.work / (kind + "-oracle"),
            rate=rate,
            loaded=loaded,
            enabled=enabled,
            ranges=[],
            resets=[],
        )
        error = float(np.max(abs(y - expected)))
        rows.append(
            {
                "case": kind,
                "input_sha256": m["input_sha256"],
                "work": str((args.work / kind).resolve()),
                "max_error": error,
                "counts": m["counts"],
                "pass": bool(
                    error < 6e-8
                    and m["counts"] == counts
                    and m["candidate_loaded"] == loaded
                ),
            }
        )
    timings = []
    probe = np.clip(rng.normal(0, 0.06, 65537), -0.2, 0.2).astype(np.float32)
    for mode in ["missing", "disabled", "enabled"]:
        values = []
        for repeat in range(5):
            _, meta = render(
                probe,
                args.renderer,
                args.work / f"timing-{mode}-{repeat}",
                None if mode == "missing" else candidate,
                sha,
                model["context"],
                enabled=mode != "disabled",
            )
            values.append(meta["qa_process_seconds"])
        timings.append(
            {
                "mode": mode,
                "seconds": values,
                "median_seconds": float(np.median(values)),
            }
        )
    assert (
        binaries["candidate"] == hashlib.sha256(args.renderer.read_bytes()).hexdigest()
    )
    assert (
        binaries["classical"] == hashlib.sha256(args.classical.read_bytes()).hexdigest()
    )
    result = {
        "binaries_sha256": binaries,
        "cases": rows,
        "all_pass": all(r["pass"] for r in rows),
        "qa_timings": timings,
        "timing_scope": "Five fresh65537-frame mono runs per mode, same QA wrapper and input. Includes continuous classical fallback, candidate history, guards and copies; not a production deadline, portable speed or standalone neural cost guarantee.",
        "scope": "Native functional parity/fallback using unadmitted generated candidates. Gain/callback variants share sources. No measured-hardware quality or production activation.",
    }
    args.report.write_text(json.dumps(result, indent=2, allow_nan=False) + "\n")
    if not result["all_pass"]:
        raise ValueError("Retained native qualification failed")


if __name__ == "__main__":
    main()
