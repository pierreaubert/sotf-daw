# AUD079 — explicit offline tail duration

`render_offline_with_tail(config, Duration, progress)` renders the decoded
program plus an explicit extra interval. `render_offline` delegates with
`Duration::ZERO`, retaining the existing default and configuration struct.
The caller chooses an endpoint even when a recursive effect has no finite
impulse-response support.

The duration is rounded up once to frames at the export rate. Source conversion
and terminal chain normalization retain their existing signal-delay compensation.
Ordinary zero-input processing continues through the source converter and plugin
chain to that endpoint. This does not depend on each plugin implementing drain,
nor alter Infinite/Unknown response metadata. Progress includes the extra frames.
Unrepresentable durations fail before creating or truncating the output file.

An independent source-padding test initially failed at export sample 480:
the candidate was zero while the reference was -0.12754709. Source-converter
finishing had stopped at program duration, discarding the FIR response across
EOF. The same test passes for 127- and 1024-frame blocks. AUD082 below further
corrects finishing when downstream filters retain additional response.

## Verification

All **19 offline-renderer tests pass**, including four new regressions:

- Exact feedback echoes: a final impulse through a 3 ms, 50% feedback delay
  produces amplitudes 0.5^(n-1) at every 144-frame echo, for block sizes
  1/127/1024. Original and zero-tail exports remain exactly 73 silent frames;
  a 30 ms tail produces exactly 1513 frames with monotonic final progress.
- A 44.1 kHz source through an actual 48→96→48 kHz chain exports exactly
  ceil(997×48000/44100)+481 frames for a 10,000,001 ns tail. The test asserts
  the configured chain really changes rate before normalization.
- Explicit 10 ms continuation matches a source WAV extended by exactly
  441 zero frames at 44.1 kHz: both 48 kHz exports contain 960 samples and
  agree within 2e-6, including the converter response crossing EOF.
- `Duration::MAX` is rejected while an existing destination remains unchanged.

Logs: `/tmp/sotf-offline-tail-source-red.log`,
`/tmp/sotf-offline-tail-duration-full.log`, and
`/tmp/sotf-offline-tail-duration-clippy.log`. Strict all-target engine Clippy
passes with default features disabled.

This API covers source-file offline rendering. Timeline export and automatic
realtime EOS retain their existing policies; the engine's separate 4096-call
drain limit remains AUD077. No silence threshold, residual-level guarantee,
native-device execution, or automatic duration choice is claimed.

## AUD082 — complete endpoint composition

Independent review found two further defects and reproduced both before fixing:

1. Source conversion stopped after its own compensated endpoint and supplied
   direct zeros to downstream FIR EQ. A 441-frame 44.1 kHz source with a final
   impulse, exported at 48 kHz through a nonflat 18 dB FIR EQ, differed from a
   much longer source padded with zeros and cropped to the requested endpoint.
   Sample 479 was 1.2170883 instead of 1.1734073 (error 0.043681026).
2. A 48→96→48 kHz chain with a 4096-frame converter chunk and one-frame render
   callbacks exhausted its continuation budget with zero of 49 frames written.
   Signal delay correctly excludes chunk waiting; a work budget must include it.

Source conversion now stays in the path until the final export frame is written.
No direct-host silence replaces its response. Continuation work is bounded by
remaining output, remaining signal trim and conservative scheduling latency,
converted once into the supplied-input clock with upward rounding and checked
block-count conversion. Scheduling latency never determines which audio samples
are trimmed. The no-source-conversion path includes host scheduling latency in
the same budget. The obsolete intermediate converter-frame cap/counter is removed.

Both regressions pass, including the FIR fixture's zero and 1 ms tail choices.
All **21 offline-renderer tests pass** on final source, and strict engine
all-target Clippy passes. Independent review found no remaining blocker in this
correction. Red log: `target/audit-offline-endpoint-composition-red.log`; final
logs: `/tmp/sotf-offline-endpoint-full-final.log` and
`/tmp/sotf-offline-endpoint-clippy.log`. This follow-up postdates the 5,579-test
eighth workspace checkpoint. The original default duration remains unchanged;
previously truncated samples inside that duration are now preserved.
