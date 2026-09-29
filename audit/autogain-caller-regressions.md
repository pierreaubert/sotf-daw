# AUD105 — independent block-caller regression evidence

2026-09-28. Source frozen. Scope: new public integration tests in EQ, Crossfeed,
and XTC; one authorized EQ development dependency for JSON construction. Shared
AutoGain production belongs to plugin_chain. AUD108's separate EQ reset fix is
reported in `audit/eq-autogain-reset.md`.

## Tests and conventions

New files, relative to `crates/sotf-plugins/crates`:

- `sotf-plugin-eq/tests/auto_gain_smoothing.rs`: eight configurations (48/96 kHz,
  137/512-frame callbacks, +9/-9 dB peaking filter). JSON supplies enabled
  AutoGain and 25/1000 ms smoothing. Every compiled call explicitly requests
  `PluginCompiledOp::EqBiquadBank` and asserts `Some`; full ordinary/compiled
  waveforms match exactly. A 1 kHz tone at the filter center independently fixes
  the needed correction's direction. Shorter smoothing must attenuate a boost
  faster and compensate a cut faster. Disabled AutoGain produces identical
  waveforms for both smoothing values, including the compiled route.
- `sotf-plugin-crossfeed/tests/auto_gain_smoothing.rs`: eight configurations
  (48/96 kHz, 137/512 callbacks, -40/-12 LUFS targets). Nonneutral Bauer mode,
  fixed full-wet mix, low-level unequal two-tone stereo input. Public realtime
  smoothing setter/readback exercises 25/1000 ms. Integrated f64 audio energy
  establishes faster attenuation or gain as required by the absolute target.
  Reset reproduces the complete waveform exactly; disabled negative controls
  are exact and the raw crossfeed is explicitly proven nonneutral.
- `sotf-plugin-xtc/tests/auto_gain_smoothing.rs`: four configurations (48/96 kHz,
  137/512 callbacks), N=1024, quiet antipolarity 1 kHz tone, filter normalization
  disabled. The public smoothing setter/readback exercises 25/500 ms. Independent
  f64 energy of the uncompensated stationary tone determines the needed gain
  direction; a single frequency has the same loudness weighting before/after
  the filter. Both raw and corrected energy checks must show actual adaptation.
  Quiet raw output remains below 0.1, leaving the output limiter inactive.
  Reset and disabled smoothing negative controls match exactly. A preliminary
  common-mode tone was too nearly neutral for the fixture prerequisite and was
  removed; no numerical tolerance was relaxed.

All paired renders use identical callback schedules. EQ still measures only
one callback in ten, and Crossfeed still measures per callback; these tests do
not claim their target clocks are callback invariant. XTC's existing sample
clock and canonical EOF tests remain separate and were rerun.

## Red to green

`/tmp/sotf-autogain-callers-red-final.log`: all three tests failed against the
original shared helper because their short/long outputs were exactly identical:

| Caller, first 48 kHz/137-frame case | Short energy | Long energy |
|---|---:|---:|
| Crossfeed, target -40 LUFS | 11.857715418360716 | 11.857715418360716 |
| EQ, -9 dB filter | 57.31619216692535 | 57.31619216692535 |
| XTC, antipolarity tone | 0.1896085208225341 | 0.1896085208225341 |

`/tmp/sotf-autogain-callers-green-final.log`: all three new tests pass after the
shared scalar-equivalent correction (2.17 seconds combined test execution).
The final formatted files also ran in the subsequent full suite.

## Full verification and remaining numerical conflict

Command:

`cargo test --offline -p sotf-plugin-eq -p sotf-plugin-crossfeed -p sotf-plugin-xtc --no-fail-fast`

Log `/tmp/sotf-autogain-callers-full.log`:

- Crossfeed: 85 passed.
- EQ: 135 passed, including the separate AUD108 reset regression.
- XTC: 211 passed, one failed; one existing ignored documentation example.
- Aggregate: 431 passed, one failed, one ignored. Existing XTC callback, causal
  measurement, finite EOF, warm bypass, and allocation/free gates passed.

The remaining failure is the preexisting XTC independent diagonal amplitude
oracle in `src/lib/autogain_tests.rs:86`: at 44.1 kHz/N=128, it reports
6.009591 dB versus the ideal 6.020599913 dB, exceeding its unchanged 0.01 dB
limit. This is the legacy scalar's floating-point fixed point now exposed in a
former block caller, not insufficient settling time or a new callback clock
error. No original tolerance, fixture, or production arithmetic was changed to
hide it.

An explicit f32 recurrence diagnostic, independently written in Python without
calling the production helper, separates the components:

| Rate | Target dB | Stalled dB stage | Converted dB | Final gain dB | Ideal deficit dB |
|---|---:|---:|---:|---:|---:|
| 44100 | 6.020599842 | 6.019548416 | 6.013018229 | 6.009588293 | 0.011011620 |
| 48000 | 6.020599842 | 6.019455910 | 6.012926512 | 6.009193282 | 0.011406632 |
| 96000 | 6.020599842 | 6.018310547 | 6.011786967 | 6.004319995 | 0.016279919 |

Artifacts `/tmp/sotf-autogain-stall-diagnostic.py` and `.log`. The diagnostic is
an explanation of the preserved approximate arithmetic, not the expected-gain
oracle used by the new audio tests. Plugin_chain separately confirmed exact
pre-change/post-change scalar plateau identity at both 4 and 30 seconds in
`target/audit-autogain-legacy-plateau.log`. Root has the compatibility/accuracy
conflict for an explicit decision; this report does not claim a green full gate.

Strict all-target Clippy passed for all three crates:

`cargo clippy --offline -p sotf-plugin-eq -p sotf-plugin-crossfeed -p sotf-plugin-xtc --all-targets -- -D warnings`

Log `/tmp/sotf-autogain-callers-clippy.log`. Rustfmt checks and scoped diff checks
passed. GPUI/MIDI features were not enabled. No host protocol, meter cadence,
parameter IDs, or DSP production changed in these caller tests.

## Dependency note

Root authorized `serde_json = { workspace = true }` under EQ dev-dependencies
for real constructor JSON coverage. Cargo added the corresponding existing
`serde_json` dependency-name edge in EQ's lockfile package record; no version
or package additions arose from this test dependency. All unrelated manifest,
lockfile, and source work was preserved.

## Subsequent scope decision

The root retained the existing XTC 0.01 dB requirement. AUD110 separately
investigates local AutoGain precision and accurate dB conversion. That follow-up
will intentionally change enabled scalar waveforms if implemented; this report
records the intermediate AUD105 compatibility checkpoint, not the final gate.
