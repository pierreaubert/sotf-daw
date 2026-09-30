#!/usr/bin/env python3
"""Write a checksum index for retained AUD142 source and result artifacts."""

from __future__ import annotations

import hashlib
from pathlib import Path


ARTIFACT_DIR = Path(__file__).resolve().parent
ROOT = ARTIFACT_DIR.parents[2]
INDEX = ARTIFACT_DIR / "packet-sha256.txt"
SOURCES = [
    ROOT / "audit/artifacts/aud142-post-core",
    ROOT / "audit/artifacts/aud142-preedit",
    ROOT / "audit/artifacts/aud142-cpu-root-check-r1",
]


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


files = sorted(
    path
    for source_dir in SOURCES
    for path in source_dir.rglob("*")
    if path.is_file() and path != INDEX
)
INDEX.write_text(
    "".join(f"{sha256(path)}  {path.relative_to(ROOT).as_posix()}\n" for path in files)
)
print(f"indexed_file_count={len(files)}")
print(f"index_sha256={sha256(INDEX)}")
print(f"index_path={INDEX.relative_to(ROOT).as_posix()}")
