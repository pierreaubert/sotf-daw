# AUD067 — shared RMS detector correction, integrated and verified

## Delivered scope

- `../math-audio/crates/math-dsp`: the SotF DSP crate, edited in place in
  the sibling math-audio checkout (version 0.5.30, previously developed
  against base revision cabbc6dc1c3d0c8aad275ac33ec015d174859c89; an
  earlier private vendored copy under `crates/3rdparties/math-dsp` was
  removed in favour of the sibling checkout).
- Only sibling source file changed: `src/detector.rs`, plus the independent
  regression test `tests/rms_accuracy.rs`.
- Root Cargo.toml patches the existing math-audio git source to the sibling
  checkout for math-dsp and math-iir-fir (same as sotf), keeping DSP
  primitives and their public IIR types on one revision.
- Cargo.lock: math-dsp and math-iir-fir resolve to the sibling path sources.
  All preexisting edits retained.
- New `sotf-plugin-gate/tests/rms_detector_accuracy.rs`. No Gate DSP, mode,
  parameter, host adapter, NIH, MIDI, or IAMF source changed in this task.

## Numerical correction

The original f32 multiply overflows before its f64 conversion for finite
samples above sqrt(f32::MAX). An f64 multiply plus f64 rolling history removes
that overflow but still loses retained quiet energy permanently through
subtractive cancellation when a dominant pulse expires. That insufficient
minimal patch remains only an isolated audit artifact.

The implemented detector stores f64 squared samples as circular leaves in a
preallocated binary sum tree. Updating one leaf recomputes only its ancestors
using nonnegative additions. Quiet subtrees recover immediately as dominant
pulses expire; no rolling subtraction can erase them permanently. The original
rounded N-sample window, zero-prefilled startup divisor N, Peak API/behavior,
and reset/mode lifecycle are preserved. No new NaN/infinity input policy is
claimed.

Independent direct f64 window references cover 100 normal configurations,
120 extreme-pulse/background configurations, five constant f32::MAX signals,
and 16 magnitude ladders spanning the finite f32 range at differing circular
phases. Checked output values stay within one f32 ULP of that independent
oracle, and empty windows return exact zero. Explicit cold thread-local
allocator counters measure zero allocations AND zero frees through processing
and reset, including 1-, 480-, and 9600-frame windows.

Normal finite audio is not bit-identical: the isolated 406300-output comparison
found 6874 changed values (1.69%), maximum one ULP / 5.960464477539063e-8 absolute
/ 1.0346113378766716e-6 dB. This reflects f64 rather than f32 energy rounding.
Known stock/minimal failure assertions are excluded from production tests.

## Costs and limits

With P = next_power_of_two(N), prepared storage is 2P f64 values = 16P bytes;
one sample requires log2(P) ancestor additions. Reset clears the prepared tree.
For N=480, storage grows 1920 -> 8192 bytes per detector. A warm-cache release
microbenchmark on an AMD Threadripper PRO 3995WX measured roughly 3.44 ->
16.1 ns/sample (4.68x for the detector kernel, about +12.6 ns/sample). This is
not a complete-plugin performance or realtime deadline guarantee. Full table
and reproducibility details remain in /tmp/sotf-rms-tree-verified.md and
/tmp/sotf-rms-tree-throughput.log.

## Dependency identity

Offline cargo metadata proves exactly one local math-dsp 0.5.30 for all 20
immediate consumers, including upstream math-analog. Exactly one math-iir-fir
0.5.24 remains at the original cabbc6dc revision. No package or revision bump,
no network fetch, and no sibling/Cargo source-cache modification was required.

Metadata: /tmp/sotf-rms-integrated-metadata.json
Resolution: /tmp/sotf-rms-integrated-resolution.log
Pre-integration snapshots: /tmp/sotf-rms-before-integration-Cargo.{toml,lock}

## Verification

- Full staged package release suite: 641 passed; two original ignored doctests.
  612 unit + 22 original integration + 5 new RMS integration + 2 doctests.
  /tmp/sotf-math-dsp-stage-release-tests.log.
- Same complete package suite from final repository vendor location: 641
  passed, zero failures, two original ignored doctests.
  /tmp/sotf-math-dsp-vendor-tests.log.
- Host, Gate, MultibandExpander, MultibandCompressor, Limiter, DeEsser --lib
  --tests: 1147 passed across 43 binaries, zero failures/ignores.
  /tmp/sotf-rms-affected-tests.log.
- New public Gate test: 18 combinations (44.1/48/96k, Upward/Duck, zero/two quiet
  backgrounds) with overlapping huge pulses, independent RMS + gain law +
  timing, and varying callbacks. Each audio sample stays within the existing
  Gate fast-math 0.02 dB error contract. External sidechain remains untouched.
- Strict all-target/all-feature Clippy for the same six workspace packages:
  PASS, /tmp/sotf-rms-affected-clippy.log.
- Standalone vendor all-target Clippy: PASS with two explicit unchanged
  upstream lint exceptions only (chunks_exact_to_as_chunks in utils.rs:36,
  manual_slice_fill in fdn.rs:120). Untouched pinned-package strict Clippy
  reproduces both; no source suppressions or unrelated fixes were added.
  /tmp/sotf-math-dsp-vendor-clippy.log;
  /tmp/sotf-math-dsp-stock-clippy.log;
  /tmp/sotf-math-dsp-stage-clippy.log.
- rustfmt --check on the changed detector and both new tests: PASS.
- git diff --check for root manifest/lock changes: PASS.

The first unoptimized full suite was superseded after its unchanged large
ESPRIT two-SVD test ran CPU-active for over ten minutes. The same test passes
in the complete release suites; no tests were filtered or tolerances relaxed.
The superseded debug process was terminated only after the optimized suite
completed successfully.

Source and report are frozen at this checkpoint. No commits or publication.
