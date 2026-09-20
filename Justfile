# --------------------------------------------------------- -*- just -*-
# sotf-daw: DAW core workspace (engine, plugins, MIDI, IAMF, drivers).
#
# Invoke from this directory so recipes resolve against sotf-daw/Cargo.toml.
# ----------------------------------------------------------------------

_default:
	just --list

import 'crates/sotf-plugins/Justfile'
import 'crates/sotf-engine/Justfile'

# ----------------------------------------------------------------------
# VARIABLES
# ----------------------------------------------------------------------

# plugins-ffi's build script shells out to a nested `cargo metadata`, which
# needs registry write access unavailable in some sandboxes (see AGENTS.md).
# Workspace-wide passes skip it there; its own gates (qa-ffi, lint-ffi)
# still cover it — run those on a normal host when touching FFI.
ffi_exclude := "--exclude plugins-ffi"

# ----------------------------------------------------------------------
# TEST (same target names as sotf)
# ----------------------------------------------------------------------

[group('test')]
check:
	cargo check --workspace {{ffi_exclude}} --lib --bins --tests --examples

[group('test')]
test:
	cargo test --workspace {{ffi_exclude}} --lib --bins --tests --examples

# ----------------------------------------------------------------------
# LINT (same target name as sotf)
# ----------------------------------------------------------------------

[group('lint')]
lint:
	cargo clippy --workspace {{ffi_exclude}} --all-targets -- -- -D warnings

# ----------------------------------------------------------------------
# QA (same target name as sotf: umbrella over the engine/plugin gates)
# ----------------------------------------------------------------------

[group('qa')]
qa: qa-plugins qa-engine

# ----------------------------------------------------------------------
# COVERAGE (same target names as sotf)
# ----------------------------------------------------------------------

# Requires: cargo install cargo-llvm-cov
[group('coverage')]
coverage:
	cargo llvm-cov --workspace {{ffi_exclude}} --lib --bins --tests --examples --lcov --output-path target/lcov.info

# Generates an HTML coverage report and opens it.
[group('coverage')]
coverage-html:
	cargo llvm-cov --workspace {{ffi_exclude}} --lib --bins --tests --examples --html --open

# Prints a text summary to stdout (fastest coverage recipe).
[group('coverage')]
coverage-summary:
	cargo llvm-cov --workspace {{ffi_exclude}} --lib --bins --tests --examples --text --summary-only

# Per-package report used by the coverage ratchet. Mirrors sotf's
# coverage-core for the crates that moved here (sotf-player stays in sotf).
[group('coverage')]
coverage-core:
	mkdir -p target/coverage
	cargo llvm-cov --package sotf-engine --no-default-features --lib --tests --json --summary-only --fail-under-lines 55 --output-path target/coverage/sotf-engine.json
	cargo llvm-cov --package sotf-plugins --lib --tests --features qa --json --summary-only --output-path target/coverage/sotf-plugins.json

# Removes stale coverage artifacts.
[group('coverage')]
coverage-clean:
	cargo llvm-cov clean

# ----------------------------------------------------------------------
# FORMAT (same target names as sotf)
# ----------------------------------------------------------------------

alias format := fmt

fmt:
	cargo fmt --all

# ----------------------------------------------------------------------
# CLEAN (same target name as sotf)
# ----------------------------------------------------------------------

# Unlike sotf's clean, this keeps the committed Cargo.lock.
clean:
	cargo clean
	find . -name '*~' -exec rm {} \; -print
