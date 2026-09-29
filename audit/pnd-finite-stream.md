# PND finite-stream completion — verified and frozen

## Implemented contract

PND now emits retained audio at EOF instead of returning immediate COMPLETE0. Its normal adaptive processing arithmetic, channel ordering and fixed latency D=2047 frames are unchanged. For a nonempty accepted stream of S frames, exclusive output endpoint is `D + floor((S-1)/512)*512 + 2048`, giving 3583..4094 continuation frames. Each successful call processes at most one 512-frame hop and preserves the destination suffix.

The first valid nonempty drain latches the exact existing effective correction formula using `current_ratio` and the CURRENT smoothed strength. It bypasses external analyzer feeding, consensus, drift/reference updates, strength smoothing and diagnostic cadence while preserving the vocoder's FFT/onset/phase/OLA synthesis bookkeeping. Synthetic input never increases the accepted source phase or drain budget.

Initialized tail metadata is `Finite(4094)`; uninitialized metadata is Unknown. The spectral agent's new host API is implemented only as a PND override: `drain_call_bound` reports ceil(current_remaining/512), at least one for empty/completed state, and None before initialization. No host source was edited by this work.

EOF lifecycle:

- Validate initialization, whole output frames, matching rate and pending capacity before changing state/output.
- Empty EOF returns COMPLETE0 without freezing future input; completed EOF is stable.
- Accepted nonempty EOF rejects further input and changed parameters until reset or successful initialize. Zero-frame process remains a no-op.
- Identical recognized parameter snapshots, including structural values, return early without validation allocations, metadata rebuilds or analyzer resets. Unknown IDs and wrong types still fail.
- Reset clears source phase, pending EOS, prepared zeros, consensus and audio/controller state without allocation. Successful initialize now resets learned ratios; invalid reinitialization preserves the active stream.

## Files

All paths below are within `crates/sotf-plugins/crates/sotf-plugin-pnd`:

- `src/lib/pnd_plugin.rs`: bounded drain, state/metadata/lifecycle and optional fixed-ratio dispatch.
- `src/lib/phase_vocoder.rs`, `src/lib/phase_vocoder_channel.rs`: test-only Clone derives for complete pre-EOF vocoder snapshots; no synthesis arithmetic change.
- `src/lib.rs`: register private oracle tests.
- New `src/lib/finite_stream_tests.rs`: three adaptive-history/support/lifecycle tests.
- New `tests/finite_stream.rs`: five public neutral/count/capacity/lifecycle tests.
- New `tests/drain_realtime.rs`: one cold allocation/deallocation test.
- `README.md`, `CHANGELOG.md`: finite-stream contract and limitations.

No constructor/schema/index, Cargo manifest/lock, host, engine, MIDI/IAMF or unrelated plugin changes.

## Evidence and oracles

Red: `target/audit-pnd-finite-red.log`. A 17-frame stereo stream with its final marker returned only 34 samples, versus the independent expected 8190 samples including finite support. The regression now passes.

Public neutral coverage:

- All 512 terminal hop phases with first/final markers and dense interior waveform; irregular input blocks1/17/257 and changing drain capacities1/7/512/8192.
- 252 boundary cases: lengths1/511/512/513/2047/2048/2049 ×channels1/2/6 ×rates44.1/48/96k ×drain capacities1/7/512/8192.
- Every case matches ordinary zero-padded processing BIT EXACTLY at correction_strength0, proving EOS introduces no synthesis difference.
- Independent delayed-source measurements, log `target/audit-pnd-neutral-measurements.log`:
  - 512 hop phases: maximum absolute error **1.7881393432617188e-7**, minimum SNR **132.805304 dB**. Regression limits5e-7 and125dB.
  - 252 boundary cases: maximum absolute error **2.484693250153214e-4**, minimum SNR **62.719141 dB**. Regression limits5e-4 and60dB.
- The initial guessed2e-5 pointwise tolerance failed because the unchanged phase-locking algorithm is not exact unity identity. Exact ordinary-padding parity isolated this from EOS; the calibrated limits above retain modest floating-point headroom and are stronger than the existing35dB unity contract. Existing committed thresholds elsewhere were not changed.
- Counts follow the independent support formula, destination suffix canaries remain untouched, completion is bounded, reset/reinitialize match fresh state, and failed capacity/rate/initialize/control/input calls preserve output and retained history.

Private oracle:

- Public reference-pilot input learns nonunity correction in both directions. Formant off/on and a final marker exercise actual retained synthesis state.
- A target-strength change just before EOF leaves current strength visibly different; expected q is calculated directly from private current controller state.
- Test-only Clone captures ALL vocoder fields after adaptive processing, including input/OLA, previous magnitudes/phases, peak ownership and onset state. The expected side uses a separate scalar zero-input scheduler and calls the existing spectral kernel directly; it never invokes plugin drain or fixed dispatch.
- Expected continuation matches exactly. Controller snapshots (ratio, drift, generation, transition, strength/current strength, consensus, diagnostic cadence and analyzer generation/confidence/matched peaks) remain unchanged after every drain call.
- Warm nonzero phase/envelope history followed by sufficient zero windows produces EXACT zero output for q0.95,1,1/0.95 and formant strength0/1; no magnitude-floor audio injection persists beyond support.
- Failed Nyquist reinitialization preserves learned history; successful initialization resets ratios and analyzers.

Realtime:

Fresh-thread first drain, first FFT during drain, small repeated capacities, complete/empty calls, metadata/bound getters, identical snapshots, reset and replay have **zero allocations and zero deallocations**, across1/2/6channels and accepted lengths1/511/513. Work per drain call is bounded by512 frames per channel. Setup/initialize and changed ordinary setters remain outside that cold success-path claim.

## Final gates

- `cargo test -p sotf-plugin-pnd --all-features`: **85 passed** =59 unit (3new),1cold,5public finite,14integration,6plugin. No ignored tests. Log `target/audit-pnd-finite-final.log`.
- `cargo clippy -p sotf-plugin-pnd --all-features --all-targets -- -D warnings`: clean. Log `target/audit-pnd-finite-clippy.log`.
- Scoped rustfmt and git diff --check: clean.

Nine new tests total. Source and tests frozen. Finite-support proof assumes finite valid DSP state and normal-amplitude numerical fixtures; this is not a huge-amplitude overflow correction or proprietary pitch-shifter parity claim. Fixed-q continuation is an EOS dispatch oracle, not an independent validation of the existing spectral algorithm.

Root implementation review found no blocker in the fixed-ratio dispatch,
estimator/diagnostic freeze, preflight and zero-buffer restoration, finite count,
native work bound or initialization reset. The normal processing route still
uses the existing adaptive arithmetic. This was source inspection, with no
additional PND test execution or DSP changes.
