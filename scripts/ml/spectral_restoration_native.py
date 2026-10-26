"""Native spectral-compressor profile for explicit target-mask qualification."""

from restoration_mask_native import reference, render_fixed_wola

__all__ = ["parameters", "reference", "render"]


def parameters(ratio=4.0, threshold=-40.0):
    return {
        "fft_size_index": 0,
        "threshold_db": threshold,
        "ratio": ratio,
        "attack_ms": 5.0,
        "release_ms": 80.0,
        "knee_db": 6.0,
        "spectral_smoothing": 0.3,
        "mix": 1.0,
        "target_mode": 0,
        "delta_listen": False,
        "adaptive_threshold": False,
        "adaptive_offset_db": 0.0,
        "channel_link": 1.0,
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
        or params["fft_size_index"] != 0
        or params["mix"] != 1
        or params["delta_listen"]
    ):
        raise ValueError(
            "Use complete full-wet FFT1024 parameters without delta monitoring"
        )
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
