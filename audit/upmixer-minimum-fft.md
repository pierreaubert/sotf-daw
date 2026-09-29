# AUD129: Upmixer minimum FFT geometry

## Finding and disposition

`UpmixerPlugin::new` accepted FFT size 1 because it checked only that the size
was a power of two. Its half-overlap hop was therefore zero, and the processing
loop could fail to consume input. The parameter factory also rounded configured
0 and 1 to 1. Separately, the HR output ring was sized only from the main FFT;
at main N=32 it held 128 frames although the fixed 512-point HR path writes a
256-frame hop.

The minimum supported WOLA geometry is N=2: the periodic sqrt-Hann window is
`[0, 1]`, and its squared windows sum to one at hop one. This defines a valid,
progressing transform only. N=2 has no useful spatial-frequency resolution
and is not a full-quality upmix configuration.

Direct construction now rejects N=0 and N=1 and documents the N>=2 bound. The
factory maps configured 0 and 1 to 2, preserves existing power-of-two sizes,
keeps upward rounding (`3 -> 4`), and retains the low-latency override to 1024.
The HR accumulator now uses `4 * max(main_fft_size, hr_fft_size)` at construction,
FFT resize, and speaker-layout preparation. Existing 64/128 behavior, causal
AutoGain, and finite EOS behavior were retained.

The public regression matrix processes sizes 2, 4, 8, 16, and 32 in both
decorrelation modes, at 44.1 and 96 kHz, with 5.1 and 7.1.4 layouts, HR direct
and AutoGain enabled and disabled, and reset/fresh-instance comparisons. Every
input lasts `sample_rate / 10 + 512 + N` frames, so enabled AutoGain crosses its
100 ms refresh and the HR route processes multiple windows. Existing N=64 and
N=128 public coverage remains in place.

## Accuracy, lifecycle, and realtime evidence

- The internal neutral identity oracle exercises every impulse phase at
  N=2, 4, 8, 16, 32, 64, and 512 over three callback partitions. Maximum-error
  assertions are below `2e-6`.
- The N=2 oracle separately checks DC, Nyquist, deterministic dense samples,
  exact latency/frame count, and finite-stream EOS against a delayed-input
  expectation with maximum error below `2e-6`.
- The public route test verifies process/reset/fresh-instance equality and
  finite output across the low sizes and route matrix. Factory checks cover
  0/1/2/3 plus `low_latency=true`; direct constructor checks reject 0 and 1.
- The public layout-change test exercises N=32 with HR enabled while changing
  stereo to 5.1 and 7.1.4.
- Fresh-thread allocator tests cover N=2 through 32 with HR and AutoGain
  enabled, before and after reset. The tests assert zero callback allocations
  and deallocations. This is allocation evidence, not a CPU benchmark.
- A neutral HR-only block impulse probe observes a nonzero retained HR-ring
  peak at frame 0 for N=2 and N=32. This isolates the ring after its startup
  discard and does not by itself establish stream alignment.
- A synchronized stream probe compares the real HR path with a reference that
  keeps the same main-path state and disables only HR channel writes. It runs
  1024 input frames, irregular and 512-frame callback partitions, and finite
  EOS. For both N=2 and N=32, the main impulse peaks at API latency N while
  the HR-only contribution first appears and peaks at frame 512. The measured
  HR arrival is therefore 510 frames later at N=2 and 480 frames later at
  N=32, with the same result for both callback partitions. This confirms a
  sub-512 end-to-end HR/main timing offset. AUD130 records these measurements
  separately from AUD129's minimum geometry and HR-ring capacity fix. Whether
  the observed offset violates intended timing is unresolved; production was
  left unchanged pending that review.

## Verification

All final package tests were run against the same formatted source snapshot;
the pre-test and post-test manifests match. Manifest SHA-256:
`d0ef11f8e20a005c3483bdefee80b9dea3cb945c498938c64e15c5fb18f06835`.
Per-file manifests: `/tmp/sotf-aud129-package-accepted-start.sha256` and
`/tmp/sotf-aud129-package-accepted-end.sha256`.

- `CARGO_NET_OFFLINE=true cargo fmt --package sotf-plugin-upmixer -- --check` —
  passed.
- `CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target TMPDIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target/audit-tmp CARGO_NET_OFFLINE=true cargo test --offline -p sotf-plugin-upmixer` —
  164 passed, 0 failed. Log:
  `/tmp/sotf-aud129-upmixer-package-accepted.log`.
- `CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target TMPDIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target/audit-tmp CARGO_NET_OFFLINE=true cargo clippy --offline -p sotf-plugin-upmixer --all-targets -- -D warnings` —
  passed. Log: `/tmp/sotf-aud129-upmixer-clippy-accepted.log`.
- Extended low-size public matrix — 6 passed. Log:
  `/tmp/sotf-aud129-small-matrix-final.log`.
- HR/main subpath impulse probe — 1 passed; measured frames are in
  `/tmp/sotf-aud129-hr-alignment-probe.log`.
- Synchronized stream-path probe — 1 passed. Output arrival evidence is in
  `/tmp/sotf-aud129-aud130-hr-probe-final.log`.

No timing benchmark was run. The shared `AUDIT.md` and
`audit/IMPLEMENTATION_PLAN.md` remain for the ledger owner; AUD130 has been
recorded there as the separate timing follow-up. Astra-medium accepted the
bounded AUD129 geometry/capacity scope. AUD130 timing intent and severity
remain unresolved.
