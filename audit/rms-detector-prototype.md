# AUD067 isolated RMS verification and integration recommendation

Date: 2026-09-28. Production, sibling checkout and Cargo source caches unchanged.
All build output is under the workspace `target/audit-rms-probe` directory.

## Artifacts and verification

- Project: `/tmp/sotf-rms-probe/` (no external dependencies).
- `src/stock.rs`: byte-identical pinned detector, SHA256
  `be7c3de8bc8aff48b473b620314f35a4a7251f266d010b73dd57c7d04bc76687`.
- `src/candidate.rs`: minimal f64 energy/history overflow correction only.
- `src/tree.rs`: separately implemented preallocated f64 nonnegative sum tree.
- Separate reviewable patches: `/tmp/sotf-rms-minimal.patch` and
  `/tmp/sotf-rms-tree.patch`.
- Tests: `/tmp/sotf-rms-tree-tests.log`: **23 passed**, zero skipped/failed.
  This includes the copied upstream tests for all three versions. Tests that
  establish the minimal version's remaining defect deliberately assert that
  defect; this count is not a claim that the minimal version is accurate.
- Clippy: `/tmp/sotf-rms-tree-clippy.log`, all targets, warnings denied: passed.
- Throughput: `/tmp/sotf-rms-tree-throughput.log`.
- Standalone vendor-manifest proposal, not integrated or package-built:
  `/tmp/sotf-math-dsp-standalone.Cargo.toml`.

Commands:

```sh
cargo test --manifest-path /tmp/sotf-rms-probe/Cargo.toml --target-dir /home/pierre/src/all_of_sotf/sotf-daw/target/audit-rms-probe -- --nocapture --test-threads=1
cargo clippy --manifest-path /tmp/sotf-rms-probe/Cargo.toml --target-dir /home/pierre/src/all_of_sotf/sotf-daw/target/audit-rms-probe --all-targets -- -D warnings
cargo run --release --manifest-path /tmp/sotf-rms-probe/Cargo.toml --target-dir /home/pierre/src/all_of_sotf/sotf-daw/target/audit-rms-probe
```

## Minimal overflow correction is insufficient

Casting the sample to f64 before multiplication AND storing squared history in
f64 removes the f32 square overflow. It stays finite for the tested isolated,
repeated and overlapping extreme pulses, including constant f32::MAX.

It does not fix subtraction cancellation. With a 480-frame window, feed a 0.1
background, insert one finite 1e20 pulse at frame480, then continue 0.1. By
frame1440 the direct-window RMS is0.10000000149011624, but the minimal version
reports exactly zero, persisting through frame3839. A 1e-5 background is also
lost permanently in the tested steady continuation. The dominant rolling sum
absorbs the small contributions; removing the dominant square cannot recover
them. No claim of full dynamic-range accuracy should accompany that patch.

## Sum-tree design

Let N be the existing window length and P its next power of two. Allocate2P
f64 nodes. Leaves P through P+N-1 form the circular history; remaining leaves
stay zero. Replace one leaf with the sample's f64 square and recompute its
ancestors as the sum of their two nonnegative children. The root is total
window energy, and output remains sqrt(root/N).

This retains:

- Original rounded window length and zero-prefilled startup divisor N.
- Original Peak path, public methods and mode/reset semantics.
- Exactly N samples of retained history, even when N is not a power of two.
- No processing/reset allocation or freeing. Mode changes still prepare/resize
  storage outside processing, as the current API already does.

Each update has exactly log2(P) parent additions, bounded by the prepared window
size. No outgoing energy is subtracted. A large pulse can make tiny energy
irrelevant to a current parent sum, but it does not overwrite the smaller
subtree; recomputing the ancestor path when the large leaf expires restores
the remaining energy. A window of all zeros produces an exactly zero root.

Finite f32 values convert exactly to f64 and their squared significands fit in
f64. Nonnegative pairwise summation has depth-dependent rounding error, rather
than cancellation against an outgoing dominant value. Every tested final f32
output is within one ULP of the independent direct f64 window calculation.
The tests cover finite inputs; this prototype does not introduce a new policy
for NaN/infinite inputs into the shared detector API.

## Independent numerical evidence

The oracle directly recomputes each selected window from source samples in f64;
it does not use a rolling subtraction or production tree nodes.

| Matrix | Coverage | Result |
|---|---|---|
| Normal audio |100 configurations: 5 rates (8/44.1/48/96/192 kHz),4 durations (0.01/1/10/50 ms),5 signals (two DC, sine, two-tone, deterministic noise) |406300 outputs; sampled direct-window checkpoints within1 ULP |
| Extreme impulses |120 configurations:5 rates ×2 amplitudes (1e20/f32::MAX) ×4 pulse counts (1/2/3/17) ×3 backgrounds (0/0.1/1e-5) |Every output within1 ULP; true silence exactly zero |
| Constant maximum |5 rates, constant f32::MAX through startup and settled windows |Finite, within1 ULP |
| Magnitude ladders |16 configurations:4 rates ×ascending/descending amplitudes ×two ring phases; amplitudes from f32::MAX to minimum subnormal |Every output within1 ULP, smaller pulses recovered after larger ones expire |
| Reset/mode transitions |5 rates, five window changes including growth and shrink, Peak extrema, reset after maximum energy |Exact parity with fresh detector |
| Cold realtime |Window1,480,9600 on new threads; first process, repeated extremes, reset and more processing |Measured0 allocations and0 frees |

### Normal finite output changes are small but not bit-identical

Both the minimal f64 version and the tree differ from stock on6874 of406300
normal outputs (about1.69%). Maximum stock difference is **one f32 ULP**,
absolute5.960464477539063e-8. The tree's largest corresponding dB difference is
1.0346113378766716e-6 dB. Maximum relative error against the direct f64 oracle
is5.816102541745813e-8, consistent with final f32 rounding. This is expected
from removing the old f32 square rounding; do not claim normal bit identity.

The minimal-versus-tree recovery regression ends at zero for the minimal
version and exactly0.1 for the tree. Normal and extreme accuracy checks retain
the same one-ULP acceptance bound; no tolerance was loosened for the tree.

## Runtime and memory tradeoff

AMD Ryzen Threadripper PRO3995WX, release opt-level3, three repetitions of
one million samples, median ns/sample, steady cache. Measurements are a small
single-thread microbenchmark under the shared workstation load, not a deadline
or full-plugin throughput guarantee.

| Window N | Stock bytes | Minimal f64 bytes | Tree bytes | Stock ns/sample | Minimal ns/sample | Tree ns/sample |
|---:|---:|---:|---:|---:|---:|---:|
|48 |192 |384 |1024 |6.703 |6.701 |11.513 |
|480 |1920 |3840 |8192 |3.436 |3.434 |16.081 |
|4800 |19200 |38400 |131072 |3.461 |3.460 |24.934 |
|48000 |192000 |384000 |1048576 |3.435 |3.438 |31.888 |

Storage is16P bytes: less than32N, versus4N stock or8N minimal. At the normal
10 ms/48 kHz window, tree processing is about4.68× this tiny original detector
kernel; the measured absolute increase is about12.6 ns/sample. Reset clears
the prepared tree in O(P); processing remains O(log P). Peak allocates no tree.

Alternative designs were considered only conceptually: compensated rolling
sums reduce cancellation, but need careful dominant-removal error tests;
periodic full rebasing is O(N) on individual callbacks and permits stale errors
between rebases. A block/tree combination could trade memory against bounded
work, but was not implemented or verified. The full sum tree has a direct
window-ownership invariant and immediate recovery at every outgoing sample.

## Read-only upstream and dependency assessment

Pinned source: `math-dsp`0.5.30, math-audio revision
`cabbc6dc1c3d0c8aad275ac33ec015d174859c89`, actual path:

`/home/pierre/.cargo/git/checkouts/math-audio-ae5031fdb189131a/cabbc6d/crates/math-dsp`.

Sibling HEAD is `e9ecb90e8838a2006e42be241919e601e05bd064`. Its detector and
math-dsp manifest are identical to the pin (`git diff pin HEAD -- ...` empty).
The latest detector change in available local refs is
`c87ddfb93bf833b32b30be44e58e1a64f8b68cc0`; none supplies this fix. The sibling
has unrelated user changes in math-autodiff; those were only observed and left
untouched. No remote fetch was performed, so this is not a claim about unseen
upstream revisions.

The pinned math-dsp package is about1.40 MB/143 files. Its manifest inherits
workspace package metadata and dependencies. Its only normal dependency on
another math-audio crate is math-iir-fir0.5.24. All other normal dependencies
are external packages already represented in the SOTF lockfile. The proposed
standalone manifest resolves those inherited versions/features explicitly,
preserving binaries and benchmarks. License is ISC in both the upstream root
manifest and README; upstream authorship metadata is retained.

### Recommended integration, after approval and shared-crate tests

1. Prefer an upstream fixed revision when a reviewed one actually exists. None
   was found in the available local history; upgrading to the current sibling
   revision alone would not fix this defect.
2. Otherwise vendor **only the math-dsp package**, preserving its upstream
   version0.5.30 and adding `publish=false`, provenance, patch/test details and
   upstream ISC/license information. Copy from the exact pinned revision,
   replace only detector.rs with the accepted tree implementation, and add the
   independent regressions. Do not fork the full math-audio workspace.
3. Resolve the inherited manifest as in
   `/tmp/sotf-math-dsp-standalone.Cargo.toml`. Keep math-iir-fir on the existing
   git URL/source identity and exact version0.5.24; retain its source revision
   in the root lockfile. Adding a separate `rev=` form only in this manifest
   could create a second Cargo source identity/type universe, so coordinate
   any such pin across the complete dependency graph instead.
4. Use a root source patch for
   `[patch."https://github.com/pierreaubert/math-audio"]` targeting the private
   math-dsp path, and exclude the standalone vendor directory from workspace
   membership consistently with the existing private Rubato fork. Merely
   replacing one direct dependency can leave an old transitive math-dsp:
   math-analog, math-autodiff and math-qa also depend on math-dsp upstream.
5. Review lockfile/source resolution for a single math-dsp and a single intended
   math-iir-fir source, then run the entire vendored math-dsp suite and the
   downstream plugin gate. This isolated23-test detector harness does not
   substitute for package-level dependency resolution or full-crate testing.

No proposed integration changes have been applied. The minimal patch and tree
patch remain separate artifacts for review.
