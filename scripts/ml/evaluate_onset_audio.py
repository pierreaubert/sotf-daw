"""Frozen-model waveform diagnostics against independent native shaping.

These quantify changed audio, not subjective improvement or an ideal target.
Fresh seeds are independent of fitting/validation/final classifier sources.
"""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from onset_assist_cases import fixture
from onset_assist_native import render


def rms(x):
    return float(np.sqrt(np.mean(np.square(x)))) if len(x) else None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", required=True, type=Path)
    parser.add_argument("--model", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    sha = hashlib.sha256(args.model.read_bytes()).hexdigest()
    rows = []
    for kind in [
        "matched",
        "slow_attack",
        "noise_bursts",
        "steady_only",
        "phase_late",
        "silence",
    ]:
        for seed in range(20294120, 20294124):
            source, identity = fixture(seed, kind=kind)
            for attack, sustain in [(60.0, 20.0), (-60.0, -20.0), (0.0, 20.0)]:
                root = args.output / f"{kind}-{seed}-{attack}"
                native, _ = render(
                    source,
                    args.renderer,
                    root / "native",
                    attack=attack,
                    sustain=sustain,
                )
                assisted, metadata = render(
                    source,
                    args.renderer,
                    root / "assisted",
                    model=args.model,
                    sha=sha,
                    attack=attack,
                    sustain=sustain,
                )
                if not metadata["model_loaded"]:
                    raise RuntimeError("Frozen model not loaded")
                a, b = assisted[:, 0].astype(float), native[:, 0].astype(float)
                delta = a - b
                event_mask = np.zeros(len(a), dtype=bool)
                nuisance_mask = event_mask.copy()
                for events, mask in [
                    (identity["desired_event_frames"], event_mask),
                    (identity["nuisance_event_frames"], nuisance_mask),
                ]:
                    for start in events:
                        mask[start : start + 480] = True  # First causal 10 ms.
                if attack == 0 and not np.array_equal(assisted, native):
                    raise RuntimeError("Zero attack changed native sustain output")
                if kind == "silence" and np.any(assisted):
                    raise RuntimeError("Assist generated audio from silence")
                if np.max(abs(a)) > 1.0:
                    raise RuntimeError("Native peak protection exceeded")
                rows.append(
                    {
                        "source": identity,
                        "attack": attack,
                        "sustain": sustain,
                        "relative_rms_change": rms(delta) / max(rms(b), 1e-20),
                        "event_first_10ms_delta_rms": rms(delta[event_mask]),
                        "nuisance_first_10ms_delta_rms": rms(delta[nuisance_mask]),
                        "outside_event_first_10ms_delta_rms": rms(delta[~event_mask]),
                        "maximum_abs_delta": float(np.max(abs(delta))),
                        "maximum_delta_first_difference": float(
                            np.max(abs(np.diff(delta)))
                        ),
                        "peak": float(np.max(abs(a))),
                        "native_sha256": hashlib.sha256(native.tobytes()).hexdigest(),
                        "assisted_sha256": hashlib.sha256(
                            assisted.tobytes()
                        ).hexdigest(),
                        "counters": metadata["final_state"]["counters"],
                    }
                )
    report = {
        "model_sha256": sha,
        "renderer_sha256": hashlib.sha256(args.renderer.read_bytes()).hexdigest(),
        "scope": "mono 48 kHz generated sources; fixed native sensitivity/mix/output; descriptive waveform changes only",
        "deployment_admitted": False,
        "rows": rows,
    }
    (args.output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    print(json.dumps({"cases": len(rows), "deployment_admitted": False}))


if __name__ == "__main__":
    main()
