# AUD122 — true-peak measurements at end of stream

## Reproduced defects

A final unit impulse at 48 kHz measured **−30.699840854 dBTP** in both the
public loudness plugin and the actual linear host. Its complete published FIR
response peaks at **−0.245173846 dBTP**, a **30.454667008 dB underread** in this
fixture. The analyzer inherited immediate drain completion and omitted the
eleven source intervals retained by its interpolation filter.

Both public regressions failed before production changes:
`/tmp/sotf-aud122-public-red.log`. The full-response oracle explicitly appends
eleven source zeros and convolves the independent row-major integer fixture
introduced by [AUD121](true-peak-coefficients.md).

After fixing the plugin, a real processing-worker regression still failed with
the same stale value. The engine refreshed shared UI data after ordinary audio
processing but never after drain. Executed failure:
`/tmp/sotf-aud122-engine-red.log`.

## Implementation and contract

- Both low-level meters expose `finish_true_peak()`. It finishes only the
  interpolation filter, retains unqueried peak maxima, and clears interpolation
  history. It adds no frames to loudness, LRA, K-weighting, sample peaks or
  correlation. Repeating it adds no duplicate response. Accepted later audio
  begins another interpolation segment within the existing loudness epoch;
  `reset` still starts a new complete measurement epoch.
- The plugin declares one drain call, validates initialization/rate/zero input
  frames before mutation, emits zero audio and leaves the destination untouched.
  Its final publication extends the last published peak interval through the
  filter suffix, retaining sample peaks and the maximum true peak. New input
  resumes ordinary query intervals. Repeated completed drain and empty process
  calls preserve the final snapshot.
- Publication uses the existing prepared slots and authoritative nested-Arc
  readiness checks. A failed publication consumes no measurement query. Direct
  repeated drain retries without repeating the filter response. Reset/disable
  epochs cannot merge pre-reset cached peaks.
- The engine calls its existing best-effort UI cache refresh after each
  successful host drain, including zero-output finalization, before forwarding
  EOS. No host queue or manager transition protocol is changed.

The final measurement does not add audio latency or append silent programme
frames. Ordinary accepted-input filter arithmetic and peak intervals are
unchanged. The backend's existing rate-warning policy is retained.

## Verification

- Eleven public host integration tests cover direct processing, analyzer taps,
  compiled plugin entry points, interpreted/compiled host execution, upstream
  tails, malformed drain contexts, reset/reinitialize/disable, resumed input,
  retained outer and nested Weak readers, delayed publication and unsupported
  192 kHz status.
- The final-position matrix covers four supported rates, 1/2/6/24 channels,
  three plugin paths and all twelve final impulse positions: **576 runs** with
  irregular 1/17/137/18-frame callbacks. Full-convolution comparison uses
  2e−12 dB tolerance. A separate dense-signal low-level matrix verifies the full
  response at all sixteen rate/channel combinations.
- Final plugin snapshots match all other serialized measurement fields exactly,
  including active M/S/I, LRA, sample peaks, correlation and validity. Backend
  tests independently compare M/S/I, gating count/energy and sample peaks with
  an unfinished control; later accepted silence also leaves the loudness clocks
  identical. Repeat queries and reset do not replay final interpolation peaks.
- Fresh-thread heap tests cover sixteen low-level host configurations, four
  backend widths, and **96 plugin configurations** (rates, widths, spatial
  on/off, no readers/outer readers/nested Weak readers). First finalization,
  repeated drain, deferred publication, reset and a new segment allocate and
  free **zero** objects.
- Full host tests pass **718 tests**, with eight existing ignored doctests.
  All **68 processing-worker tests** and **24 backend EBU tests** pass. Strict
  host/engine all-target Clippy passes. Backend library Clippy passes with the
  same two preexisting exceptions documented in `SOTF_FORK.md`.

Logs: `/tmp/sotf-aud122-{host,engine-green,backend,clippy,backend-clippy}.log`.
Checkpoint 19 passes **5,988 tests across 353 binaries**, with ten skipped,
including the expanded nested-Weak heap matrix. FFI is included; MIDI/IAMF
package tests are excluded. Build time is 36.89s; test time is 80.577s.
All 410 changed in-scope non-vendored Rust files and both changed vendored Rust
files pass formatting, and scoped diff checks pass. Workspace log:
`/tmp/sotf-audit-wave19-nextest.log`.

## Limits

Final publication remains best effort if every prepared snapshot or nested
array is retained, or the publication lock is busy. A direct repeated plugin
drain can retry; an already completed host stage is not polled indefinitely.
The engine's shared cache independently follows its existing retention policy.
Callers needing an authoritative final export must release retained snapshots
and arrange a successful publication before treating cached data as final.

The reference here is the published finite FIR response. Its unit-impulse peak
is slightly below unity; this does not claim exact continuous reconstruction,
new sample-rate support or full external programme certification. Those limits
from AUD121 remain. Broader host branch queues and engine transition protocols
remain outside this change.
