# AUD114: HRIR resampling without backend timing shifts

## Defect and correction

The prior helper exported output starting at the backend's frame zero and stopped feeding it as soon as the source samples had been submitted. It retained the sinc filter's delay, omitted delayed late response, and could return entirely silent short 44.1/48 kHz conversions. The public 36-case impulse probe is `/tmp/sotf-sofa-resample-clock-probe.rs`; original output is `/tmp/sotf-sofa-resample-clock-probe.log`. The permanent red test log is `/tmp/sotf-sofa-resampling-red.log`: two failures, one pass. One timing failure placed an impulse at output frame 31 instead of 0; the independent 100 Hz transfer check had relative error 0.415823186.

`src/hrtf/resample.rs` now uses Rubato 1.0.1 fixed input/output FFT quanta with both grids even. The old desired reduced-grid multiplier is rounded up to the next even value: 7 becomes 8. Already-even grids keep their geometry; formerly odd grids deliberately change sinc length and cutoff to make the input sinc center and reported integer output delay describe exactly the same physical time.

Each measurement resets the prepared backend. Input is zero-padded and continued until the complete requested output window exists. The exact backend delay is discarded and `ceil(N * target/source)` frames are retained. No threshold-based tail estimate or silent missing-output padding is used. Scratch is reused across measurements and only successfully completed IRs/metadata are committed.

This preserves the existing sample-amplitude convention, with no added gain normalization. The exported finite window can omit sinc response before time zero or after the retained duration; it is not an infinite-tail claim. A genuine propagation delay already materialized by AUD109 remains in the source IR and is resampled in physical time.

## Bounds and failure behavior

Positive representable rates and exact M×2×N storage are required before any indexing. Source rates retain their existing nearest-integer-Hz interpretation. Checked duration, dataset, byte-size, grid, unreduced backend rate arithmetic and crop-timeline calculations precede processing. Owned staging/scratch use fallible reservation.

Either FFT grid is capped at 262144 frames; transforms are twice the grid length. Very long IRs or rare nearly coprime/extreme rate pairs can therefore return an explicit unsupported-geometry error. This limits backend geometry; it does not pretend to measure the opaque FFT planner's exact allocation or make its internal allocation infallible. Actual input/output dimensions and delay are checked after construction against the integer preflight.

Valid same-rate data is unchanged. Valid empty datasets update rate/length metadata without constructing a backend. All tested errors preserve source samples and metadata.

## Evidence

Focused suite: `cargo test --offline -p sotf-plugin-binaural --test sofa_resampling`, **5 passed**. Log: `/tmp/sotf-sofa-resampling-green.log`.

- 36 impulse cases: 48→96, 96→48, 44.1→48, 48→44.1 kHz; N 16/64/512; first, early and near-final impulses. Nonzero finite output and peak time within one output sample of physical time, with independent opposite-polarity ear scaling.
- 12 interior direct-f64-DFT comparisons: 100/1000/4000 Hz at all four ratios. Complex response is compared to `(target/source) * exp(-j*2*pi*f*n/source)` with relative error below 2e-4; the impulse is far from cropped boundaries.
- Exact same-rate preservation and exact reversed-measurement-order results.
- Ten invalid rate/shape/overflow/unsupported-grid cases preserve all source samples and rate/length/count metadata. The coprime-rate fixture stays tiny and must fail before huge backend allocation.
- Three empty dataset shapes convert metadata successfully without samples.

Full Binaural suite: **126 passed, zero ignored**, `/tmp/sotf-sofa-binaural-full.log`. Strict all-target Clippy passed, `/tmp/sotf-sofa-binaural-clippy.log`. Independent implementation review found no blocker: [review](sofa-resampling-independent-review.md).

Combined loader/render proof in facade `tests/sofa_delay.rs` passes **10 tests**, with strict Clippy. It includes 18 bit-identical cross-rate metadata/manual-source waveform pairs and 72 first/final impulse peaks. Maximum timing error is **0.44217687074831247 frames**, within the independently chosen one-frame bound. This proves genuine materialized SOFA delay survives backend-delay removal. Logs: `/tmp/sotf-sofa-delay-cross-rate.log` and `/tmp/sotf-sofa-delay-cross-rate-clippy.log`.

Independent design review: `/tmp/sotf-sofa-resample-independent-review.md`. No realtime callback protocol or native hardware claim is introduced.
