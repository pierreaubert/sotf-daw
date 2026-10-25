"""Native validation for explicitly reviewable offline Matrix routing proposals."""

import json
import subprocess
from pathlib import Path

import numpy as np
from matrix_routing import MAX_ERROR, MAX_GROUP_RMS, MAX_RMS, validate


def render(params, physical, rate, renderer, work, callback=193):
    physical = np.asarray(physical, np.float32)
    if (
        physical.ndim != 2
        or not 1 <= len(physical) <= 2048
        or physical.shape[1] != max(params["input_channel_map"]) + 1
        or not np.isfinite(physical).all()
        or np.max(np.abs(physical)) > 1
    ):
        raise ValueError("Unsupported native routing input")
    work = Path(work)
    work.mkdir(parents=True, exist_ok=False)
    request = {
        "sample_rate": rate,
        "callback_frames": callback,
        "params": params,
        "input": physical.ravel().tolist(),
    }
    path = work / "request.json"
    path.write_text(json.dumps(request, allow_nan=False) + "\n")
    process = subprocess.run(
        [str(Path(renderer).resolve()), str(path)],
        check=False,
        capture_output=True,
        text=True,
        timeout=120,
    )
    (work / "stderr.txt").write_text(process.stderr)
    if process.returncode:
        raise ValueError("Native routing validation failed: " + process.stderr)
    (work / "response.json").write_text(process.stdout)
    response = json.loads(process.stdout)
    outputs = max(params["output_channel_map"]) + 1
    actual = np.asarray(response["output"], np.float64)
    if (
        response["status"] != "ok"
        or response["frames"] != len(physical)
        or response["sample_rate"] != rate
        or response["physical_input_channels"] != physical.shape[1]
        or response["physical_output_channels"] != outputs
        or response["latency_samples"] != 0
        or response["drained_frames"] != 0
        or actual.shape != (len(physical) * outputs,)
        or not np.isfinite(actual).all()
        or np.max(np.abs(actual)) > 1 + 2e-6
    ):
        raise ValueError("Invalid native routing response")
    actual = actual.reshape(len(physical), outputs)
    expected = np.zeros_like(actual)
    matrix = np.asarray(params["matrix"], np.float64).reshape(
        len(params["output_channel_map"]), len(params["input_channel_map"])
    )
    expected[:, params["output_channel_map"]] = (
        physical[:, params["input_channel_map"]].astype(np.float64) @ matrix.T
    )
    error = float(np.max(np.abs(actual - expected)))
    if error > 2e-6:
        raise ValueError("Native routing differs from independent linear product")
    response.pop("output")
    response["max_reference_error"] = error
    return actual, response


def qualify(data, fit, renderer, work):
    if not fit["accepted"]:
        raise ValueError("Native qualification requires an admitted statistical fit")
    work = Path(work)
    params = fit["parameters"]
    width = max(params["input_channel_map"]) + 1
    matrix = np.asarray(params["matrix"]).reshape(len(params["output_channel_map"]), -1)
    # Isolated physical channels also test that unmapped inputs do not leak.
    probes = np.concatenate(
        [
            np.eye(width),
            -np.eye(width),
            np.zeros((1, width)),
            np.random.default_rng(20491026).uniform(-1, 1, (193, width)),
        ]
    )
    coherent = np.zeros((len(matrix), width))
    coherent[:, params["input_channel_map"]] = np.sign(matrix)
    probes = np.concatenate([probes, coherent])
    first, probe_meta = render(
        params, probes, data["sample_rate"], renderer, work / "probes", 193
    )
    second, _ = render(
        params, probes, data["sample_rate"], renderer, work / "probes-reopen", 17
    )
    if not np.array_equal(first, second):
        raise ValueError("Native routing depends on callback size")
    sources = {g[0]: g for g in validate(data)}
    heldouts = []
    for index, fold in enumerate(fit["nested_holdouts"]):
        _, x, y = sources[fold["source_id"]]
        physical = np.zeros((len(x), width), np.float32)
        physical[:, params["input_channel_map"]] = x
        fold_params = params | {"matrix": np.asarray(fold["matrix"]).ravel().tolist()}
        actual, meta = render(
            fold_params,
            physical,
            data["sample_rate"],
            renderer,
            work / f"heldout-{index:02d}",
            257,
        )
        delta = actual[:, params["output_channel_map"]] - y
        output_mse = np.mean(delta**2, axis=0)
        heldouts.append(
            {
                "source_id": fold["source_id"],
                "mse": float(np.mean(delta**2)),
                "rms": float(np.sqrt(np.mean(delta**2))),
                "max_error": float(np.max(np.abs(delta))),
                "output_mse": output_mse.tolist(),
                "worst_output_rms": float(np.sqrt(np.max(output_mse))),
                "native": meta,
            }
        )
    rms = float(np.sqrt(np.mean([f["mse"] for f in heldouts])))
    output_rms = np.sqrt(np.mean([f["output_mse"] for f in heldouts], axis=0))
    if (
        rms > MAX_RMS + 1e-6
        or np.max(output_rms) > MAX_RMS + 1e-6
        or max(f["rms"] for f in heldouts) > MAX_GROUP_RMS + 1e-6
        or max(f["worst_output_rms"] for f in heldouts) > MAX_GROUP_RMS + 1e-6
        or max(f["max_error"] for f in heldouts) > MAX_ERROR + 2e-6
    ):
        raise ValueError("Native nested-heldout reference mismatch")
    return {
        "probe": probe_meta,
        "nested_rms": rms,
        "nested_output_rms": output_rms.tolist(),
        "heldouts": heldouts,
        "callback_parity": True,
        "latency_samples": 0,
        "drained_frames": 0,
        "scope": "Cold native sparse routing, basis/coherent/unmapped/random probes and actual nested-heldout replay; no automatic routing application",
    }
