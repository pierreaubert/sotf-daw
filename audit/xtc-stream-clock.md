# AUD080 XTC startup and accepted-input timeline — verified

Source frozen after this correction. No EOS drain, enabled/disabled policy, host, native wrapper, dependency or MIDI/IAMF edits.

## Change

Enabled XTC now consumes exactly one input frame and schedules one output frame per input-clock tick. Large callbacks are still subdivided through existing prepared stereo staging. Queued output cannot stop input consumption early. The declared latency remains N=fft_size, with exactly N startup zeros.

The analysis buffer begins with N−H=3H zeros. The first three windows start at negative programme times. Each negative synthesis hop is cleared after its last overlap contribution and is never published; this also prevents old skipped cells reappearing when the ring wraps. The read cursor begins at3H. All existing frequency-domain filter, matrix, crossfade, OLA arithmetic and resource-retirement paths remain in use. Successful initialization and reset restore the same prepared audio timeline.

Process validates initialization, sample rate, checked input/output sample products and exact buffer sizes before adopting pending filters or changing output/history. Zero-frame callbacks return without timeline/publication changes. A zero initialization rate fails before mutation. Hard-disabled routing is still immediate, and existing enable/disable semantics remain unchanged.

AutoGain still measures on its existing callback cadence; no default-mode partition-invariance claim is made. Exact neutral oracle tests disable AutoGain. Current explicit drain remains COMPLETE0, so callers still need ordinary zero continuation to reveal the suffix; AUD073 remains open.

## Red → green evidence

`/tmp/sotf-xtc-clock-red.log`: three new tests fail on lost first sample, lost mixed-callback interval and acceptance of the wrong sample rate.

`/tmp/sotf-xtc-clock-green.log`: all three pass after the scheduler fix.

Final six new test functions in `tests/stream_clock.rs` cover:

- 160 independent dense-waveform fixtures: all eight supported FFT sizes128..16384, four rates44.1/48/96/192k and five callback patterns including1, mixed17/137/512, N, 4096/137 and8193. Source extends beyond a full4N ring wrap. Oracle is independently constructed `[N zeros] + source`, with fixed absolute FFT reconstruction tolerance1.5e-6. All startup zeros are exact.
- 259 independent impulse fixtures: every hop phase for N128/256/512, then first/hop/window boundary markers for each larger N. Full stereo waveform and leading/trailing zero regions are checked, not just a largest-peak index.
- A dedicated4096→137 callback sequence checks the previously discarded4608–4643 interval and adjacent frames exactly within the same fixed FFT tolerance.
- Invalid input/output dimensions, wrong rate and usize overflow preserve canaries/history and are retryable. Reset/reinitialize and invalid initialization compare with a fresh reference timeline.
- Immediate hard-disabled routing stays exact.
- 32 cold-thread default and diagnostic-neutral cases cover all FFT sizes and44.1/192k, oversized callbacks, reset and137-frame continuation. Every measured process/reset path reports0 allocations and0 frees; outputs stay finite. Publication/retirement tests also pass in the existing suite.

Commands:

```text
cargo test -p sotf-plugin-xtc
175 passed, 0 failed, 1 preexisting ignored documentation example
/tmp/sotf-xtc-clock-full.log

cargo clippy -p sotf-plugin-xtc --all-targets -- -D warnings
Passed
/tmp/sotf-xtc-clock-clippy.log
```

Scoped rustfmt --check and git diff --check pass. No assertion tolerance was weakened. The only existing test expectation changed is reset's internal input_fill: it now correctly contains N−H synthetic zero history instead of zero samples.

Current wave files: `src/lib/xtc_plugin.rs`, one reset assertion in `src/lib/tests.rs`, new `tests/stream_clock.rs`, README and CHANGELOG. Earlier publication/format diffs in the same files remain preserved.
# Workspace fixture follow-up

The ninth aggregate run exposed two facade XTC fixtures that compared output to
input at identical indices after skipping a prefix. That expectation conflicts
with the declared FFT-size signal delay and had relied on the former
callback-dependent schedule. The corrected fixtures assert exact startup silence
and compare `output[latency..]` against the corresponding source prefix, including
source sample zero and all available delayed frames. Their original SER >70 dB
and maximum-error <1e-3 thresholds are unchanged.

Both facade suites pass all 10 tests after the correction; strict focused Clippy
passes. Logs: `/tmp/sotf-audit-wave9-xtc-fixtures.log` and
`/tmp/sotf-audit-wave9-xtc-fixtures-clippy.log`. The first aggregate's two failures
are retained in `/tmp/sotf-audit-wave9-nextest.log`; the subsequent aggregate is
recorded separately in AUDIT.md. No production XTC changes were needed here.
