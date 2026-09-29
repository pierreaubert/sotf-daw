# Downmix finite spectral support

2026-09-28, AUD073 follow-up after AUD081 startup correction. MIDI and IAMF are
excluded. This is a focused verification checkpoint after the ninth aggregate.

## Implementation

Only the Downmix crate changed. The existing spectral sample loop is extracted
into `process_spectral`, which reads normal input or structural zeros. Its
2048-frame delay, 1024-frame hop, coefficient smoothing, FFT/phase/LtRt math,
LFE filtering and OLA clock are unchanged. No new audio buffer is allocated.

Scalar source presence/phase and optional remaining count describe EOS. For
S>0 accepted frames, continuation is `3072 + ((1024 - S % 1024) % 1024)` frames.
The independent derivation is the last occupied window origin plus existing
delay plus window length, minus emitted source duration. Each drain advances
at most one hop and writes only its returned stereo prefix. The uniform native
finite support is 4095 frames, including possible trailing zeros. Simple mode
has finite zero support when eligible.

Eligibility requires no LFE channels, Lt/Rt's unconditional LFE exclusion, or
both current and target left/right gains exactly zero for every LFE channel.
Both ordinary spectral sums and phase-energy vectors remove LFE under that
condition. A live ITU fade is insufficient until its gains settle. Previously
accumulated audio/analysis is still included in the finite window bound.

Observable recursive LFE reports Infinite and preserves legacy immediate drain
completion without freezing state. This does not render that tail automatically;
callers can select ordinary zero continuation through the explicit offline
duration API. No epsilon cutoff or finite recursive LFE claim is introduced.

## Lifecycle and work contract

Drain validates initialization, matching rate, whole stereo frames and positive
capacity when required before consuming audio. Empty input completes without
freezing. Eligible nonempty EOS rejects new input and changed/unknown controls
until reset or successful initialize; exact known snapshots are early no-ops.
Simple eligible EOS follows the same lifecycle with no output. Malformed output
and failed reinitialize leave the prior stream intact.

The new additive native `drain_call_bound` reports the exact number of successful
full-capacity calls needed from its current state, with a minimum one for an
immediate COMPLETE result. This uses actual fixed-hop progress; it does not
derive work from maximum output capacity alone. Default `begin_drain` stays a
no-op. Querying after a smaller prior destination accounts for remaining frames.
Generic host enforcement belongs to the separate AUD077 change.

## Evidence

- Five of six initial new tests fail on old production: missing retained audio,
  missing metadata, and missing drain/lifecycle validation. One reset comparison
  alone passes and is not counted as evidence of the old drain. Red log:
  `/tmp/sotf-downmix-finite-red.log`.
- **2,048** first/final-marker configurations cover every final hop phase in
  both spectral modes, with an independent exact delayed-impulse reference.
- **144** dense front-channel configurations cover four layouts, three modes,
  four rates and three ordinary callback partitions. Full output, endpoint and
  startup delay match the independent coefficient-scaled source.
- **32** nontrivial phase/LtRt cases compare the finite response with ordinary
  zero continuation, including a live surround gain ramp and LFE excitation;
  every subsequent sample beyond the bound is zero. These share the existing
  spectral kernel and verify EOS dispatch/support, not independent spectral
  algorithm accuracy.
- A separate newly settled ITU test keeps exciting LFE while its gain fades,
  then compares retained analysis/OLA against an untouched continuing twin.
- Validation, prefix canaries, invalid-rate/reinitialize retry, frozen controls,
  repeated completion, six reset/reinitialize cases and **42** exact native
  call-bound configurations pass.
- **24** cold configurations (four layouts, three modes, two rates) run first
  and final drain, scalar metadata, reset and replay on a fresh callback thread:
  **zero allocations and zero deallocations** in each measured interval.

`cargo test -p sotf-plugin-downmix --offline --all-features` passes **69 tests**
before the final newly settled ITU regression; the final finite-stream suite
passes all **9 tests**, giving **70 distinct tests verified**, no failures or
ignores. Logs: `/tmp/sotf-downmix-finite-full.log` and
`/tmp/sotf-downmix-finite-focused.log`.

Strict all-target/all-feature Clippy passes in
`/tmp/sotf-downmix-finite-clippy.log`; final test-file Clippy is recorded in
`/tmp/sotf-downmix-finite-final-test-clippy.log`. Scoped rustfmt and diff checks
pass. Production and tests are frozen for independent review and integration.

Independent implementation review found no introduced blocker in eligibility,
support, ordinary scheduling, preflight, frozen controls, lifecycle or call
counts. It identified preexisting ordinary-input validation limits, addressed
separately below. The reviewer made no source changes or additional test runs.

## AUD091 ordinary-input preflight follow-up

Ordinary nonempty processing now rejects a mismatched configured rate and any
nonfinite sample before DSP/output changes. Three simple/phase/LtRt replay
fixtures preserve caller canaries and compare later programme and drain output
bit exactly with an untouched twin. Invalid samples include NaN and both
infinities in an LFE channel, even when the configured mix discards that channel.
The initial wrong-rate constructor call failed on old production.

The full suite established explicitly supported constructor-time processing at
44.1 kHz. An initially attempted initialization guard conflicted with those two
startup fixtures and was removed; their expectations were retained unchanged.
Initialization remains required for explicit drain. This preserves the existing
ordinary setup behavior while enforcing its configured sample clock.

Final full suite: **71 passed**, no failed/ignored, with strict all-target and
all-feature Clippy and scoped rustfmt/diff checks. Logs:
`/tmp/sotf-downmix-preflight-red.log`,
`/tmp/sotf-downmix-preflight-full-final.log`,
`/tmp/sotf-downmix-preflight-final-clippy.log`.

## Oversampled capacity integration follow-up

Independent wrapper inspection found that initially advertising zero capacity
could make a minimally prepared 2x wrapper reserve 512 child frames, although
Downmix later requires a 1024-frame spectral drain destination. A real wrapper
regression reproduced the resulting capacity error. The query now advertises
the structural hop maximum before any input and after completion; simple mode
still advertises zero. Work-count eligibility is checked separately.

Both 2x and 4x wrappers prepared for one-frame ordinary callbacks now return the
retained impulse, complete within their composed bound, and measure **zero
allocations and zero deallocations** during cold begin/query/drain. This changes
capacity metadata, not DSP or tail length. Final full Downmix suite: **72 passed**,
no failures/ignores; strict all-target/all-feature Clippy passes. Logs:
`/tmp/sotf-downmix-oversampled-capacity-red.log`,
`/tmp/sotf-downmix-oversampled-capacity-full.log`,
`/tmp/sotf-downmix-oversampled-capacity-clippy.log`.
