#!/usr/bin/env python3
"""Prepare a controlled old/new Peak CPU comparison using identical current dependencies.

This preserves recovered Rust source and copies it without modification. It is
not a reconstruction of the historical compiler or dependency closure.
"""

import argparse
import hashlib
import json
import shutil
import tarfile
import tomllib
from pathlib import Path


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def toml_value(value):
    if isinstance(value, dict):
        return "{ " + ", ".join(json.dumps(k) + " = " + toml_value(v) for k, v in value.items()) + " }"
    if isinstance(value, list):
        return "[" + ", ".join(toml_value(v) for v in value) + "]"
    return json.dumps(value)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    work = args.output.resolve()
    work.mkdir(parents=True, exist_ok=False)
    package = root / "crates/sotf-plugins/crates/sotf-plugin-dynamic-eq"
    archive = root / "crates/sotf-plugins/target/audit-artifacts/aud139-preedit-cpu-source-recovered-r1/selected-source.tar.gz"
    assert digest(archive) == "736c9b538e6511d0eb47a2296e8ccf73955445582980ad820b73d98049b0aa42"
    prefix = "sotf-daw/crates/sotf-plugins/crates/sotf-plugin-dynamic-eq/"
    old = work / "old"
    new = work / "new"
    old.mkdir()
    new.mkdir()
    with tarfile.open(archive) as source:
        for member in source.getmembers():
            if not member.isfile() or not member.name.startswith(prefix):
                continue
            relative = Path(member.name.removeprefix(prefix))
            assert not relative.is_absolute() and ".." not in relative.parts
            destination = old / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(source.extractfile(member).read())
    shutil.copytree(package / "src", new / "src")
    shutil.copy2(package / "Cargo.toml", new / "Cargo.toml")
    # Use the exact recovered helper in both variants. The live helper differs
    # only in rustfmt layout, but matching its bytes removes that qualification.
    harness = old / "tests/aud139_cpu_baseline.rs"
    assert digest(harness) == "7e48711da3adf514e91d2433180886308ad7607bfbd9be8d9337a9f3a1b4714a"
    (new / "tests").mkdir()
    shutil.copy2(harness, new / "tests/aud139_cpu_baseline.rs")
    workspace = tomllib.loads((root / "Cargo.toml").read_text())
    current = tomllib.loads((package / "Cargo.toml").read_text())
    common_package = workspace["workspace"]["package"]
    common_dependencies = workspace["workspace"]["dependencies"]
    manifest = ["[package]"]
    for key in ["name", "description", "version"]:
        manifest.append(f"{key} = {toml_value(current['package'][key])}")
    for key in ["edition", "license", "repository"]:
        manifest.append(f"{key} = {toml_value(common_package[key])}")
    manifest.extend(["", "[lib]", 'name = "sotf_plugin_dynamic_eq"', 'path = "src/lib.rs"',
                     "", "[features]", 'qa = ["sotf-host/qa"]', "", "[dependencies]"])
    for name in current["dependencies"]:
        dependency = common_dependencies[name]
        if isinstance(dependency, dict):
            dependency = dict(dependency)
            if "path" in dependency:
                dependency["path"] = str((root / dependency["path"]).resolve())
        manifest.append(f"{json.dumps(name)} = {toml_value(dependency)}")
    for source, patches in workspace.get("patch", {}).items():
        manifest.extend(["", f"[patch.{json.dumps(source)}]"])
        for name, dependency in patches.items():
            dependency = dict(dependency)
            if "path" in dependency:
                dependency["path"] = str((root / dependency["path"]).resolve())
            manifest.append(f"{json.dumps(name)} = {toml_value(dependency)}")
    release = workspace["profile"]["release"]
    manifest.extend(["", "[profile.release]"])
    manifest.extend(f"{json.dumps(k)} = {toml_value(v)}" for k, v in release.items() if not isinstance(v, dict))
    for variant in [old, new]:
        shutil.copy2(variant / "Cargo.toml", variant / "original-Cargo.toml")
        # Cargo may reuse a target artifact for same-name/version path packages
        # whose sources predate the previous build. Distinct package identities
        # force separate compile units while the explicit library name stays
        # identical for the byte-for-byte shared harness.
        variant_manifest = list(manifest)
        variant_manifest[1] = f'name = "aud139-dynamic-eq-cpu-{variant.name}"'
        (variant / "Cargo.toml").write_text("\n".join(variant_manifest) + "\n")
        shutil.copy2(root / "Cargo.lock", variant / "Cargo.lock")
        files = sorted(p for p in variant.rglob("*") if p.is_file())
        (variant / "initial-source.sha256").write_text("".join(f"{digest(p)}  {p.relative_to(variant)}\n" for p in files))
    receipt = {
        "scope": "Controlled old/new Peak source comparison under identical current dependency manifests, compiler and profile; not historical build recreation",
        "old_archive_sha256": digest(archive), "unchanged_shared_harness_sha256": digest(harness),
        "live_harness_sha256": digest(package / "tests/aud139_cpu_baseline.rs"),
        "root_lock_seed_sha256": digest(root / "Cargo.lock"),
        "build_manifest_sha256": {variant.name: digest(variant / "Cargo.toml") for variant in [old, new]}, "release_profile": release,
        "variants": [str(old), str(new)],
        "next_gate": "Normalize the temporary locks offline, verify resolved dependency sets equal excluding the two variant packages, then require separately compiled and bound executables before matched timings",
        "limitations": "No shelf CPU variant yet. Manual timing harness is not an accuracy oracle or worst-case execution time proof.",
    }
    (work / "preparation-receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps(receipt, indent=2))


if __name__ == "__main__":
    main()
