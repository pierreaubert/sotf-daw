"""Expander profile for the shared fixed-WOLA native render protocol."""

from restoration_mask_native import reference, render_fixed_wola

__all__ = ["parameters", "reference", "render"]


def parameters(ratio=4.0, threshold=-40.0):
    return {
        "num_bands": 3,
        "crossover_preset": 0,
        "crossover_frequencies": [250.0, 2500.0],
        "threshold_db": threshold,
        "ratio": ratio,
        "attack_ms": 5.0,
        "release_ms": 80.0,
        "knee_db": 6.0,
        "range_db": 20.0,
        "hysteresis_db": 3.0,
        "hold_ms": 10.0,
        "link_channels": True,
        "mix": 1.0,
        "detection_mode": "Peak",
        "lookahead_ms": 0.0,
        "bands": [],
        "processing_mode": "spectral",
        "auto_makeup": False,
        "measured_auto_makeup": False,
        "sidechain_hpf_hz": 0.0,
    }


def render(
    signal,
    renderer,
    work,
    *,
    model=None,
    sha=None,
    enabled=True,
    rate=48000,
    callback=193,
    drain=193,
    params=None,
):
    params = parameters() if params is None else params
    if (
        set(params) != set(parameters())
        or params["mix"] != 1
        or params["processing_mode"] != "spectral"
    ):
        raise ValueError("Use complete full-wet spectral parameters")
    return render_fixed_wola(
        signal,
        renderer,
        work,
        params=params,
        model=model,
        sha=sha,
        enabled=enabled,
        rate=rate,
        callback=callback,
        drain=drain,
    )
