#!/usr/bin/env python3
"""Generate mono golden audio from unchanged upstream DFN2 Rust sources.

This uses a minimal inference manifest, the release's pinned Tract revision,
and the upstream default allocator. The reference crate stays outside the repo;
only synthetic inputs, golden outputs, hashes, and build evidence are retained.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import platform
import random
import shutil
import struct
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

REVISION = "af5d24a125cc20759f483d4e354fdf70f7951024"
TRACT = "ed00ff7eadc84b345318b042f134602ed743879d"
MODEL_SHA = "412e81d8d1779b8f925ac4b53472655a9f866c4af73dbf930ab587359f631628"
SOURCES = {
    "lib.rs": "5dfa85aa3077e21faac48bee574f0983c831ae30f4b05759207961a05cb995e5",
    "tract.rs": "3966fdf513c714848bfdd55b54ca807be13d8964a3d0bce900f537f530927d94",
    "transforms.rs": "e307e0d00ac3a9379ce21b3fa3d8d4bc651a803dac2e0ff9a00cfe0d5e835148",
}
MAIN = r"""
use anyhow::{ensure, Context, Result};
use ndarray::{ArrayView2, ArrayViewMut2};
use std::{env, fs, path::PathBuf};
use upstream_df::tract::{DfParams, DfTract, ReduceMask, RuntimeParams};

fn main() -> Result<()> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    ensure!(args.len() == 4, "Expected model, input, waveform, score paths");
    let input = fs::read(&args[1]).context("Read input PCM")?;
    ensure!(input.len().is_multiple_of(480 * 4), "Expected whole mono hops");
    let samples: Vec<_> = input.as_chunks::<4>().0.iter()
        .map(|bytes| f32::from_le_bytes(*bytes)).collect();
    ensure!(samples.iter().all(|x| x.is_finite()), "Non-finite input PCM");
    let params = DfParams::new(PathBuf::from(&args[0]))?;
    let policy = RuntimeParams::new(1, false, 100., -10., 30., 20., ReduceMask::MEAN);
    let mut model = DfTract::new(params, &policy)?;
    let mut waveform = Vec::with_capacity(input.len());
    let mut scores = Vec::new();
    for frame in samples.as_chunks::<480>().0 {
        let mut enhanced = [0f32; 480];
        let score = model.process(
            ArrayView2::from_shape((1, 480), frame)?,
            ArrayViewMut2::from_shape((1, 480), &mut enhanced)?,
        )?;
        ensure!(score.is_finite() && enhanced.iter().all(|x| x.is_finite()), "Non-finite reference output");
        scores.extend_from_slice(&score.to_le_bytes());
        for value in enhanced { waveform.extend_from_slice(&value.to_le_bytes()); }
    }
    fs::write(&args[2], waveform)?;
    fs::write(&args[3], scores)?;
    Ok(())
}
"""


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def write_if_changed(path: Path, data: bytes) -> None:
    """Keep identical reference sources untouched so Cargo can reuse its cache."""
    if not path.exists() or path.read_bytes() != data:
        path.write_bytes(data)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work-dir", required=True, type=Path)
    parser.add_argument("--target-dir", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--source-dir", type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    args.work_dir = args.work_dir.resolve()
    if args.work_dir.is_relative_to(root):
        raise ValueError(
            "The generated reference crate must stay outside this repository"
        )
    existing_manifest = args.work_dir / "Cargo.toml"
    if (
        args.work_dir.exists()
        and any(args.work_dir.iterdir())
        and (
            not existing_manifest.exists()
            or 'name = "dfn2-reference-evidence"' not in existing_manifest.read_text()
        )
    ):
        raise ValueError(
            "Use an empty work directory or this tool's existing reference crate"
        )
    model = (
        root / "crates/sotf-plugins/models/deepfilternet2/DeepFilterNet2_onnx.tar.gz"
    )
    if digest(model.read_bytes()) != MODEL_SHA:
        raise ValueError("Reference model hash mismatch")
    args.output.mkdir(parents=True, exist_ok=True)
    upstream = args.work_dir / "upstream"
    (upstream / "src").mkdir(parents=True, exist_ok=True)
    for name, expected in SOURCES.items():
        if args.source_dir:
            data = (args.source_dir / name).read_bytes()
        else:
            url = f"https://raw.githubusercontent.com/Rikorose/DeepFilterNet/{REVISION}/libDF/src/{name}"
            with urllib.request.urlopen(url, timeout=60) as response:
                data = response.read()
        if digest(data) != expected:
            raise ValueError(f"Upstream source mismatch: {name}")
        write_if_changed(upstream / "src" / name, data)
    manifest = """[package]
name = "deep_filter"
version = "0.3.1"
edition = "2021"
license = "MIT OR Apache-2.0"
[lib]
name = "df"
[features]
default = ["tract"]
tract = ["transforms"]
transforms = []
timings = []
dataset = []
cache = []
wav-utils = []
[dependencies]
rustfft = "=6.4.1"
realfft = "=3.5.0"
num-complex = { version = "=0.4.6", features = ["serde"] }
log = "0.4"
ndarray = "=0.15.6"
rubato = "=0.12.0"
thiserror = "1"
anyhow = "1"
rust-ini = "=0.18.0"
flate2 = "1"
tar = "0.4"
"""
    for name in ["tract-core", "tract-onnx", "tract-pulse", "tract-hir"]:
        manifest += (
            f'{name} = {{ git = "https://github.com/sonos/tract", rev = "{TRACT}" }}\n'
        )
    write_if_changed(upstream / "Cargo.toml", manifest.encode())
    wrapper_manifest = """[package]
name = "dfn2-reference-evidence"
version = "0.1.0"
edition = "2024"
[workspace]
[dependencies]
upstream-df = { package = "deep_filter", path = "upstream" }
ndarray = "=0.15.6"
anyhow = "1"
[profile.release]
lto = "thin"
codegen-units = 14
"""
    write_if_changed(args.work_dir / "Cargo.toml", wrapper_manifest.encode())
    (args.work_dir / "src").mkdir(exist_ok=True)
    write_if_changed(args.work_dir / "src/main.rs", MAIN.encode())
    existing_lock = (args.work_dir / "Cargo.lock").exists()
    if not existing_lock:
        shutil.copyfile(root / "Cargo.lock", args.work_dir / "Cargo.lock")
    rng = random.Random(20261024)
    pcm = []
    for n in range(64 * 480):
        t = n / 48_000
        packet = n // 480
        if packet < 8 or 40 <= packet < 48:
            sample = 0.0
        elif packet < 24:
            phase = math.tau * (150 * t + 2 * math.sin(math.tau * 0.8 * t))
            sample = 0.14 * (math.sin(phase) + 0.4 * math.sin(2 * phase))
            sample += 0.02 * rng.uniform(-1, 1)
        elif packet < 40:
            sample = 0.04 * math.sin(math.tau * 3100 * t) + 0.16 * rng.uniform(-1, 1)
        else:
            sample = 0.12 * math.sin(math.tau * 347 * t)
            sample += 0.7 if n % 2111 == 0 else 0
        pcm.append(sample)
    input_path = args.output / "input.f32le"
    input_path.write_bytes(struct.pack(f"<{len(pcm)}f", *pcm))
    wave_path, score_path = args.output / "waveform.f32le", args.output / "scores.f32le"
    command = [
        "rtk",
        "proxy",
        "cargo",
        "run",
        "--release",
        "--jobs",
        "2",
        "--manifest-path",
        str(args.work_dir / "Cargo.toml"),
        "--target-dir",
        str(args.target_dir),
    ]
    command += ["--", str(model), str(input_path), str(wave_path), str(score_path)]
    start = time.time()
    with (args.output / "reference-build.stdout.txt").open("w") as log:
        result = subprocess.run(
            command, stdout=log, stderr=subprocess.STDOUT, check=False
        )
    metadata = {
        "upstream_revision": REVISION,
        "upstream_source_sha256": SOURCES,
        "source_policy": "All three upstream Rust source files are byte-for-byte unchanged; minimal inference manifest, pinned matching Tract, upstream default allocator",
        "tract_revision": TRACT,
        "model_sha256": MODEL_SHA,
        "command": command,
        "exit_code": result.returncode,
        "started_unix": start,
        "finished_unix": time.time(),
        "sample_rate": 48000,
        "channels": 1,
        "hop_frames": 480,
        "packets": 64,
        "input_recipe": "Seed 20261024, silence / harmonic signal plus noise / noisy 3100 Hz tone / silence / tone plus impulses; synthetic, not held-out quality evidence",
        "python": sys.version,
        "platform": platform.platform(),
        "rustc": subprocess.check_output(
            ["rtk", "proxy", "rustc", "-Vv"], text=True
        ).strip(),
        "cargo": subprocess.check_output(
            ["rtk", "proxy", "cargo", "--version"], text=True
        ).strip(),
        "generator_sha256": digest(Path(__file__).read_bytes()),
        "wrapper_sha256": digest(MAIN.encode()),
        "minimal_upstream_manifest_sha256": digest(manifest.encode()),
        "files": {},
    }
    if result.returncode == 0:
        for path in [input_path, wave_path, score_path]:
            metadata["files"][path.name] = {
                "bytes": path.stat().st_size,
                "sha256": digest(path.read_bytes()),
            }
        shutil.copyfile(
            args.work_dir / "Cargo.lock", args.output / "reference-Cargo.lock"
        )
        metadata["reference_lock_sha256"] = digest(
            (args.output / "reference-Cargo.lock").read_bytes()
        )
    (args.output / "provenance.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(
        json.dumps(
            {"exit_code": result.returncode, "output": str(args.output)}, indent=2
        )
    )
    raise SystemExit(result.returncode)


if __name__ == "__main__":
    main()
