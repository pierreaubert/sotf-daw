# Release checkpoint verification

Date: 2026-10-03. Root executed these checks on Linux. This is a merge checkpoint, not completion of the original feature audit or certification of release packages.

All Cargo commands used `--offline --locked`, shared target `crates/sotf-plugins/target`, and `/tmp/sotf-daw-audit-cargo.lock`. Running-engine checks used the named ALSA null device (`ALIM_E2E_DEVICE='SOTF Audit Null'` and the checked-in audit null configuration).

| Gate | Result |
| --- | --- |
| `cargo check --workspace --exclude plugins-ffi --lib --bins --tests --examples` | PASS |
| `cargo clippy --workspace --exclude plugins-ffi --all-targets -- -D warnings` | PASS after mechanical test lint fixes; vendored `nih_plug` still emits dependency warnings |
| Broad `cargo nextest run --workspace --exclude plugins-ffi --exclude sotf-midi --exclude sotf-iamf --lib --bins --tests --examples --no-fail-fast` | Initial run: 7,385 passed, 10 failed, 58 skipped out of 7,395 executed. Every failure is accounted for below. |
| Host/AB focused `cargo test -p sotf-host -p sotf-plugin-ab-compare --lib --tests` (also ran Denoiser in the same command) | Host and AB targets passed; host lib 665 passed/1 ignored; AB lib 94, variable-rate 50, composition 15, protocol 27, property 5 passed. |
| Declick `cargo test -p sotf-plugin-declick --no-fail-fast` | 131 passed, 0 failed, 5 ignored, including doctest. Separate active external-corpus and boundary anatomy checks passed. |
| Denoiser `cargo test -p sotf-plugin-denoiser --lib --tests --no-fail-fast` | Final retry: 109 passed, 0 failed, 4 ignored. |
| Engine `cargo test -p sotf-engine --test denoiser_configuration` | Final retry: 5 passed |
| FFI `cargo test -p plugins-ffi --lib denoiser` / `declick_ffi` | 3 / 18 passed; Denoiser rerun after the fade correction |
| NIH `cargo test -p plugins-nih --features declick --lib` | 227 passed, 1 ignored |
| Convolution `cargo test -p sotf-plugin-convolution --test integration --test direct_convolution --test finite_stream` | Final retry: 28 passed, 3 ignored |
| `cargo run -p sotf-plugin-denoiser --features qa --bin qa-denoiser` | ALL PASS, including 8-channel timing matrix |
| Facade `cargo test -p sotf-plugins --test aud140_channel_changing_eof` | Final: 13 passed, 3 ignored |
| Staged whitespace check excluding archived `*.patch` | PASS; the preserved experimental patch contains required diff context spaces |
| Workspace formatting | Global check finds pre-existing vendored formatting differences; 160 changed first-party Rust files were formatted. |

## Disposition of the ten initial broad failures

1. Engine `play_sends_flush_before_ack`: timeout under parallel load; serial retry PASS.
2. Host `isolated_control_state_and_audio_share_transport_without_stale_sidecar`: completion timing under parallel load; serial retry PASS.
3. Facade `test_limiter_plugin_timing`: 6.095 ms P99 under parallel load; serial retry PASS, unchanged timing threshold.
4. Denoiser fade multirate unit test: corrected endpoint snap to bound the complete step; retry PASS.
5. Denoiser audition multirate test: corrected process context to the initialized sample rate; retry PASS.
6. Denoiser audition retoggle test: align the independent replica with actual callback-boundary toggle frames; retry PASS.
7. Engine legacy Denoiser audio: explicitly match the legacy 12 dB setting instead of comparing against a 10 dB default; exact audio retry PASS.
8. Convolution reset: replace racing asynchronous IR activation with nonzero-state deterministic reset fixture; retry PASS. Exact assertions retained; synchronous loaded-IR reset coverage remains active.
9. FIR M1 multiband accuracy: known pre-existing limitation, −0.245743895 dB error for 96 kHz/1024 taps/phase 0/500 Hz. Explicitly deferred/ignored under the user's release scope. Original stimulus and bounds retained; no accuracy improvement claimed.
10. Facade channel-changing drain preflight: root instrumentation found that `rebuild_graph_drain_plan` legitimately prepares the alleged oversized emission (required/prepared output 262146/262146). The fixture must increase its declaration after preparation to test genuinely unavailable capacity. The fixture now increases the bound after preparation, preserving all rejection and no-consumption assertions. Final target: 13 passed, 3 ignored. No production host correction was required.

The three serial timing retries used nextest `--test-threads 1` with a filter selecting exactly those tests: 3 passed. No timing bound or production code was changed for them. This focused evidence does not make the initial broad invocation a passing run.

## Deferred tests and release limits

Declick: diagnostic R44 corpus anatomy, new R48 tone/quadratic/ramp accuracy, and real-music repair accuracy. Denoiser: four blind-quality characterizations (stationary tone, transient, burst diagnostic, ML diagnostic). FIR: M1 multiband response accuracy. These remain open in [the backlog](../../RELEASE-DEFERRED.md); ignored is not passed. Other skips predate this stabilization and are preserved.

MIDI and IAMF were excluded from direct broad testing per the user; dependencies may compile. Full multi-hour QA, fresh loaded native binaries, physical audio devices, macOS/iOS/AU, signing and packaging remain release-platform follow-ups. Prior native/sibling receipts are historical evidence, not a fresh platform certification of this checkpoint.

Raw logs remain in `checkpoint-gates/` and `declick-gates/` locally. Concise command manifests and this receipt belong in the checkpoint commit; raw event streams and build artifacts do not.
