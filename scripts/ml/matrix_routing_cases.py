"""Synthetic identified linear routing examples and explicit failure controls."""

import numpy as np


def example(seed, inputs=2, outputs=2, sources=10, noise=0.001, maps=False):
    rng = np.random.default_rng(seed)
    matrix = rng.uniform(-1, 1, (outputs, inputs))
    matrix *= 0.8 / np.sum(np.abs(matrix), axis=1, keepdims=True)
    groups = []
    for index in range(sources):
        x = rng.uniform(-0.6, 0.6, (64, inputs))
        y = x @ matrix.T + rng.normal(0, noise, (64, outputs))
        groups.append(
            {
                "source_id": f"synthetic-{seed}-{index}",
                "input": x.tolist(),
                "desired_output": y.tolist(),
            }
        )
    imap, omap = list(range(inputs)), list(range(outputs))
    if maps:
        imap = rng.choice(128, inputs, replace=False).tolist()
        omap = rng.choice(128, outputs, replace=False).tolist()
    return {
        "format": "sotf-matrix-routing-examples-v1",
        "data_origin": "synthetic_validation",
        "sample_rate": 48000,
        "input_labels": [f"in-{i}" for i in range(inputs)],
        "output_labels": [f"out-{i}" for i in range(outputs)],
        "input_channel_map": imap,
        "output_channel_map": omap,
        "groups": groups,
    }, matrix


def cases(seed):
    for index, (inputs, outputs, noise, maps, rate) in enumerate(
        [
            (1, 1, 0, False, 44100),
            (2, 2, 0.001, False, 48000),
            (3, 2, 0.002, True, 96000),
            (2, 5, 0, True, 192000),
            (6, 4, 0.001, False, 48000),
            (16, 8, 0.001, True, 96000),
        ]
    ):
        data, truth = example(seed + index, inputs, outputs, noise=noise, maps=maps)
        data["sample_rate"] = rate
        yield f"linear-{inputs}x{outputs}", data, truth, True
    data, _ = example(seed + 50)
    identity = np.eye(2)
    for group in data["groups"]:
        group["desired_output"] = group["input"]
    yield "identity", data, identity, True
    data, _ = example(seed + 51, inputs=2, outputs=3, noise=0)
    silence = np.zeros((3, 2))
    for group in data["groups"]:
        group["desired_output"] = np.zeros((len(group["input"]), 3)).tolist()
    yield "explicit-silent-output", data, silence, True
    for offset, name in enumerate(
        [
            "collinear",
            "near-collinear",
            "underexcited",
            "fold-rank",
            "contradictory",
            "nonlinear-clipping",
            "coherent-overload",
        ]
    ):
        data, truth = example(seed + 100 + offset)
        if name == "coherent-overload":
            truth = np.array([[0.7, 0.7], [0.6, -0.6]])
        for index, group in enumerate(data["groups"]):
            x = np.array(group["input"])
            if name in {"collinear", "near-collinear"}:
                x[:, 1] = x[:, 0] + (np.arange(len(x)) % 2) * (
                    1e-6 if name == "near-collinear" else 0
                )
            if name == "underexcited":
                x *= 0.001
            if name == "fold-rank" and index:
                x[:, 1] = 0
            y = x @ truth.T
            if name == "contradictory" and index % 2:
                y *= -1
            if name == "nonlinear-clipping":
                y = np.clip(y, -0.08, 0.08)
            group.update(input=x.tolist(), desired_output=y.tolist())
        yield name, data, truth, False
