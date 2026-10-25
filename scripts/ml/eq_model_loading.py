"""Bounded loading of verified offline EQ weights into an exact architecture."""

import hashlib
import json
from pathlib import Path

import torch


def read_bounded_bytes(path, limit):
    with Path(path).open("rb") as source:
        raw = source.read(limit + 1)
    if len(raw) > limit:
        raise ValueError("EQ model file exceeds size limit")
    return raw


def load_model(path, contract, factory):
    path = Path(path)
    report = json.loads(
        read_bounded_bytes(path.parent / "training-report.json", 1024 * 1024)
    )
    if not isinstance(report, dict):
        raise TypeError("EQ training report must be an object")
    raw = read_bounded_bytes(path, 8 * 1024 * 1024)
    digest = hashlib.sha256(raw).hexdigest()
    if digest != report.get("weights_sha256"):
        raise ValueError("EQ model weights SHA-256 mismatch")
    document = json.loads(raw)
    if not isinstance(document, dict) or document.get("contract") != contract:
        raise ValueError("Unsupported EQ model contract")
    weights = document.get("weights")
    model = factory()
    expected = model.state_dict()
    if not isinstance(weights, dict) or weights.keys() != expected.keys():
        raise ValueError("EQ model parameter names mismatch")
    state = {}
    for name, reference in expected.items():
        try:
            value = torch.as_tensor(weights[name], dtype=reference.dtype)
        except (TypeError, ValueError, RuntimeError, OverflowError) as error:
            raise ValueError("Invalid EQ numeric weights") from error
        if value.shape != reference.shape or not torch.isfinite(value).all():
            raise ValueError("Invalid EQ weight shape or non-finite value")
        state[name] = value
    model.load_state_dict(state, strict=True)
    model.eval()
    return model, digest
