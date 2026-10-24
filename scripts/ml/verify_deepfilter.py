#!/usr/bin/env python3
"""Verify the pinned DFN2 archive and the local inference runtime sources."""

from __future__ import annotations

import argparse
import configparser
import hashlib
import json
import tarfile
from pathlib import Path


def verify(root: Path) -> dict:
    """Check member identities and configured geometry without extracting files."""
    bundle = root / "crates/sotf-plugins/models/deepfilternet2"
    manifest = json.loads((bundle / "manifest.json").read_text())
    archive = bundle / manifest["model"]
    with archive.open("rb") as archive_file:
        actual = hashlib.file_digest(archive_file, "sha256").hexdigest()
    if actual != manifest["sha256"]:
        raise ValueError("Model archive SHA-256 mismatch")
    expected = manifest["files"]
    with tarfile.open(archive, "r:gz") as tar:
        members = tar.getmembers()
        names = [member.name for member in members]
        if len(names) != len(set(names)) or set(names) != set(expected):
            raise ValueError("Unexpected, missing, or repeated archive member")
        config = None
        for member in members:
            if not member.isfile():
                raise ValueError(f"Archive member is not a regular file: {member.name}")
            stream = tar.extractfile(member)
            if stream is None:
                raise ValueError(f"Unreadable archive member: {member.name}")
            data = stream.read()
            identity = expected[member.name]
            if len(data) != identity["bytes"]:
                raise ValueError(f"Member size mismatch: {member.name}")
            if hashlib.sha256(data).hexdigest() != identity["sha256"]:
                raise ValueError(f"Member hash mismatch: {member.name}")
            if member.name.endswith("config.ini"):
                config = configparser.ConfigParser()
                config.read_string(data.decode())
    if config is None or config["train"]["model"] != "deepfilternet2":
        raise ValueError("Expected DeepFilterNet2 configuration")
    df, network = config["df"], config["deepfilternet"]
    lookahead = int(network["conv_lookahead"]) + int(network["df_lookahead"])
    hop, fft = int(df["hop_size"]), int(df["fft_size"])
    geometry = {
        "model_clock_hz": int(df["sr"]),
        "hop_frames": hop,
        "fft_frames": fft,
        "model_lookahead_hops": lookahead,
        "intrinsic_delay_frames": fft - hop + lookahead * hop,
    }
    for key, value in geometry.items():
        if manifest[key] != value:
            raise ValueError(f"Manifest geometry mismatch: {key}")
    runtime = root / "vendor/deep-filter-runtime"
    provenance = json.loads((runtime / "upstream.json").read_text())
    for name, digest in provenance["local_sha256"].items():
        if hashlib.sha256((runtime / name).read_bytes()).hexdigest() != digest:
            raise ValueError(f"Local runtime source hash mismatch: {name}")
    reference_files = 0
    reference = manifest.get("reference_golden")
    if reference:
        path = bundle / reference["path"]
        data = path.read_bytes()
        if hashlib.sha256(data).hexdigest() != reference["sha256"]:
            raise ValueError("Upstream reference provenance hash mismatch")
        golden = json.loads(data)
        if (
            golden["exit_code"] != 0
            or golden["model_sha256"] != actual
            or golden["upstream_revision"] != provenance["revision"]
            or golden["tract_revision"] != provenance["tract_revision"]
        ):
            raise ValueError("Reference runner identity mismatch")
        for name, expected_hash in provenance["source_sha256"].items():
            if golden["upstream_source_sha256"].get(name) != expected_hash:
                raise ValueError(f"Reference upstream source mismatch: {name}")
        expected_names = {"input.f32le", "waveform.f32le", "scores.f32le"}
        if set(golden["files"]) != expected_names:
            raise ValueError("Unexpected reference PCM files")
        for name, identity in golden["files"].items():
            pcm = (path.parent / name).read_bytes()
            if (
                len(pcm) != identity["bytes"]
                or hashlib.sha256(pcm).hexdigest() != identity["sha256"]
            ):
                raise ValueError(f"Reference PCM identity mismatch: {name}")
            reference_files += 1
        lock = (path.parent / "reference-Cargo.lock").read_bytes()
        if hashlib.sha256(lock).hexdigest() != golden["reference_lock_sha256"]:
            raise ValueError("Reference dependency lock mismatch")
        generator = (root / "scripts/ml/deepfilter_reference.py").read_bytes()
        if hashlib.sha256(generator).hexdigest() != golden["generator_sha256"]:
            raise ValueError("Reference generator source mismatch")
    return {
        "archive_sha256": actual,
        "verified_members": len(members),
        "geometry": geometry,
        "runtime_revision": provenance["revision"],
        "tract_revision": provenance["tract_revision"],
        "verified_runtime_files": len(provenance["local_sha256"]),
        "verified_reference_pcm_files": reference_files,
        "scope": "Artifact integrity; no enhancement quality or runtime acceptance claim",
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root", type=Path, default=Path(__file__).resolve().parents[2]
    )
    args = parser.parse_args()
    print(json.dumps(verify(args.root), indent=2))


if __name__ == "__main__":
    main()
