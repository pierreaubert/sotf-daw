# Independent limiter oversampling telemetry review

## Review outcome

No introduced arithmetic, lifetime, or bounded-storage blocker found. Inspected
the actual source, accepted telemetry proposal, host converter scheduling, and
permanent tests. The initial review was read-only; the parent then authorized
only the new public `tests/oversampling_telemetry.rs` to close concrete evidence
gaps. No production source was changed by this reviewer.

### Audio/gain association

- `native_kernel.rs` observes the actual delayed sample and clamped first-stage
  wet sample before the public mix. The high-rate core uses wet=1 and observes
  emitted frames after its lookahead, so its descriptor must not add core
  lookahead a second time. `WetCore::render` follows that rule.
- Actual applied gain uses an f64 ratio of finite, nonzero pre-gain samples,
  capped at unity. Zero audio receives a unity label. It does not infer limiter
  gain from filter attenuation or differently phased dry/output amplitudes.
- The core descriptor number is the emitted high-rate cursor divided by 256F.
  Native output block b receives min(a[b−1],a[b]) after the explicit 256 startup
  frames. The independent backend impulse test injects after the upsampler at
  every high-rate phase and proves output is zero beyond these two down units.
- Final-guard first-stage gains use the actual ISP audio ring position. They
  are delayed by J=3D before multiplication by the final ISP gain. Core labels
  travel through G=D+J. Each channel is combined before the channel minimum,
  avoiding artificial cross-channel sums. Private known-gain tests distinguish
  same-channel 18.0618 dB from separate-channel 12.0412 dB.
- The effective outer mix is the value just advanced for that output frame.
  Its indicator is (1−m)+m*g, independent of phase cancellation. The actual
  synthetic opposing-signal test produces zero audio but still zero reduction.

### Descriptor capacity and EOF

- Ordinary submission is split at the remaining 256-frame accepted phase;
  it can generate one unit before delivery. `begin_drain_with` can generate at
  most the residual unit and one up-filter overlap unit before delivery.
- Later child drain output is requested only after queued native output has
  emptied. The child returns at most 256 high-rate frames, not a whole native
  maximum inferred as guaranteed progress. Partial high-rate minima accumulate
  in the same descriptor until that down unit is complete.
- Missing final-unit frames receive implicit unity labels, and the final pure
  down-filter overlap obtains a unity current descriptor plus the preceding
  real descriptor. No stale descriptor is silently reused: tags are checked.
- Eight fixed descriptors cover the documented live frontier. The permanent
  all-256-phase source test enforces a five-entry bound with 0, fractional,
  and maximum lookahead; the observed high-water mark is three entries.
  Tiny output capacities cannot generate additional
  units while old native output remains queued; public partial-drain and cold
  tests exercise this scheduling invariant separately.
- Once wet conversion finishes, incoming core labels are unity while actual
  guard audio/gain delays flush. `finish` runs only for emitted frames, so
  zero-output progress, queries, and repeated begin do not advance output
  telemetry time. Reset primes gain rings/descriptors with unity and resets the
  publication accumulator. The last public cache value remains visible until
  the next scheduled publication, matching existing native behavior.

### Conservative claims

The README and `LimiterData` correctly describe a gain-control indication,
not a scalar gain ratio for the final FIR-combined waveform. One core reduction
can label two 256-frame output units; the 100-ms display interval adds its
ordinary aggregation. Positive and negative FIR coefficients can cancel, so
no claim of exact output attenuation or exact audible duration is justified.
Per-channel composition and affine mix are consistent with this convention.

## Evidence gaps found and closed

The original public neutral test inspected only its final pre-EOS snapshot;
it did not independently measure filter attenuation or inspect reset, live mix,
and EOF publications. Private positive gain tests did not establish that the
public cache actually published a positive result. Existing 512 cold fixtures
also ended before the first 100-ms publication. The implementation owner took
the cold-publication extension; this reviewer added three public tests only.

### New public tests

1. **512 residual-phase configurations, two reset epochs each:** 2x/4x and every
   phase0..255, with sample/ISP modes, three low-level frequencies, static mix
   settings and live mix ramps where allowed. An independent output-frame clock
   inspects every actual 100-ms publication, including a publication reached
   only during partial finite drain. All 2048 observed publications remain
   exactly 0 dB/inactive. Independent −60 dB input-peak checks reject an unchanged
   initial cache as a false pass.
2. **12 coherent filter measurements:** 48/96 kHz × 2x/4x × 1 kHz/0.45R/0.49R.
   A separate f64 single-frequency projection measures the steady output/input
   fundamental. At 0.49R attenuation is **106.381037..106.381274 dB**, while every
   meter update remains exactly zero. Lower-frequency attenuation is near
   0.000005..0.000131 dB. This explicitly rejects energy-ratio telemetry.
3. **96 settled overload configurations:** four rates × mono/stereo/6 channels
   × 2x/4x × dry/half/full wet sample mode or full wet ISP. Constant amplitude4,
   ceiling−6 dBFS, and 10-ms release give an independent steady gain expectation.
   Every later publication, including the final plateau interval reached during
   drain, matches within the unchanged **0.01 dB** bound. Overload→reset→quiet
   then publishes exact zero/inactive after fresh cadence, proving gain labels
   and accumulation do not leak between epochs.

### Independent ISP calibration correction

The initial trial compared ISP DC against ideal T/A and exposed a 0.014395571 dB
delta at 44.1 kHz/4x/ISP: published18.055595 versus ideal18.041199827 dB. This is
not evidence of a meter fault: the finite Hann-sinc phase sums are not normalized
to unity. Independently summing that mathematical kernel gives maximum DC phase
gain1.00090448394 at4x reconstruction (0.00785269691 dB) and1.00011346215 at2x
(0.00098546378 dB).

The corrected stationary oracle is
`g = 10^(−6/20) / [4*max(M_native, M_high_rate)]`, followed by the independent
affine dry/wet gain. Serial stages enforce the same ceiling, so calibration
uses the maximum rather than multiplying both phase maxima. No production
helper or fast-math implementation is used by this reference. The remaining
worst measured error is **0.006903506 dB**, consistent with existing scalar
approximations. The initial 0.01-dB assertion was **not loosened**.

## Verification

```text
cargo test -p sotf-plugin-limiter --test oversampling_telemetry -- --nocapture
cargo clippy -p sotf-plugin-limiter --test oversampling_telemetry -- -D warnings
```

The initial log preserves the missing-calibration trial; the corrected public
target passed **3 tests, 0 failures, 0 ignored** in 9.50 seconds. Strict target
Clippy, rustfmt, and scoped diff checks passed. Logs are in:

- `/tmp/sotf-limiter-public-telemetry-first.log`
- `/tmp/sotf-limiter-public-telemetry-dc-corrected.log`
- `/tmp/sotf-limiter-public-telemetry-green.log`
- `/tmp/sotf-limiter-public-telemetry-clippy.log`

This review does not replace the owner's full-crate, release QA, or cold
publication gates. Public cache queries in these numerical tests occur outside
any claimed realtime allocation measurement.
