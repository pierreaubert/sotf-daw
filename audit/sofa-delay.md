# AUD109: exact integer SOFA delay loading

Date: 2026-09-28. MIDI/IAMF excluded. No sibling, dependency, cache-schema or realtime callback changes.

## Implemented contract

`sofa::load_sofa` parses one immutable SOFA byte buffer through the pinned low-level HDF5 reader and constructs the unchanged public `SofaFile` type. Its `LoadedSofa` result records whether nonzero delay was materialized. IR shape is checked against M×2×N; rates must have a scalar/I/M representation, be uniform, positive and finite, and remain positive/finite in the existing f32 field. Scalar rate attributes and spherical/default/Cartesian position behavior remain compatible.

Shared [I,R] and per-measurement [M,R] exact nonnegative integer `Data.Delay` values are applied by zero-prefixing each ear's source HRIR and padding to one checked common length. Missing/all-zero delay leaves raw samples and length unchanged, including signed-zero samples. Fractional and negative nonzero delays return explicit unsupported-capability errors without rounding or claiming invalid SOFA. Nonfinite values and malformed shapes return validation errors.

The new delayed dataset is limited to 256 MiB, checked before reserve/zero-fill, with requested/allowed byte counts in the error. Checked frame/sample/byte arithmetic and fallible reservation precede materialization. This is a dataset limit, independent of any renderer's per-IR support; it does not imply every individually renderable measurement collection fits.

SQLite `.hrtfdb`, `.sqlite` and `.db` routes remain on the existing dependency loader, with delay_applied=false. Already materialized caches are not delayed twice. Lost metadata in old caches cannot be recovered.

## Consumer and lifecycle changes

Both Binaural file-loading entry points use the shared loader. Background tracking uses spectra prepared from the canonical loaded data; it does not reopen the file. Initialize stages existing configured/database-fallback path resolution, loading, resampling and the existing support validation before live sample-rate/smoother/FDN/crossfade/LFE changes, then reuses that prepared SofaFile. Setter preparation was already staged and remains so. New delay/rate/shape/support preparation failures preserve live settings, output history, partial drain state, active/previous/pending filter owners and the existing worker. Later unrelated filter-preparation errors retain their preexisting contract; this is not a general initialization rewrite.

XTC uses the shared result before preparing/publishing filters. For a newly materialized nonzero delay it rejects only selected plant IRs whose nonzero suffix extends beyond FFT N. Long unselected measurements and selected trailing-zero padding remain safe. Existing raw/zero-delay/SQLite long-IR truncation is explicitly preserved as a separate limitation.

Same-rate integer timing is proved by the loader oracles. The combined AUD109+AUD114 public regression additionally covers 48→96 kHz and both 44.1↔48 kHz directions after the separate resampler correction. This does not add fractional metadata support or claim universal cross-rate response accuracy.

## Independent evidence

Original standalone proof: `/tmp/sotf-sofa-delay-public-proof.md`, with immutable source `/tmp/sotf-sofa-delay-probe.rs` and pinned-reader provenance. Permanent public red: `/tmp/sotf-sofa-delay-permanent-red.log`, 0 passed/2 failed. Shared delay [0,3] produced Binaural maximum waveform error 0.004947916 and XTC source-1 error 0.018320596 against manually shifted twins.

Permanent facade tests in `crates/sotf-plugins/tests/sofa_delay.rs` now pass 10/10 (`/tmp/sotf-sofa-delay-cross-rate.log`; the original same-rate nine-test checkpoint is `/tmp/sotf-sofa-delay-matrix-final.log`):

- Shared and measurement-specific integer fixtures: three exact source positions, two ears, two programme markers, mixed 1/17/127 callback partition, process plus complete drain. All 24 onset checks equal declared scheduler delay + physical metadata delay, including the final marker.
- Independent f64 DFT at five bins per ear/measurement matches the analytic impulse shift theorem, with exact sample/padding checks. Manual shifted twins compare full Binaural and XTC emitted waveforms with 2e-7 maximum-error gates.
- Thirty-six no-delay/all-zero × coordinate/default × scalar/I/M/attribute-rate loader parity cases, plus exact no-delay versus zero-delay Binaural/XTC waveform controls.
- Explicit fractional/negative/nonfinite/overflow/1-billion-sample-cap errors, malformed delay/IR/rate shapes, nonuniform/nonpositive/nonfinite/unrepresentable rates. The huge-delay fixture is tiny and is rejected before large allocation.
- XTC selected versus unselected support, pure trailing-zero padding, exact N−1/N boundary, and preserved legacy truncation controls.
- Binaural and XTC failed preparation with nonzero live history and partially served tails: unchanged settings/remaining waveform, including attempted Binaural 48→96 kHz failed initialization.
- Three SQLite extension routes preserve already materialized samples and exact consumer waveforms.
- Combined metadata plus resampling: shared/MR delays × three rate pairs × three source positions =18 full consumer waveform comparisons, all bit-identical to manually shifted source twins. The independent first/final physical-peak oracle performs72 checks, maximum error0.44217687074831247 output frames (unchanged ≤1-frame bound). Twins share the canonical padded source window, since AUD114 deliberately retains ceil(source_length*ratio) rather than an infinite sinc tail.

New Binaural private owner regression (`src/lib/sofa_delay_tests.rs`) passes 1/1, two active/partial-tail epochs (`/tmp/sotf-sofa-delay-ownership.log`). It verifies identity of ready publication, active and previous snapshots, existing worker thread, input/output histories, crossfade counters, old rate and remaining drain-work bound after invalid-delay reinitialization.

Independent read-only review by plugin_chain found no introduced arithmetic/ownership blocker in the loader or XTC support check.

## Verification

- `cargo test -p sotf-host -p sotf-plugin-xtc`: **879 passed, 9 existing ignored doctests**, `/tmp/sotf-sofa-host-xtc-full.log` (host667/8ignored; XTC212/1ignored). Includes existing cold allocation/free and XTC process/EOS/AutoGain gates; no callback changes were introduced here.
- `cargo clippy -p sotf-host -p sotf-plugin-xtc --all-targets -- -D warnings`: passed, `/tmp/sotf-sofa-host-xtc-clippy.log`.
- `cargo test -p sotf-plugin-binaural`: **126 passed, 0 failed/ignored**, `/tmp/sotf-sofa-binaural-full.log`. Includes AUD114 five tests, all existing cold checks, and the AUD109 owner regression.
- `cargo clippy -p sotf-plugin-binaural --all-targets -- -D warnings`: passed, `/tmp/sotf-sofa-binaural-clippy.log`.
- `cargo clippy -p sotf-plugins --no-default-features --test sofa_delay -- -D warnings`: passed, `/tmp/sotf-sofa-delay-facade-clippy.log`.

The public regression binary uses the existing facade dependencies; no manifest or lockfile changes were needed. No native platform execution or full signed/fractional SOFA compliance claim is made.

AUD114 implementation read-review found no blocker; detailed geometry/flush/transaction proof: `/tmp/sotf-sofa-resample-implementation-independent-review.md`. Final combined-test strict Clippy passed at `/tmp/sotf-sofa-delay-cross-rate-clippy.log`; scoped rustfmt --check and git diff --check passed. Source and tests are frozen.
