# AUD121 — true-peak interpolation calibration

## Defect and source

The host meter and private math-dsp backend used the same incorrect four-phase
FIR, including one identity phase. The existing host reference test repeated
those coefficients. It checked implementation agreement, not the claimed ITU
calibration. The earlier metering audit's accuracy claim is retracted.

The published interpolation coefficients are in
[ITU-R BS.1770-5 Annex 2, printed pages 18–19](https://www.itu.int/dms_pubrec/itu-r/rec/bs/R-REC-BS.1770-5-202311-I!!PDF-E.pdf).
The source's former “Table 2” label was also wrong: that table describes the
loudness high-pass stage. The PDF's interpolation rows were retrieved directly.
The same coefficients were also verified in
[BS.1770-4 Annex 2, printed page 17](https://www.itu.int/dms_pubrec/itu-r/rec/bs/R-REC-BS.1770-4-201510-S!!PDF-E.pdf).
This corrects the originally declared algorithm as well as matching the current
edition's interpolation table.

Four public regressions fail before production edits:

- Both meters report **−1.345348568 dBTP** for a steady 12 kHz/48 kHz sinusoid
  whose continuous peak is **−6.020599913 dBTP**: a **4.675251346 dB overread**.
  Startup is discarded before measurement. The expected amplitude is analytic,
  independent of any FIR coefficients.
- Host and backend interval peaks disagree with explicit zero-stuffed
  convolution using the published 48-tap row-major coefficients.

Actual executed failures: `/tmp/sotf-aud121-public-red.log`.

## Correction

Both production tables now contain the published columns, individually reversed
for the existing oldest-to-newest history iteration. Thus the first published
row multiplies the newest source sample. The host retains all four phases at
44.1/48 kHz and the published 0/2 phase subset at 88.2/96 kHz. The backend keeps
its existing four-phase execution and warning policy outside 48 kHz.

The reference represents the source as exact integer numerators over 32768,
in printed row order. It inserts three zeros between source samples and applies
a full causal 48-tap convolution, then groups emitted peaks by source interval.
It uses neither production coefficients nor the production history algorithm.
The obsolete coefficient-copy reference is removed. Backend ring/shift-register
equivalence remains a useful separate implementation check.

No audio-processing filter, public parameter, allocation strategy, history
length, reset, peak-query ownership or publication protocol changes. Reported
true peaks intentionally change. Existing high-rate availability remains a
separate capability limit.

## Verification

- All four public regressions pass. The analytic tone is within 0.15 dB of its
  known continuous amplitude. Full-convolution interval checks use a 2e−12
  absolute tolerance in their respective units (linear backend, dB host).
- Host convolution checks cover 16 rate/channel configurations, two callback
  partitions and two reset epochs: 64 complete streams. Signals contain initial
  silence, impulses, signed channel differences, DC, dense deterministic audio
  and explicit trailing zeros. Every channel and every query interval is checked;
  repeated empty queries return the defined empty peak.
- Backend public convolution checks cover 1/2/6/24 channels at 48 kHz through
  irregular queries, ring wrapping and two reset epochs.
- Two fresh-thread heap tests cover 16 host and four backend configurations,
  first processing, repeated queries and reset. Both epochs allocate and free
  **zero** objects. Logs: `/tmp/sotf-aud121-{public-green,heap}.log`.
- The full host suite passes **704 tests with eight existing ignored doctests**;
  the two subsequently added heap tests also pass. Existing retained-snapshot,
  LRA, sample-peak and AutoGain regressions remain green.
- All **24 backend EBU tests** pass. Strict host all-target Clippy passes.
  Backend library Clippy passes with the two previously documented upstream
  exceptions. Logs: `/tmp/sotf-aud121-{host-full,backend,host-clippy,backend-clippy}.log`.
- Checkpoint 18 passes **5,975 tests across 352 binaries**, with ten skipped,
  including FFI and excluding MIDI/IAMF package tests. The build took 1m13s;
  tests took 79.379s. All 408 changed in-scope non-vendored Rust files pass
  formatting and scoped diff checks; the changed vendored constant file also
  passes rustfmt. Log: `/tmp/sotf-audit-wave18-nextest.log`.

## Unchanged telemetry

Before editing production, 20 public monitor configurations (five rates,
1/2/6/24 channels) were captured across two four-second epochs with irregular
callbacks, spatial correlation, LRA and reset. All **7,232 serialized snapshots**
are identical after excluding the intentionally corrected `true_peaks_dbtp`
array. This includes the loudness values, LRA, sample peaks, correlation and
validity/status fields; no NaN loudness is accepted by the fixture.

The 27,031,183-byte captures have SHA-256
`c8dc8c1731c209bfba6b418350a403abb7f794c2d331d1a28160a0e15f608832`.
Captures, source backups, manifest and temporary example source are preserved
under `crates/sotf-plugins/target/audit-tmp/aud121-*`. The example is removed
from the repository source tree. Logs:
`/tmp/sotf-aud121-baseline-{before,after}.log`.

## Limits

These checks establish the published FIR calculation and correct the large
calibration error. This short oversampled FIR still approximates the continuous
peak; it is not an exact arbitrary-phase/near-Nyquist reconstructor. Neither
these tests nor existing compliance flags establish full external programme
certification. The authentic EBU corpus remains unavailable in this environment.
The correction does not expand supported host true-peak rates or change the
existing nonfinite-input policy.
