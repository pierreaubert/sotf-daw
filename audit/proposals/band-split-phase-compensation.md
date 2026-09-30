# AUD143 proposal: explicit phase-compensated multiband BandSplit

## Decision

Add an explicit `PhaseCompensated` recombination mode for LR24 and LR48
multiband BandSplit. Preserve the existing cascaded output as
`LegacyCascade`; do not silently change the shared crossover math or the
meaning of existing saved settings. The existing constructors and settings
without a mode continue to select legacy behavior. New callers can request the
phase-compensated mode explicitly.

The phase-compensated mode should be implemented behind new, explicit
phase-compensated entry points in the LR4/LR8 multiband math API (or an
equivalent isolated plugin-owned implementation if that API cannot express the
state cleanly). Existing shared-math constructors remain byte-for-byte
behaviorally compatible for other consumers. `BandSplitPlugin` chooses which
entry point to use from its mode; no other consumer changes mode by accident.

## Recombination definition

For `N` bands there are `N-1` crossover stages. At stage `j`, let `L_j` and
`H_j` be the existing complementary LR lowpass and highpass transfer functions
and let `A_j = L_j + H_j` be that stage's allpass transfer function. Define
the outputs at unity band gain as:

```text
B_i = (product H_j for j < i) * L_i * (product A_j for i < j < N-1),
       0 <= i < N-1
B_(N-1) = product H_j for 0 <= j < N-1
```

The empty product is one. Telescoping gives
`sum(B_i) = product(A_j)`, so unity-gain reconstruction has unit magnitude
while retaining the allpass phase. Each earlier band therefore needs the
allpass state of every later crossover stage; merely delaying it or correcting
the final summed output is not equivalent. Per-band gain changes intentionally
change the sum response.

Use the existing bilinear-prewarped LR sections and coefficients for each
slope. LR24 is the square of a second-order Butterworth lowpass section.
LR48 is the square of the fourth-order Butterworth section, with its two
second-order sections using `Q = 1/(2 sin(pi/8))` and
`Q = 1/(2 sin(3pi/8))`. Validate both slopes against independent complex
responses; do not infer LR48 accuracy from the LR24 formula.

## Compatibility, structure, and real-time behavior

- Add an explicit serialized mode with a serde default of `LegacyCascade` so
  old presets and engine configurations keep their established output. Keep
  the current constructors legacy; add an explicit mode constructor or
  builder. Newly created UI instances should explicitly default to
  `PhaseCompensated` and expose the choice; deserialized settings with no mode
  and existing constructors remain `LegacyCascade`.
- Keep band-major output order exactly as it is today:
  `[band0 channels..., band1 channels..., ...]`. Keep all input channels
  independent and preserve the current output width of `channels * bands`.
- Keep band count, cutoff vector shape, and slope as structural configuration.
  Validate finite, in-range, strictly increasing cuts before replacing live
  state. Apply no allocation or structural rebuild from `process` or parameter
  callbacks. A mode or slope switch must rebuild off the audio thread, or use
  a separately designed crossfade; it must not replace filter state abruptly
  inside the process loop.
- Preserve current cutoff smoothing and the bounded coefficient update cadence
  for each cutoff. Preserve per-band gain smoothing and its current parameter
  semantics. Apply the same automation, initialization, reset, and rejection
  rules to both modes. Confirm block-partition invariance for cutoff automation
  instead of assuming the fixed-cutoff proof covers a changing filter.
- Treat all added allpass filters as persistent recursive state. Reset every
  added state in place and prove reset is allocation-free after population.
  BandSplit currently inherits `Plugin::tail_length() == TailLength::Unknown`;
  preserve that report unless measured zero-input decay supports a more
  precise contract. Do not claim a finite tail just because the sum is
  allpass.
- Measure cold and steady processing cost at 2, 3, and 4 bands and at the
  largest supported channel layout. The extra work is one allpass section per
  earlier band per later crossover, so budget it before exposing the mode in
  the application.

## Application route and mounted controls

The public DSP supports two through four bands, but the current typed engine
settings carry only one cutoff, and the chain output-width calculation doubles
the input width. The application route must be extended along the same narrow
settings path:

1. Extend typed `PluginSettings::BandSplit` with an ordered cutoff vector and
   recombination mode. Retain the legacy single `frequency` representation for
   deserialization compatibility; define one deterministic conversion for old
   settings to a one-cutoff legacy configuration.
2. Extend the settings converter and factory/builder path to pass band count,
   cutoffs, slope, and mode to `BandSplitPlugin`.
3. Update plugin-chain channel propagation to return
   `input_channels * (cutoffs.len() + 1)` for enabled BandSplit instances.
   Keep disabled/suspended behavior consistent with other channel-changing
   plugins.
4. Trace the currently mounted BandSplit control path end to end. The present
   evidence includes the one-frequency engine setting and an Audio Unit view
   controller that delegates to the generic Rust view; neither establishes a
   reachable three/four-band control in the SOTF app. Add all cutoffs and the
   mode to the actual mounted control schema and preset path, and show a user
   can change and read them there. Ensure round-trip serialization preserves
   new values and the old default; factory/model-only tests do not establish
   reachable controls.
5. Add an engine/app route test that runs 2-, 3-, and 4-band settings through
   factory creation and the actual chain, checks channel widths and band-major
   ordering, and confirms saved configuration round-trips. Add a mounted-panel
   integration check that finds the controls by their registered IDs, changes
   them, reads back the resulting settings, and confirms the preset keeps the
   mode and ordered cutoffs.

Keep this scoped to BandSplit settings, conversion, factory construction,
channel propagation, and its controls. It does not require a broad engine or
manager rewrite.

## Required verification before acceptance

Keep analytical prediction, independent-reference accuracy, expected red
reproduction, and passing smoke/capture separate in the report. A red test
must not make its containing test target look like an unexplained suite
failure; run it explicitly as a named expected-red gate and record its exit
status. Required acceptance coverage:

- Public `BandSplitPlugin` and an actual `DawHost` split → merge chain.
- LR24 and independently derived LR48 complex per-band and summed response,
  including phase as well as magnitude.
- Two, three, and four bands; close and wide ordered cutoff sets; distinct
  stereo inputs; finite, nonzero output for each channel.
- The preserved pre-edit baseline is specifically 48 kHz, stereo, and a
  coherent 1,100 Hz probe. It does not establish behavior at other rates or
  channel counts. Before implementation, declare the supported API limits;
  acceptance should cover 44.1, 48, and 96 kHz and 1, 2, 6, and 8 channels
  where the constructors accept those layouts.
- Unity-gain summed magnitude within 0.005 of 1.0 and complex responses
  within 0.002 absolute complex error from the independent fixed-cutoff model.
  Evaluate probes at 0.5x, 1x, and 2x each cutoff where valid, plus a
  logarithmic sweep through the supported band to catch narrow response
  errors.
- For identical sample-timed cutoff automation, compare multiple block
  partitions and require maximum full-scale sample residual at most
  `2e-5`; report the measured residual as well as the bound. Preserve the
  existing smoothing/cadence and compare each implementation mode separately.
- Legacy-mode vectors against the preserved pre-change captures, including
  output channel order and saved setting defaults.
- Per-band gain, cutoff automation, slope selection, invalid/rejected updates,
  initialization, repeated reset, partition invariance, and truthful tail
  behavior.
- Zero allocations in steady processing and populated reset; explicit CPU
  measurements for 2–4 bands and representative channel counts.
- Engine/factory/settings/channel-width route and preset round-trip, not only
  public DSP construction.

## Work boundary

First review the baseline and this design with Astra. Then implement the
explicit mode and math/API changes in a separate reviewed slice. Do not fold
the existing LR8 populated-reset allocation issue into a broad shared-math
behavior change: either fix the LR8 reset in place while preserving its
coefficients and state semantics, or isolate a no-allocation plugin reset
path, and verify other consumers before choosing. AUD141's Crossover fix is a
separate accepted change and is not evidence for BandSplit.
