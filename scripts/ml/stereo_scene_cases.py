"""Synthetic functional scene labels/policies, explicitly not listening annotations."""

from pathlib import Path

import numpy as np
from scipy.signal import lfilter
from stereo_imager_native import preset
from stereo_scenes import SESSION_FORMAT, digest, file_digest

CONTEXT = {
    "sample_rate": 48000,
    "channels": 2,
    "scene_definition_id": "synthetic-spatial-scene-demonstration",
}
SETTINGS = {
    "centered": preset(width=1.4, low_width=0.8, high_width=1.2, mix=0.7),
    "diffuse": preset(),
    "side_dominant": preset(width=0.7, low_width=0.5, mid_width=0.9, mono_bass=True),
}


def signal(scene, seed, rate=48000, seconds=1, gain=1):
    rng = np.random.default_rng(seed)
    length = int(rate * seconds)
    channels = rng.normal(size=(length, 2))
    pole = rng.uniform(0, 0.45)
    colored = lfilter([1 - pole], [1, -pole], channels, axis=0)
    mid, side = colored[:, 0], colored[:, 1]
    fraction = {"centered": 0.025, "diffuse": 0.48, "side_dominant": 0.83}[scene]
    mid *= np.sqrt(1 - fraction)
    side *= np.sqrt(fraction)
    envelope = 1 + 0.05 * np.sin(
        2 * np.pi * rng.uniform(0.5, 3) * np.arange(length) / rate
    )
    return (
        np.column_stack([mid + side, mid - side]) * envelope[:, None] * 0.07 * gain
    ).astype(np.float32)


def session(directory, renderer, seed=20881026):
    root = Path(directory)
    root.mkdir(parents=True, exist_ok=False)
    records = []
    for group in range(8):
        for index, scene in enumerate(SETTINGS):
            x = signal(scene, seed + 100 * group + index)
            name = f"source-{group}-{scene}"
            raw = x.astype("<f4").tobytes()
            path = root / (name + ".f32le")
            path.write_bytes(raw)
            records.append(
                {
                    "id": name,
                    "source_group": f"source-{group}",
                    "scene": scene,
                    "frames": len(x),
                    "audio_file": str(path.resolve()),
                    "audio_sha256": digest(raw),
                }
            )
    return {
        "format": SESSION_FORMAT,
        "context": dict(CONTEXT),
        "data_origin": "synthetic_validation",
        "renderer_sha256": file_digest(renderer),
        "settings_by_scene": SETTINGS,
        "protocol": {"whole_source_grouping": True},
        "records": records,
    }
