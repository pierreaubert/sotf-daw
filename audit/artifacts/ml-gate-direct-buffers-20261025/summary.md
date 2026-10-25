# Whole-callback learned Gate integration

Tracking: private sotf-daw issue 10, full ML audit (31 plugin proposals and five program candidates). Gate remains experimental and in progress; this checkpoint does not establish held-out quality or production adoption.

## Contract and implementation

- Asynchronous learned Gate no longer claims strict partition-equivalent signal state. Classical and explicitly synchronous offline modes retain their existing claim.
- A direct-buffer capability on the two in-place traits enables standard and nested adapters to pass one complete callback and its original context. It is independent of subdivision support. Default capability is false; existing processors retain their old route.
- Direct-capable processors prepare lifecycle/rate/layout validation without scratch sized to public callbacks. Both f32 and f64 routes avoid conversion buffers and subdivision. The original public callback size remains unrestricted by this adapter.
- Gate shares a generic private kernel between wide in-place and separate compact outputs. Programme source, immutable keys, lookahead, gain, model packets, and f32 arithmetic remain the same. f64 transport retains f32 DSP precision; unrepresentable finite-to-f32 input rejects the whole block before signal/output mutation. Wide f64 keys retain their original bits.
- All ordinary entry points share input/EOS ownership; only draining uses zero continuation directly. This preserves retained lookahead and reset requirements.

## Focused evidence

- Host tests: 714 library tests plus four mode tests and eight buffer tests passed. The new block-dependent oracle embeds callback length and checks the original MIDI offset/transport. Both adapter views receive exactly one 131,073-frame callback without allocation/free, preserve immutable input, and honor native/fallback f64 precision. Whole-block invalidity leaves output/call count unchanged.
- Five Gate direct-buffer tests passed on frozen revision r2: 72 classical configurations (three modes, 1/2/5 channels, internal/external keys, linked/unlinked, Peak/RMS, 5 ms lookahead); f32 and f32-precision f64 agreement; atomic f64 range rejection; large cold realtime callbacks/reset for RNNoise/Silero through both adapter views; preparation-mode claims; exact wide f64 keys; and an analytic impulse drain across all three native entry points and all modes.
- The first EOS regression failed on the earlier build while native entry-point bookkeeping was being consolidated. `process_input` now owns that lifecycle for all formats. The successful r2 run uses a fixed source hash manifest. Preserve the initial log as diagnostics, not accepted final evidence.
- Full frozen r2 validation passed: 127 model-enabled Gate tests (one ignored), 107 default-feature Gate tests, 726 host tests, and 26 engine offline-renderer tests. The named actual-model WAV test is included and passes. Strict host/Gate all-target Clippy and scoped engine lib/test Clippy (`--no-deps`) pass. Focused formatting passes. Source hashes were checked before every command in the frozen r2 sequence. Subsequent changes are verified doc comments only, clarifying realtime versus offline heap policy, immutable capability until initialization, and bus-width constraints; final strict Clippy passes with that clarification. See `doc-only-change.json`, the saved r2 preimages and patch, and final/r2 source manifests.

## Remaining

Automatic timeline offline preparation/restoration, frontend configuration, independent multilingual/natural-noise/music quality, quiet-plosive gain-shift robustness, stronger classical baselines/listening, and real DAW/cross-platform budgets remain open. Other ML proposals retain their existing full scope in the ledger.

Captured command logs remove only surplus blank lines at EOF for repository whitespace checks; saved source preimages remain byte-for-byte unchanged. Full objective and remaining validation requirements stay open.
