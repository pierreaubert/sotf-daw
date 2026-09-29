# AUD010 — Limiter oversampling configuration routes

2026-09-28. Configuration routes verified together with the limiter's DSP,
lifecycle and telemetry gates. This records the separate integration checks.
MIDI/IAMF are excluded.

## Engine changes

- Append `oversampling: usize` to `PluginSettings::Limiter`, with serde default0
  for legacy presets. Choice indices0/1/2 mean physical1x/2x/4x.
- Append accessor index10 after the ten existing IDs, derive new defaults from
  the shared parameter schema, and forward the value in the exhaustive converter.
- Explicit analog-core and legacy fuzzer literals select0, preserving their
  existing native processing behavior and avoiding an accidental new mode.
  The analog limiter's independent native-core test fixture also selects0.
- No constructor/parser change is required in the facade or bridge: they already
  deserialize the canonical Limiter parameters. No external preferred wrapper
  is requested; the limiter owns its new internal rate path.

## Executed engine evidence

`cargo test --offline -p sotf-engine --no-default-features --test limiter_configuration`
passes all four tests. Log: `/tmp/sotf-limiter-oversampling-engine.log`.

Two new tests verify:

- All eleven ordered IDs, preserving indices0..9. Removing the new field from
  actual serialized settings restores index0 and the original native240-frame
  default latency at48kHz.
- 27 configurations (44.1/48/96kHz ×1/2/6channels ×three choices) pass through
  generic engine accessors, saved JSON, converter, real factory and initialize.
  The integer getter and choice persist; preferred oversampling stays absent.
  Full process plus drain equals independently delayed original audio exactly
  at mix0, including both first/final samples, irregular calls and tail canaries.
  Delay is floor(Fs*0.125ms), plus512frames for the new2x/4x modes.

The two prior AUD102 tests remain unchanged and pass:54link/compatibility route
configurations plus the independent linked/unlinked quiet-channel waveform.

Strict engine lint also passes:
`cargo clippy --offline -p sotf-engine --no-default-features --all-targets -- -D warnings`.
Log: `/tmp/sotf-limiter-oversampling-engine-clippy.log`.

## External routes

The independent [bridge, C ABI and NIH report](limiter-oversampling-propagation.md)
records six passing tests and three passing strict Clippy commands. Its actual
factory, C entry points and generated native wrapper checks cover persisted
integer choices, legacy JSON, announced and physical latency, waveform equality,
failed restore history, active structural rejection and valid reactivation.
The limiter does not acquire a second host oversampling stage.

Those routes required only new tests and their module registrations. No external
restoration production changes were necessary. These portable Linux tests do
not establish commercial DAW or macOS AudioUnit runtime behavior. Final aggregate
verification passes 5,841 tests across 324 binaries, with MIDI/IAMF excluded and
10 skipped: `/tmp/sotf-audit-wave14-nextest.log`.

## Toolbar and layout follow-up

The first aggregate exposed two integration omissions: the factory rejected
choice labels used by the toolbar's existing wire-form contract, and the layout
snapshots had not yet recorded the new selector. Both are corrected.

The existing shared choice-index deserializer now accepts valid integer indices,
integral numeric wire forms and labels `1x`/`2x`/`4x` in both limiter parameter
structs. Serialized state remains an integer; live scalar writes retain their
typed structural rules. Fractional/out-of-range values and unknown labels are
rejected. No gain or sample-processing code changed.

The whole toolbar factory test passes. Ten reviewed limiter snapshots add the
index10 selector; only the 844-pixel profile also moves the unchanged TIMING
group into the existing overflow presentation. All group/control definitions
remain present. Logs: `/tmp/sotf-limiter-oversampling-toolbar.log`,
`/tmp/sotf-limiter-oversampling-layout.log` and
`/tmp/sotf-limiter-oversampling-layout-review.log`. Strict limiter Clippy passes
after this change: `/tmp/sotf-limiter-oversampling-wire-clippy.log`.
