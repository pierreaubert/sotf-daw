"""Explicitly synthetic functional sessions, not physical calibration evidence."""

import numpy as np
from loudness_preferences import FORMAT


def target(level):
    delta = 83 - np.asarray(level)
    return np.stack(
        [0.08 * delta + 0.5 * np.sin(delta / 12), -0.018 * delta, 0.035 * delta],
        axis=-1,
    )


def session(seed=20431026, kind="consistent"):
    rng = np.random.default_rng(seed)
    data = {
        "format": FORMAT,
        "data_origin": "synthetic_validation",
        "context": {
            "listener_id": "synthetic-listener",
            "playback_chain_id": "synthetic-chain",
            "position_id": "synthetic-position",
            "session_id": f"synthetic-{seed}-{kind}",
            "presentation": "stereo_speakers",
            "sample_rate": 48000,
        },
        "calibration": {
            "level_basis": "identity_reference_signal_at_listening_position",
            "weighting": "Z",
            "confirmed": True,
            "measurement_id": "synthetic-only",
            "method": "synthetic_fixture",
            "reference_level_db_spl": 83,
        },
        "protocol": {
            "blinded": True,
            "randomized_order": True,
            "reference_identity_anchor": True,
            "headroom_policy": "native_peak_normalized",
            "external_level_matching_method": "synthetic_control",
            "maximum_level_mismatch_db": 0.1,
        },
        "filter_shape": {
            "low_freq": 100,
            "mid_freq": 3500,
            "mid_q": 0.707,
            "high_freq": 8000,
        },
        "observations": [],
    }
    for programme in range(8):
        offset = rng.normal(0, 0.12, 3)
        if kind == "contradictory":
            offset = np.full(3, 2.5 if programme < 4 else -2.5)
        for level in [50, 60, 70, 83]:
            scale = (83 - level) / 33
            gains = target(level) + offset * scale + rng.normal(0, 0.06, 3) * scale
            data["observations"].append(
                {
                    "id": f"observation-{programme}-{level}",
                    "programme_id": f"programme-{programme}",
                    "level_db_spl": level,
                    "preferred_gains_db": gains.tolist(),
                    "uncertainty_db": 1.8 if kind == "uncertain" else 0.1,
                }
            )
    return data
