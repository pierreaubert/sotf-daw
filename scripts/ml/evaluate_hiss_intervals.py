#!/usr/bin/env python3
"""Fresh functional classifier cases and an explicit semantic counterfactual."""

import argparse
import json
import subprocess
import sys
from pathlib import Path

import numpy as np
from hiss_interval_cases import CONTEXT, session, signal
from hiss_intervals import classical, digest, features, load_model, propose
from hiss_native import PARAMS, render


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--renderer", type=Path, required=True)
    p.add_argument("--work", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True)
    args = p.parse_args()
    args.work.mkdir(parents=True, exist_ok=False)
    session(args.work / "training", seed=20771026)
    fitted = subprocess.run(
        [
            sys.executable,
            "scripts/ml/fit_hiss_intervals.py",
            "--session",
            str(args.work / "training/session.json"),
            "--output",
            str(args.work / "fit"),
        ],
        check=False,
        capture_output=True,
        text=True,
    )
    (args.work / "fit-command.txt").write_text(fitted.stdout + fitted.stderr)
    if fitted.returncode:
        raise ValueError(
            "Functional training session did not admit: "
            + fitted.stdout
            + fitted.stderr
        )
    fit_report = json.loads((args.work / "fit/report.json").read_text())
    model = load_model(args.work / "fit/model.json", fit_report["model_sha256"])
    rows = []

    def evaluate(name, x, label, group):
        folder = args.work / "cases" / name
        folder.mkdir(parents=True, exist_ok=False)
        raw = x.astype("<f4").tobytes()
        (folder / "input.f32le").write_bytes(raw)
        result = propose(x, CONTEXT, model)
        row = {
            "case": name,
            "source_group": group,
            "label": label,
            "input_file": str(folder / "input.f32le"),
            "input_sha256": digest(raw),
            "features": features(x, CONTEXT).tolist(),
            "classical_review_candidate": classical(features(x, CONTEXT)),
            "proposal": result["intervals"][0],
        }
        (folder / "proposal.json").write_text(json.dumps(result, indent=2) + "\n")
        rows.append(row)
        return row

    for label in ["noise_only", "programme"]:
        for i in range(16):
            evaluate(
                f"fresh-{label}-{i}",
                signal(label, 20781026 + i * 10 + (label == "programme")),
                label,
                f"fresh-{label}-{i}",
            )
    counter = signal("noise_only", 20811026)
    a = evaluate(
        "counterfactual-unwanted-hiss", counter, "noise_only", "same-underlying-pcm"
    )
    b = evaluate(
        "counterfactual-desired-noise-effect",
        counter.copy(),
        "programme",
        "same-underlying-pcm",
    )
    assert a["proposal"] == b["proposal"] and a["input_sha256"] == b["input_sha256"]
    fixture = Path(
        "crates/sotf-plugins/crates/sotf-plugin-hiss-reducer/tests/fixtures/recorded"
    )
    manifest = json.loads((fixture / "manifest.json").read_text())
    raw = (fixture / manifest["file"]).read_bytes()
    assert digest(raw) == manifest["pcm_sha256"]
    speech = (np.frombuffer(raw, dtype="<i2").astype(np.float32) / 32768)[:, None]
    evaluate(
        "recorded-s-development",
        speech,
        "programme",
        "existing-single-speaker-development",
    )
    # Explicit diagnostic selection, never called by the proposal API/CLI.
    y, meta = render(
        np.tile(counter, (3, 1)),
        args.renderer,
        args.work / "explicit-counterfactual-native",
        dict(PARAMS, spectral_mode=True, use_captured_profile=True, strength=1.0),
        capture=counter,
    )
    start, end = 57600, 134400
    x = np.tile(counter, (3, 1))
    attenuation = float(
        10
        * np.log10(
            np.mean(x[start:end].astype(float) ** 2)
            / np.mean(y[start:end].astype(float) ** 2)
        )
    )
    context_path = args.work / "context.json"
    context_path.write_text(json.dumps(CONTEXT) + "\n")
    cli = [
        sys.executable,
        "scripts/ml/propose_hiss_intervals.py",
        "--audio",
        a["input_file"],
        "--frames",
        "48000",
        "--context",
        str(context_path),
        "--model",
        str(args.work / "fit/model.json"),
        "--model-sha256",
        fit_report["model_sha256"],
        "--output",
        str(args.work / "proposal-cli"),
    ]
    completed = subprocess.run(cli, check=False, capture_output=True, text=True)
    (args.work / "proposal-cli-command.txt").write_text(
        completed.stdout + completed.stderr
    )
    if completed.returncode:
        raise ValueError(completed.stderr)
    cli_result = json.loads((args.work / "proposal-cli/proposals.json").read_text())
    assert (
        cli_result["automatic_capture"] is False
        and cli_result["profile_changed"] is False
    )
    fresh = [r for r in rows if r["case"].startswith("fresh-")]
    result = {
        "scope": "Synthetic functional source-labelled model; existing recorded s is development only. Identical noise PCM with opposing intended-use labels proves semantic ambiguity; no automatic-capture or population-safety claim.",
        "training_session": str(args.work / "training/session.json"),
        "fit_report": str(args.work / "fit/report.json"),
        "model": str(args.work / "fit/model.json"),
        "model_sha256": fit_report["model_sha256"],
        "cases": rows,
        "fresh_counts": {
            method: {
                label: sum(
                    r["label"] == label
                    and (
                        r["proposal"]["review_candidate"]
                        if method == "learned"
                        else r["classical_review_candidate"]
                    )
                    for r in fresh
                )
                for label in ["noise_only", "programme"]
            }
            for method in ["learned", "classical"]
        },
        "semantic_counterfactual_identical_decisions": True,
        "counterfactual_review_candidate": a["proposal"]["review_candidate"],
        "explicit_native_diagnostic": {
            "metadata": str(args.work / "explicit-counterfactual-native/metadata.json"),
            "capture_sha256": meta["capture_sha256"],
            "full_band_attenuation_db": attenuation,
            "region_frames": [start, end],
            "qualification": "Manual diagnostic capture of identical desired noise effect; not an automatic action by the classifier.",
        },
        "successful_proposal_cli": str(args.work / "proposal-cli/proposals.json"),
    }
    args.output.write_text(json.dumps(result, indent=2, allow_nan=False) + "\n")
    print(json.dumps({k: v for k, v in result.items() if k != "cases"}, indent=2))


if __name__ == "__main__":
    main()
