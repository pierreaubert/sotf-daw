# AUD-095: MultibandExpander soft-knee state boundary

Status: reviewed and approved after the eleventh aggregate, then implemented in the scoped MBE crate. Both permanent full-curve tests reproduced the defect before the change; all six new tests and the full 152-test crate suite now pass, with strict Clippy and release QA clean. Verified report: `/tmp/sotf-mbe-soft-knee-verified.md`; exact production delta: `/tmp/sotf-mbe-soft-knee-production.patch`. The initial isolated investigation below made no repository DSP/test changes or Cargo builds. MIDI/IAMF excluded.

## Confirmed spectral defect

`MultibandExpanderPlugin::calculate_expansion_attenuation` defines a centered soft knee with width `K`: attenuation is zero at `T+K/2`, equals `(R−1)K/8` at its center `T`, and joins the below-threshold expansion slope at `T−K/2`. This law is continuous with a continuous first derivative at both edges.

The `process_spectral_hop` gate state machine instead treats **the knee center `T` as the unity opening boundary**. `Open` never evaluates the attenuation law. `Hold` returns to `Open` when the level reaches `T`; `Closing` also returns immediately to `Open`/target zero at `T`. The upper half of the knee is therefore bypassed regardless of the nonzero gain reduction defined by the existing law. Positive hold/hysteresis deliberately introduce state dependence, but the reproduced discontinuity occurs with **both zero**, after long settling from either history.

Relevant source: `crates/sotf-plugins/crates/sotf-plugin-multiband-expander/src/lib/multiband_expander_plugin.rs`, `calculate_expansion_attenuation` and `process_spectral_hop` (approximately lines 674–850).

### Public probe and independent oracle

Artifacts:

- `/tmp/sotf-mbe-soft-knee-probe.rs`
- `/tmp/sotf-mbe-soft-knee-red.log`
- `/tmp/sotf-mbe-soft-knee-build.txt` (exact existing rlib paths/compiler command)

The probe links existing plugin/host artifacts with `rustc`; sources and executable live only in `/tmp`. It calls the public native plugin API. Parameters: 48 kHz, one spectral band, FFT size 1024, threshold −24 dB, ratio 4, knee 12 dB, range 80 dB, attack 1 ms, release 10 ms, hold/hysteresis zero, fully wet, no auto makeup, detector HPF disabled. The input is a coherent interior-bin cosine at bin 37. Eight windows establish an open (−6 dBFS) or closed (−50 dBFS) history, followed by a constant target level through 64 windows; the last 16 complete windows are measured using an independent f64 cosine projection.

For periodic Hann analysis a coherent cosine of amplitude `A` occupies three positive-frequency bins. With the plugin's amplitude normalization, the center bin has magnitude `A` and its two neighbors have magnitude `A/2`. Hann synthesis and four-overlap normalization give the settled fundamental gain

`G(A) = (2 g(A) + g(A/2)) / 3`.

This follows by multiplying the three cosine components by the synthesis Hann and summing four phases; sideband terms cancel. `g` is derived independently in f64 by integrating the linear attenuation-slope transition from `−(R−1)` at the lower knee edge to zero at the upper edge, then converting dB with standard `powf`. The oracle does not call the production attenuation function, FFT, fast log or fast power.

Final measurements with the valid 10 ms release:

| Input dBFS | Actual gain dB | Independent centered-knee gain dB | Error dB |
| --- | ---: | ---: | ---: |
| −31 | −24.009066 | −23.995246 | −0.013820 |
| −30 (lower edge) | −20.999146 | −20.995246 | −0.003899 |
| −27 | −13.038151 | −13.049879 | +0.011728 |
| −24.01 | −7.173753 | −7.171602 | −0.002151 |
| −24 (center) | −7.157529 | −7.155167 | −0.002362 |
| −23.99 | −2.994599 | −7.138753 | **+4.144154** |
| −21 | −2.269307 | −3.235271 | **+0.965965** |
| −18 (upper edge) | −1.265506 | −1.264555 | −0.000950 |
| −17 | −0.000903 | −0.928799 | **+0.927896** |

The gain jump from −24.01 to −23.99 dBFS is **4.179154 dB for a 0.02 dB input change**. The independent curve changes only 0.032849 dB. The additional discrepancy above the central bin's upper knee comes from its neighboring Hann bins crossing the same incorrect center boundary. Exact equality at `T` can fall to either comparison side under floating-point FFT/log error; bracketing levels avoid relying on that accident.

Every measured waveform was bit-identical between callbacks `[1]` and `[137,1,511]`; settled gains also agreed within 0.001 dB from initially open and initially closed histories. The executable fails its final assertion that the bracketed settled gain difference is below 0.1 dB, preserving a deterministic red reproduction.

## Existing hold/hysteresis semantics

Current spectral state transitions:

1. `Open`: below `T`, enter `Hold`, load the rounded hold count in hops; target remains zero on this transition.
2. `Hold`: at/above `T`, return to `Open`; otherwise decrement a positive hold count. Once exhausted, enter `Closing` only below `T−hysteresis`; target then uses the centered attenuation law.
3. `Closing`: at/above `T`, return to `Open` with target zero; otherwise continue the centered attenuation law.

The proposed fix preserves this state machine and its timing/count ordering. It changes the opening boundary to where the existing gain law actually reaches unity, with the closing boundary the same documented hysteresis distance below it. It does not remove intentional hysteresis/history dependence or change attack/release smoothing.

## Time-domain interaction

The same file's `process_stream` time-domain loop (approximately lines 1640–1775) contains the same three branches and comparisons against `th`/`th−hys`, calling the same centered attenuation function only while closing. Both linked and independent detectors feed this common state logic. The threshold may come from a per-band override or the existing per-sample smoother.

Thus the source defect is shared: fixing only spectral mode would leave the single-band Expander and time-domain MultibandExpander with the same inaccessible upper knee. The spectral reviewer independently confirmed this with a public one-band DC probe: zero HPF removes audio/detector filtering, peak detection of steady DC gives a direct level oracle, and expected settled gain is simply `g(A)` without Hann weighting.

Artifacts `/tmp/sotf-mbe-time-knee-probe.rs` and `/tmp/sotf-mbe-time-knee-probe.log` cover **432 cases**: 44.1/48/96 kHz, 1/2 channels, linked/unlinked, knee 0/12, open/closed prefixes and nine levels. With the same `T=−24,R=4,K=12` fixture, actual gain at levels −24.01→−23.99 dBFS is **−4.516418→0 dB** from open history and **−4.516545→0 dB** from closed history. The independent law predicts **−4.515012→−4.485012 dB**, a continuous change of only 0.030001 dB. At −21 dBFS actual gain is zero instead of −1.125 dB. Reference-path maximum error is 0.015118824 dB, and the largest settled open/closed difference across this matrix is 0.0065353 dB. The proposed 0.03 dB time-domain tolerance covers the existing approximation error while remaining far below the confirmed 4.485 dB defect. This independent probe also used existing rlibs only, with no Cargo or repository changes.

## Proposed bounded correction

Scope: **MultibandExpander crate only**, with permanent focused regressions and its documentation. No host, native wrapper, wiring, startup/drain, crossover, detector or oversampling changes.

- Derive the opening boundary from the actual threshold and effective per-band knee: `opening = threshold` for `knee < 0.1`, otherwise `threshold + knee/2`.
- Use that boundary in all three state decisions, and `opening−hysteresis` for the closing trigger. Keep passing the original knee **center** to `calculate_expansion_attenuation`.
- Apply the same small boundary derivation to spectral and time-domain state machines. Compute it at the existing hop/sample control clock respectively, honoring band overrides and current threshold smoothing. A small shared scalar helper is acceptable to prevent drift; no new state or allocation is needed.
- Preserve the existing hold counter ordering, rounded sample/hop durations, envelope coefficients, range cap, gain computation and bin normalization. Hard-knee behavior remains byte-for-byte equivalent because its boundary stays at the original threshold.

This matches the local Gate AUD-065 boundary correction (`GatePlugin::advance_threshold`) without importing its separate detector, hold or attack/release policies.

## Required permanent acceptance evidence

1. Public coherent spectral tone oracle at lower edge, center and upper edge, with close brackets around the old threshold and the adjacent Hann-bin threshold. Require strict numerical agreement with the independent f64 law (proposed 0.03 dB, covering measured existing fast-math error below 0.014 dB) and a separate continuity limit; never widen tolerance to absorb the multi-dB defect.
2. Public one-band time-domain DC oracle with HPF off and no makeup. Cover both detector-link choices; RMS requires independent settled-level interpretation, not a peak-amplitude assumption.
3. Initially open/closed histories and multiple callback partitions. A nonzero hysteresis case must retain the intended distinct histories between the new close/open boundaries. Positive hold must retain its sample/hop duration and eventual transfer target; threshold/knee changes must update boundaries on the established control clock.
4. Negative controls: knee zero (unchanged hard-knee law and transition sequence), ratio one (unity audio), range zero, dry mix, bypass/inactive/solo bands. Per-band overrides must be evaluated independently of globals.
5. Existing startup/drain/reset and allocation tests remain green. Add a targeted cold control/processing check only if the chosen code change adds new hot-path machinery; the proposed scalar derivation needs no extra storage.

## Compatibility

This deliberately changes output for existing nonzero-knee presets: the upper half of the configured knee will finally attenuate, and hold/hysteresis boundaries move from its center to its unity edge. In the demonstrated preset the old upper-half error exceeds 4 dB. Preserve parameter IDs, units, defaults, serialized values and hard-knee results; document the audible bug fix rather than silently remapping presets or adding an undocumented legacy curve. No external feature-parity claim is needed: the plugin's existing centered gain function supplies the intended mathematical contract.
