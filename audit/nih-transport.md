# AUD-039 implementation handoff

Production files: `plugins-nih/src/wrapper.rs`, `src/wrapper/transport.rs`.
Regression files: `src/wrapper/transport/tests.rs`, existing `src/wrapper_tests.rs` adapted to the shared processing core. Dev dependency `clap-sys` matches NIH's already locked git revision. Existing user Justfile edits were untouched.

## Contract

- Read native NIH transport each callback, preserving sample position, independent PPQ, finite positive tempo, representable positive time signature, play/record state and valid loop bounds.
- Prefer explicit host positions, including seeks and wraps. If the host omits position, continue a signed saturating cursor while playing; hold it while stopped. Retain the last valid tempo/signature, initially 120 BPM and 4/4.
- SOTF's sample position is unsigned: negative native preroll is clipped to zero at the output boundary but remains signed internally, and negative PPQ is retained. Clip loop bounds to the unsigned domain; discard reversed/empty ranges.
- Keep PPQ finite for malformed scalar inputs and extreme cursors. Reset the fallback state on initialization and reset.
- No allocation, lock, logging, parameter ID, preset or DSP audio changes in production mapping. Loop/preroll fields unavailable in NIH/SOTF cannot be invented.

## Verification

- `cargo test --offline -p plugins-nih --lib`: **39 passed**, log `/tmp/sotf-nih-transport-full.log`.
- `cargo clippy --offline -p plugins-nih --all-targets --features gain -- -D warnings`: passed, log `/tmp/sotf-nih-transport-clippy.log`.
- Five new tests include actual NIH CLAP callbacks (no fabricated private Transport): independent sample/PPQ origins; variable callbacks and seeks; tempo/signature changes; seconds-only/beats-only sources; omitted clocks, stop and reset; 2x/4x buffered oversampling and loop clock conversion; malformed values, negative preroll and saturation. Native callbacks run under allocation assertions from the first callback.
- Existing direct wrapper layout/parameter/allocation tests invoke the same processing body with an explicit absent-transport snapshot. A test-only trait lives only in the test module, preserving exported macro usability.

## Exact dependency observations for the next scope

NIH pinned revision `de421011f41a6d10fc8c7a6084e4f4dee0143683`:

1. `src/wrapper/clap/wrapper.rs:1973-2024`: parameter events split buffers only with `SAMPLE_ACCURATE_AUTOMATION`; transport events always split. Current SOTF macro enables the flag only for EQ/LinearPhaseEQ.
2. `:2190-2200`: advancing a split slice's seconds position is unnecessarily gated on valid tempo; with seconds-only transport and no tempo, each split slice can receive the same position. VST3 always offsets project sample time (`src/wrapper/vst3/wrapper.rs:1353`). This is upstream behavior, not solved by forwarding the available context. Do not compensate repeated positions blindly: a legitimate seek/repeat is indistinguishable through the current public NIH Transport API.
3. `ProcessStatus::Normal` maps to CLAP CONTINUE_IF_NOT_QUIET, `Tail(_)` and `KeepAlive` to CONTINUE (`clap/wrapper.rs:2258-2264`). The tail extension returns last Tail length, u32::MAX for KeepAlive, or zero (`:3178-3187`). VST3 uses the analogous mapping (`vst3/wrapper.rs:1672-1680`).
4. Native ProcessContext offers latency updates but no direct tail setter. The pinned CLAP wrapper imports only `clap_plugin_tail`/CLAP_EXT_TAIL, with no host tail-changed notification path found. Dynamic tail reporting needs framework behavior considered explicitly.
5. SOTF `Plugin::drain_output_frames_max` is a maximum output capacity for *one drain call*, explicitly documented at `sotf-host/src/plugin.rs:225-231`. It cannot be used as a total native tail estimate. Current trait offers no aggregate tail-length bound; do not declare a finite tail from this number.

Source is frozen. No native tail status or event-dispatch policy was changed in AUD-039.
