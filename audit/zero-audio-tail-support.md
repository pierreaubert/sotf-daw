# AUD073: BandMerge and TransientShaper exact zero-audio tails

## Result

Source and dependencies establish that neither plugin retains program audio. BandMerge sums only current band samples multiplied by smoothed gains; its reconstruction meter observes those samples. TransientShaper's fast/slow envelopes and control smoothers determine a bounded gain that multiplies only the current input frame. Peak protection is another current-frame multiplier. Detector decay is not an audible response to zero input.

Both now declare `TailLength::Finite(0)` and `drain_call_bound() == Some(1)`. These scalar declarations apply before initialization and after reset as well. Existing default drain remains immediate `COMPLETE` with zero output, capacity remains zero, and no control or input freeze is introduced. No processing arithmetic, state, channel layout, parameters, or shared host code changed.

Scope: the two plugin source files, their changelogs, and new `tests/finite_stream.rs` files. The implementation plan was saved before edits at `/tmp/sotf-zero-audio-tail-plan.md`. Existing unrelated workspace/source changes were preserved.

## Independent evidence

Both public metadata regressions first failed on the old `Unknown` declaration against `Finite(0)`:

- `/tmp/sotf-zero-tail-band-red.log`: 0 passed, 1 failed.
- `/tmp/sotf-zero-tail-transient-red.log`: 0 passed, 1 failed.

The independent waveform oracle requires exact zero from the first zero-input frame onward, without calculating expected output using production DSP functions. It first warms with nonzero deterministic program, then changes controls at EOF while their smoothers are unfinished:

- BandMerge: 63 configurations, all 2–8 bands, 1/2/6 channels, 44.1/48/192 kHz. Alternate gain extremes and mute changes, plus an armed reconstruction meter.
- TransientShaper: 108 configurations, 1/2/6 channels, 44.1/48/192 kHz, all four attack/sustain extreme pairs, dry/mixed/wet. Immediately reverse shaping, sensitivity, output gain and mix controls after warmup.
- Both: zero callbacks of 1, 7, 137 and 8193 frames; reset then another exact-zero frame. Metadata, one-call completion, repeated drains, unchanged destination canaries, and accepted controls/processing after the no-op drain are pinned. The TransientShaper public parametric adapter is exercised directly.
- Five prepared instances move to fresh threads (BandMerge 2/8 bands; TransientShaper 1/2/6 channels). Explicit thread-local allocation **and deallocation** counters cover metadata, begin/no-op drain and reset for two epochs. All counts are `(0, 0)`. This test makes no new claim about general processing allocations.

## Verification

- `cargo test -p sotf-plugin-band-merge -p sotf-plugin-transient-shaper --all-features`: **120 passed, 0 failed, 0 ignored** (73 BandMerge, 47 TransientShaper; 6 new tests). Log `/tmp/sotf-zero-tail-full.log`.
- `cargo clippy -p sotf-plugin-band-merge -p sotf-plugin-transient-shaper --all-targets --all-features -- -D warnings`: clean. Log `/tmp/sotf-zero-tail-clippy.log`.
- Scoped rustfmt check and `git diff --check`: clean.

Source is frozen. MIDI and IAMF were excluded. This metadata allows native hosts to recognize immediate audio completion; envelope/control state intentionally remains intact for future input, exactly as before.
