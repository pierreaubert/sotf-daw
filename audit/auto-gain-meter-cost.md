# AUD120 — AutoGain measurement cost

AutoGain now owns two private `GainMeter` instances using the same pinned
`math-dsp::EbuR128` backend with M/S/sample-peak measurement. Both loudness windows
remain warm across selection changes. Every refresh consumes the peak interval
for every channel, including channels with zero positional loudness weight.

This removes unused integrated history and scans, true-peak FIRs, stereo
correlation and display arrays. The pair no longer requests 576,000 bytes for
integrated histories, in addition to the other removed analyzer storage. The
generic analyzer, its LRA support and the EBU backend source are unchanged.
Public interfaces, gain recurrence, caller timelines, delays and safety-stage
ordering are preserved. The existing positional channel-weight convention is
also preserved; this does not establish semantic multichannel compliance.

## Exact compatibility

Before changing production code, public examples captured these baselines:

- 16 shared-helper cases: 1/2/6/24 channels, 48/192 kHz, initially M/S,
  irregular ingestion, unequal input/output refresh schedules, warmed window
  selection changes, target/limit/smoothing/enable controls, silence and reset.
  All **17,280,000 scalar frame gains** and telemetry bits match afterward.
  The 69,442,048-byte trace SHA-256 is
  `e662363c3e759ce629b81d8677721e8b81affe5628e5bbf659b683468f96a870`.
- 52 actual EQ configurations: four seconds of active AutoGain warmup, multiple
  filter topologies and banks, native DF1/TDF2 and applicable 2x/4x routes,
  48/192 kHz, large irregular callbacks, live band changes and finite drain.
  **1,122,344 audio samples, 8,192 tail samples and declared latencies** match
  bit-for-bit. Boost fixtures explicitly assert nontrivial compensation.

Artifacts, probe sources, original production sources and before/after binaries
are under `crates/sotf-plugins/target/audit-tmp/aud120-*` and
`audit_meter_*`. Manifests record binary/source hashes. The temporary examples
were removed from the source tree. Logs are `/tmp/sotf-aud120-{helper,eq}-{before,after}.log`.

## Permanent checks

Four public tests compare measurements against independent, unchanged generic
`LoudnessMonitor` instances. The 24 ordinary configurations cover 1/2/6/24
channels and 8/44.1/44.101/48/96/192 kHz, cold and warm M/S windows, silence,
per-channel early peaks, incomplete intervals, repeated empty queries and reset.
Other cases check malformed-call preflight without consuming pending peaks,
NaN/infinite/extreme input policy and reset recovery, constructor error strings,
and successful sample-rate replacement, including the accepted endpoints.
Finite values must match bits; NaN classifications must agree.

Two heap tests cover eight cold configurations, live scalar controls, processing
and reset; plus two hour-long epochs at four channel/rate configurations.
Processing, query and reset show **zero allocations and frees**. Nonzero long
audio uses 8 kHz stereo. Accelerated 20 Hz cases use silence to isolate storage
and make no filter-accuracy claim outside ordinary audio rates. Error-path
strings and control-thread constructor/rate changes remain allocation-permitted.

Focused logs: `/tmp/sotf-aud120-meter-tests.log` and
`/tmp/sotf-aud120-heap.log`.

Strict all-target Clippy passes for the host and all seven direct caller crates:
EQ, A/B Compare, Loudness Compensation, Crossfeed, XTC, AAE and Upmixer.
Log: `/tmp/sotf-aud120-callers-clippy.log`.

Checkpoint 17 passes 5,903 workspace tests plus 66 FFI tests, with 10 skipped;
MIDI and IAMF package tests are excluded. This includes the existing independent
scalar-gain, causal clock, reference alignment, EOS and accuracy regressions in
all callers. See `AUDIT.md` for scope and logs.

## Matched local CPU measurements

Seven alternating before/after trials used preserved binaries from the same
optimized test profile (`opt-level=1`, native CPU), pinned to logical CPU 0.
The host is an AMD Threadripper PRO 3995WX with Rust 1.98.1.
No other audit tests or builds ran during measurement. These are local
throughput results, not release-profile or callback-deadline guarantees.

EQ uses stereo, two bands, 1024-frame callbacks, 100 warmup callbacks, then
five sets of 300 callbacks; each trial reports the median set. The helper uses
two/six channels, 300 warmup callbacks and the same five measured sets, refreshing
both meters each callback. The tables report median times across seven trials
and median paired after/before ratios. Disabled EQ still measures diagnostics.

| EQ rate | Factor | AutoGain | Before ns/frame | After ns/frame | Paired ratio |
|---|---:|---|---:|---:|---:|
| 48 kHz | 1 | false | 225.908 | 35.267 | 0.1561 |
| 48 kHz | 1 | true | 235.152 | 43.933 | 0.1868 |
| 48 kHz | 2 | false | 309.167 | 115.798 | 0.3745 |
| 48 kHz | 2 | true | 318.666 | 124.483 | 0.3910 |
| 48 kHz | 4 | false | 379.245 | 184.063 | 0.4865 |
| 48 kHz | 4 | true | 386.917 | 193.067 | 0.4985 |
| 192 kHz | 1 | false | 54.657 | 35.371 | 0.6470 |
| 192 kHz | 1 | true | 59.357 | 39.900 | 0.6721 |
| 192 kHz | 2 | false | 135.599 | 115.695 | 0.8536 |
| 192 kHz | 2 | true | 140.521 | 120.257 | 0.8550 |
| 192 kHz | 4 | false | 204.874 | 184.733 | 0.9017 |
| 192 kHz | 4 | true | 208.583 | 188.509 | 0.9030 |

| Helper rate | Channels | Enabled | Before ns/frame | After ns/frame | Paired ratio |
|---|---:|---|---:|---:|---:|
| 48 kHz | 2 | false | 192.135 | 22.658 | 0.1181 |
| 48 kHz | 2 | true | 196.787 | 27.210 | 0.1379 |
| 48 kHz | 6 | false | 518.957 | 63.802 | 0.1231 |
| 48 kHz | 6 | true | 523.114 | 68.446 | 0.1307 |
| 192 kHz | 2 | false | 42.802 | 22.677 | 0.5298 |
| 192 kHz | 2 | true | 42.851 | 22.683 | 0.5287 |
| 192 kHz | 6 | false | 64.400 | 63.753 | 0.9891 |
| 192 kHz | 6 | true | 64.488 | 63.799 | 0.9890 |

The largest savings occur at 48 kHz where the old generic monitor runs its
compliant true-peak FIR. At 192 kHz that FIR is already inactive. Six-channel
192 kHz helper results are close to unchanged: approximately 1.1% median
improvement, with one enabled trial slightly slower (ratio 1.0071). Do not
interpret that case as a large performance improvement.

Raw trials and `aud120-cost-manifest.json` include every timing and ratio range;
the orchestration script and summary are `/tmp/sotf-aud120-cost.py` and `.log`.

## Limits

This is an arithmetic-preserving private optimization. The earlier AUD112/AUD115
clock corrections deliberately changed enabled behavior and remain separately
documented. The old external Cargo cache disappeared before this work, so these
new artifacts do not establish the still-pending historical AUD115 CPU comparison.
Failed sample-rate replacement retains its preexisting partial-update behavior;
no additional transactionality is claimed here.
