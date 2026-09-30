# AUD141: LR24 multiway recombination correction proposal

Status: **bounded design accepted by Astra; implementation in progress**. This
proposal covers the reproduced multiway LR24 IIR topology mismatch only.

## Confirmed behavior

For LR24 split frequencies 1,000/1,200 Hz at 48 kHz, the new public-plugin
regression feeds a coherent 1,100 Hz tone and independently projects each
band over 9,600 settled frames. The current public plugin returns summed
complex transfer `0.83207302−j0.01922535` (magnitude `0.83229510`) rather than
the all-pass product. The measured band 0 transfer differs from the independent
reference by `0.16770490`. The expected-red run and provenance are recorded in
`audit/crossover-multiway-recombination.md` and
`/tmp/sotf-aud141-public-red.log`.

## Proposed correction

Limit production changes to the multiway LR24 processing branch in
`crates/sotf-plugins/crates/sotf-plugin-crossover/src/crossover_plugin.rs`.
For `m` ordered split points, let `L_i` and `H_i` be the low/high complex
responses for split `i`, and `A_i = L_i + H_i` its all-pass sum. Emit
`m + 1` bands as:

```text
B_b = (product of H_i for i < b) * L_b * (product of A_i for i > b),  0 <= b < m
B_m = product of H_i for 0 <= i < m
```

This telescopes to `sum(B_b) = product(A_i)`. For two split points it is
`[L0*A1, H0*L1, H0*H1]`; for three it is
`[L0*A1*A2, H0*L1*A2, H0*H1*L2, H0*H1*H2]`.

The current bank already calculates both `low_buf` and `high_buf` from the
same per-band/per-split crossover instance. Route highpass for splits before
the output band's index, lowpass at that index, and `low_buf + high_buf`
through later splits. The final band remains highpass through every split.
This adds only the sum for later all-pass sections; it does not allocate new
filters or add filter state. Keep output channel order band-major and preserve
the 16-sample frequency-coefficient update cadence and its persistent phase.

Do not change the two-way LR24, per-channel LR24, FIR, parameter identity,
structural update/rejection, or recursive-tail behavior. Low/both/high
selection continues to return its named endpoint or all band channels. The
multiway low endpoint intentionally changes from `L0*L1*...` to
`L0*A1*...`: both its magnitude and phase change, removing the unintended
later-lowpass attenuation. The last high band is unchanged. Preservation
baselines guard the separate two-way LR24, per-channel LR24 and FIR paths.

The product and telescoping equations describe stationary, fixed-cutoff LTI
responses. During frequency automation, serial time-varying sections cannot
generally be commuted. Keep the existing 16-sample coefficient cadence and
persistent smoother phase; test bounded waveform, callback-partition and
reset behavior during automation without claiming exact all-pass transfer at
each automated instant.

## Baselines before production edits

The overlapping-split red result above must be kept as defect evidence. Before
editing production, capture ordinary nonzero output arrays from the existing
two-way LR24, per-channel LR24, and FIR modes using deterministic stereo input
and callback partitions. Include meaningful low/high/both selections where
they are supported. Save the actual vectors and source/lock manifest, then
replay them byte-for-byte after the correction. The test helper must remain
isolated from the production path.

## Independent acceptance evidence

1. Extend `tests/aud141_recombination.rs` to three and four output bands. Use
   the independent f64 analog LR4 response and prewarped bilinear reference
   already used by the reproduced test. Measure complex phase and magnitude
   for every branch and their sum; check close and wide splits and probe points
   near each split. Keep the sine windows coherent and assert the projected
   tone dominates residual energy.
2. Require absolute complex transfer error at most `2e-3` for each band and
   the summed transfer, with expected summed magnitude within `2e-3` of unity.
   Assert these as bounds; report observed maximum errors separately.
3. Cover 44.1, 48, and 96 kHz, distinct left/right tones, three/four bands,
   and `low`, `high`, and `both` selections. Exercise reset, reinitialization,
   a rejected invalid frequency update without audio change, and the existing
   callback partitions around the persistent 16-sample automation cadence.
4. Add an actual stereo `DawHost` chain regression:
   `CrossoverPlugin::new_multiway(2, "LR24", ..., "both", two_splits)` emits
   six band-major channels into `BandMergePlugin::new(2, 3)`, which returns
   stereo. Feed distinct coherent tones on left and right, verify the 2→6→2
   edges and each output channel's complex response against the independent
   f64 all-pass product, and check cross-channel leakage. Do not use
   BandMerge's self-relative `reconstruction_error_db` as an oracle for the
   original input; it compares with the sum of the bands it receives.
5. Run the crossover package tests, the direct and host-chain regressions,
   strict package Clippy and saved ordinary baseline replay. Exercise cold
   post-initialize processing, repeated callbacks, reset and post-reset
   processing under allocation/deallocation guards. Keep MIDI/IAMF exclusions
   intact. Any CPU timing change is to be measured separately after
   correctness, not inferred from adding a small band-local sum.

## Limits

The source-confirmed correction is for the multiway LR24 IIR all-pass
decomposition. It does not establish arbitrary crossover spacing quality,
all plugin graph support, or a finite recursive tail. It does not cover the
separate `BandSplit` cascade; that plugin's unequal group delay and response
need a separate design. The FIR path and separate AUD073 recursive-tail
question remain out of scope.
