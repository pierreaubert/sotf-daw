#!/usr/bin/env python3
"""Capture the reviewable AUD142 post-core source checkpoint."""

from __future__ import annotations

import gzip
import hashlib
import json
import tarfile
from pathlib import Path


ARTIFACT_DIR = Path(__file__).resolve().parent
ROOT = ARTIFACT_DIR.parents[2]
PACKAGE = ROOT / "crates/sotf-plugins/crates/sotf-plugin-crossover"

EXTRA_PATHS = [
    "Cargo.toml",
    "Cargo.lock",
    "crates/sotf-plugins/Cargo.toml",
    "crates/sotf-engine/src/plugins/plugin_settings.rs",
    "crates/sotf-engine/src/plugins/plugin_type.rs",
    "crates/sotf-engine/src/plugins/plugin_config_converter.rs",
    "crates/sotf-engine/src/plugins/plugin_config_converter/spatial.rs",
    "crates/sotf-engine/src/plugin_param_accessors/crossover.rs",
    "crates/sotf-plugins/crates/plugins-ffi/src/lib.rs",
    "crates/sotf-plugins/crates/plugins-ffi/src/parameter_map.rs",
    "crates/sotf-plugins/crates/plugins-ffi/src/lib/plugin.rs",
    "crates/sotf-plugins/crates/plugins-ffi/src/plugin_factory.rs",
    "crates/sotf-plugins/crates/plugins-ffi/src/lib/tests.rs",
    "crates/sotf-plugins/crates/plugins-ffi/src/lib/state_tests.rs",
    "crates/sotf-plugins/crates/plugins-ffi/src/lib/aud142_crossover_baseline_tests.rs",
]


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


package_paths = sorted(
    path
    for path in PACKAGE.rglob("*")
    if path.is_file() and "target" not in path.relative_to(PACKAGE).parts
)
extra_paths = [ROOT / relative for relative in EXTRA_PATHS]
missing = [path for path in extra_paths if not path.is_file()]
if missing:
    raise FileNotFoundError("missing selected source paths: " + ", ".join(map(str, missing)))
selected = sorted({path for path in package_paths + extra_paths})
relative_names = {path: path.relative_to(ROOT).as_posix() for path in selected}
captured_hashes = {path: sha256(path) for path in selected}

manifest_path = ARTIFACT_DIR / "core-source-r1.sha256"
manifest_path.write_text(
    "".join(f"{captured_hashes[path]}  {relative_names[path]}\n" for path in selected)
)

archive_path = ARTIFACT_DIR / "core-source-r1.tar.gz"
with archive_path.open("wb") as archive_file:
    with gzip.GzipFile(filename="", mode="wb", fileobj=archive_file, mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode="w") as archive:
            for path in selected:
                archive.add(path, arcname=relative_names[path], recursive=False)

post_capture_mismatches = [path for path in selected if sha256(path) != captured_hashes[path]]
if post_capture_mismatches:
    raise RuntimeError(
        "selected sources changed during capture: " + ", ".join(map(str, post_capture_mismatches))
    )

with tarfile.open(archive_path, mode="r:gz") as archive:
    archived_hashes = {}
    for path in selected:
        member = archive.extractfile(relative_names[path])
        if member is None:
            raise RuntimeError(f"archive is missing {relative_names[path]}")
        archived_hashes[path] = hashlib.sha256(member.read()).hexdigest()
if archived_hashes != captured_hashes:
    raise RuntimeError("archive content hashes do not match the selected-source manifest")

receipt = {
    "checkpoint": "AUD142 post-pure-core, before AUD144 and broader Crossover FFI/engine route edits",
    "selected_file_count": len(selected),
    "selected_crossover_package_files": len(package_paths),
    "extra_route_and_workspace_files": EXTRA_PATHS,
    "archive": archive_path.relative_to(ROOT).as_posix(),
    "archive_sha256": sha256(archive_path),
    "manifest": manifest_path.relative_to(ROOT).as_posix(),
    "manifest_sha256": sha256(manifest_path),
    "sources_unchanged_during_capture": True,
    "archive_contents_match_manifest": True,
    "source_root": ROOT.as_posix(),
    "recovery": "Extract the archive from the repository root, then verify core-source-r1.sha256 with sha256sum -c.",
}
receipt_path = ARTIFACT_DIR / "core-source-r1-receipt.json"
receipt_path.write_text(json.dumps(receipt, indent=2) + "\n")
print(json.dumps(receipt, indent=2))
