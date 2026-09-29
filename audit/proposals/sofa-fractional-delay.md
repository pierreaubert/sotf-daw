# AUD-131 proposal: fractional and signed SOFA `Data.Delay`

Status: design accepted; scoped implementation and source review accepted by
Astra on 2026-09-29. Focused SOFA tests, strict Clippy and scoped formatting
pass. The workspace run completed non-green (6,064 passed, 1 failed, 14
skipped) because an active AUD-132 Upmixer candidate test failed; see
`audit/sofa-fractional-delay.md`. MIDI and IAMF remain excluded.

## Original gap and requirement

Before AUD-131, the shared loader in
`crates/sotf-plugins/crates/sotf-host/src/sofa/load.rs` accepted `Data.Delay`
in `[I,R]` or `[M,R]` form, read each value as `usize`, and materialized exact
nonnegative integer delays by prefixing zeros. It rejected finite negative and
fractional values as unsupported; it did not call those files invalid SOFA.
Missing and all-zero metadata preserved the original samples. The 256 MiB
limit applied to checked materialized data. These were deliberate AUD-109
boundaries, not unreviewed regressions.

Both spatial consumers already use that canonical loader. Binaural materializes
the source-rate response before `resample_sofa`, whose AUD-114 tests preserve
physical time across rate conversion. XTC consumes the same materialized IR and
checks selected nonzero support against its FFT size. SQLite caches remain a
separate legacy route without recoverable `Data.Delay` metadata. The current
Binaural `latency_samples` remains the FFT scheduler latency; SOFA delay is part
of the impulse response. This proposal does not change a host or engine latency
protocol.

The official [SOFA specifications](https://sofacoustics.org/mediawiki/index.php/SOFA_specifications)
define FIR `Data.Delay` as a `double` additional delay in samples, with `IR` or
`MR` dimensions. The [SimpleFreeFieldHRIR convention](https://www.sofaconventions.org/mediawiki/index.php/SimpleFreeFieldHRIR)
also ties delay units to `Data.SamplingRate`. Those pages establish units and
shapes; they do not provide a fractional interpolation algorithm or an explicit
signed-value policy. This proposal therefore treats negative-delay handling as
a SOTF causalization policy, not as a statement that negative values are
required or permitted by every SOFA convention.

## Proposed behavior

1. Keep absent, all-zero, and nonnegative integer-only inputs on the existing
   bit-exact fast path. Continue validating dimensions, finite values, checked
   sizes, and the existing 256 MiB materialization ceiling.
2. Accept exact negative integers by applying one dataset-wide integer rebase
   `max(0, -minimum_delay)` to every `[M,R]` response, then prefix each IR by
   its rebased delay. This preserves every ear/measurement difference exactly;
   the common absolute shift is explicit SOTF behavior because a real-time
   causal renderer cannot emit an advance before its input.
3. When any delay is fractional, apply a 65-tap Hann-windowed sinc candidate
   only to responses with a nonzero fractional part. Normalize coefficients in
   f64 and store them as f32. Decompose each value as `q = floor(delay)` and
   `fraction = delay - q`, and choose the dataset-wide integer offset
   `max(0, 32 - floor(minimum_delay))`. Compute prefixes from checked integer
   components: `q + offset - 32` for fractional responses and `q + offset`
   for integer responses. Do not form `delay + offset` or `32 + fraction` to
   calculate integer positions; either sum can round away a subnormal or
   near-integer fraction. Integer entries retain literal sample copies even in
   mixed datasets. Reject any conversion, expansion, or byte-count overflow
   before allocation.
4. Record the chosen common offset in the loader result in source-rate samples
   and seconds. Carry that provenance through Binaural and XTC filter
   preparation and expose the active offset through a small read-only plugin
   diagnostic/getter. A successful replacement updates it; a failed
   transactional replacement retains the prior active value. Keep
   `delay_applied` true when nonzero source metadata or rebasing was applied.
   This makes the common causal shift inspectable; it is not silently treated
   as the source file's original absolute timing.
5. Materialize before Binaural resampling, so both the delay and common offset
   retain their physical time. XTC keeps its same-rate path and validates the
   complete selected nonzero fractional tail before filter preparation. Its
   plant response must be checked against the analytic delayed SOFA response;
   its inverse-filter output must be checked as a plant/filter cascade, not
   assumed to be a copy delayed by the same amount. Failed Binaural/XTC
   preparation remains transactional. Cached SQLite data, interpolation policy
   between SOFA measurements, plugin scheduler latency, and realtime processing
   remain unchanged.

For delay `d = q + f`, where `q = floor(d)` and `0 <= f < 1`, the candidate FIR
uses taps proportional to `sinc((k - 32) - f) * Hann(k)`, `k=0..64`,
normalized to unity DC gain. The implementation evaluates the tap-relative
offset before applying `pi`, and uses the analytic sinc limit near zero. Exact
`f=0` bypasses coefficient evaluation and copies samples through a literal unit
integer shift; it must not rely on floating `sin(pi*n)` residuals. Fractional
responses use prefix `q + offset - 32` after rebasing, so their intended
passband delay is `d + offset`. The common offset is the only expected
absolute-time change. A fractional response needs 64 additional stored samples
per IR before common padding and the existing storage check.

## Numerical screen and limitations

An offline f64 response calculation, with the normalized 65-tap Hann-sinc
coefficients rounded to f32 before evaluation, swept fractional parts 0.01–0.99
and 4,500 frequency points from DC to 0.45 cycles/sample. The largest observed
magnitude error against an ideal fractional phase was 0.0261 dB; the largest
phase-equivalent delay error was 0.000579 samples. The latter is the absolute
unwrapped phase residual divided by `2*pi*frequency`, a phase-delay error
metric; it is not derivative group delay. This is a design-screen calculation,
not a Rust implementation test or a product measurement. It
supports candidate gates of 0.03 dB and 0.001 sample through 0.45
cycles/sample. The proposal makes no accuracy promise closer to Nyquist: a
real finite fractional-delay response necessarily has a difficult Nyquist
boundary, and this candidate's response error rises above the screened band.

## Required independent evidence

- Public loader tests for shared `[I,R]` and per-measurement `[M,R]` delays,
  both ears, mixed integer/fractional values, and signed values such as
  `[-1.25, 2.5]`; verify the returned rebase, effective delay, and preserved
  relative delay. Include fractions near zero and one, smallest positive and
  negative subnormals, values adjacent to positive and negative integer
  boundaries, negative integer values, zero, negative zero, malformed
  dimensions, and nonfinite values.
- Conversion/support tests for huge positive values and huge common negative
  offsets with small relative spread. Verify overflow/256 MiB rejection before
  any large allocation, and exact output sizing on both sides of the support
  boundary.
- Bit-exact existing controls for missing, all-zero, nonnegative integer and
  manually zero-prefixed responses. Verify integer-only loader output is
  unchanged from the pre-edit sample digest.
- An independently coded frequency-domain oracle for impulses at multiple
  source positions and fractional parts. Check the materialized response
  against analytic phase `exp(-j*omega*(d + offset))`, with unwrapped phase and
  magnitude checked independently. Check DC gain separately; gate phase-delay residual only at nonzero
  frequencies as
  `abs(unwrapped_measured_phase - unwrapped_ideal_phase)/(2*pi*f)` at 0.03 dB /
  0.001 sample through 0.45 cycles/sample. Keep the oracle independent of
  production window/coefficient-generation code.
- Binaural public output and full EOS tail checks, including 44.1→48 kHz and
  48→96 kHz physical-time controls. Compare ordinary and irregular callback
  partitions, reset, and failed/replaced filter preparation. Verify the
  additional 64-sample support is included in Binaural's FFT-size rejection.
- XTC plant response and inverse-filter cascade checks using the shared loader,
  with selected support immediately inside and outside the FFT boundary,
  unselected long measurements, and failed preparation preserving both live
  filters and the reported offset. Do not expect the inverse output alone to
  be delayed by the plant's common SOFA offset.
- Confirm existing callback allocation guards remain zero: coefficient design
  and convolution occur during file loading, not audio processing. Report
  loader preparation cost separately if the measured setup cost is material;
  do not describe it as callback overhead.
- Focused host, Binaural, XTC tests and strict Clippy on one frozen DAW source
  and lock snapshot; run the required broad workspace gate once after the
  changed source is stable. Preserve MIDI/IAMF test exclusions.

## Accepted design

The bounded design above is approved for implementation. The dataset-wide
causal rebase is explicit SOTF policy: it preserves relative spatial timing and
reports the common absolute shift through `LoadedSofa` and consumer diagnostics.
The implementation must preserve the integer-only fast path bit-for-bit,
perform convolution during control-side loading, and leave renderer latency
protocols unchanged. Independent validation will check DC gain separately and
measure unwrapped phase-delay error only at nonzero frequency.

## Baseline artifacts

Before this proposal, the spatial source snapshot was copied to
`/tmp/sotf-aud131-preedit/`; its source and current `Cargo.lock` checksums are
in `/tmp/sotf-aud131-preedit.sha256`. The accepted integer-delay and
cross-rate controls are retained in `audit/sofa-delay.md` and
`/tmp/sotf-sofa-delay-cross-rate.log`. Before any production edit, an ignored
focused test captured the canonical integer loader output and full process plus
drain arrays for three Binaural input routes and both XTC input routes. It
passed 1/1; the loader array contains 138 f32 samples and each consumer array
contains 640 f32 samples. Exact command result is
`/tmp/sotf-aud131-preedit-capture.log`; array SHA-256 values are in
`/tmp/sotf-aud131-preedit-arrays.sha256`. The pre-edit fixture exporter is
temporarily present in `crates/sotf-plugins/tests/sofa_delay.rs` so the same
integer controls can be recaptured after implementation; remove that exporter
after comparison. Implementation and scoped validation results are recorded
in `audit/sofa-fractional-delay.md`.
