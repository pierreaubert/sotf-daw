# AUD103 proposal — AEC constructor adaptive configuration parity

2026-09-28. Source inspection plus an isolated executable linked to immutable
existing public API rlibs. **Pre-implementation evidence below was collected without repository edits or Cargo runs.**
MIDI/IAMF excluded. AUD100's clock-preflight correction remains accepted and
unchanged. Root approved the scoped constructor correction after review; permanent verification is recorded in the implementation report.

## Public contract and source cause

`AecPlugin::new(sample_rate)` is an infallible public constructor; its current
supported direct-processing behavior is explicit in the new AUD100 regression
and established earlier constructor-only tests. AEC has no initialized guard on
ordinary process or drain. Requiring an additional initialize call would silently
narrow that public contract and is not the proposed correction.

Canonical defaults expose echo_tail_ms=200, step_size=0.5 and
post_filter_enabled=true (`params.rs`). There is no documented alternate learning
mode for the `new` route. Nevertheless `src/lib/aec_plugin.rs:76` currently calls
`TwoPathAec::new(B, echo_tail_samples, 0.3, 0.7)` while the public scalar is 0.5.
The backend constructor at `two_path.rs:86` hardcodes 48000 in its delegation to
`new_with_sample_rate`. Thus `new(rate)` has two hidden differences:

1. Background adaptive step is 0.7, not its reported/canonical 0.5.
2. Power smoothing and foreground promotion hold use a 48 kHz clock even at
   other positive construction rates.

By contrast, `from_params` and successful `initialize` call `rebuild_aec`, which
uses `TwoPathAec::new_with_sample_rate(B, support, step_size*0.6, step_size, rate)`.
At defaults that is foreground0.3/background0.5 at the requested clock. Reset
clears learned/history state but retains coefficients, so it does not repair the
constructor mismatch. A same-value step setter returns early; toggling the
structural step away and back before initialize forces the correct rebuild.

## Executed public waveform evidence

Artifacts:

- `/tmp/sotf-aec-constructor-probe.rs`
- `/tmp/sotf-aec-constructor-probe.log`
- `/tmp/sotf-aec-constructor-build.txt`
- `target/audit-tmp/aec-constructor-probe/`: executable and immutable AEC/host
  rlibs (selected from the latest existing artifacts using Cargo fingerprints).

The executable uses only public `AecPlugin`, settings/setter/getter, process,
reset, latency and tail APIs. No backend implementation is copied or mirrored.
Input is deterministic xorshift reference audio bounded by0.2, with microphone
`0.6*reference[t−21]+0.2*reference[t−113]` (zero prehistory), two seconds per case.
There are 18 histories: 3 rates × 3 partitions (`[1]`, `[73,257,511]`, `[8193]`)
× 2 reset epochs. Each history starts with zero learned/audio state and compares:

- raw `new(rate)`;
- raw + identical public step snapshot0.5;
- `from_params(rate, defaults)`;
- `new(rate)` + `initialize(rate)`;
- `new(rate)` + public step0.6 then0.5;
- `from_params(rate, step_size0.7)` as an independent distinguishing control.

All five nominal-default routes report the same three scalar values, latency256,
tail metadata, and post-filter mix1. The three properly prepared default routes
(from_params, initialize, changed-away/back setter) are **bit-identical** in every
history. Raw and same-snapshot are **bit-identical** to each other, but differ from
the canonical default output in every history:

| Rate | First differing output frame | Max absolute difference | RMS difference |
|---:|---:|---:|---:|
|44100|24832|0.106290713|0.0070110143|
|48000|26112|0.130970066|0.0080406423|
|96000|26112|0.157156866|0.0268965916|

Positions and metrics agree across all three partitions and both reset epochs.
At48000 the raw constructor's complete waveform is **bit-identical** to the
explicit step_size0.7 route, despite its getter reporting0.5. This independently
isolates the hidden background scalar at the rate where timing is otherwise the
same. At44100/96000 raw versus explicit0.7 still differs (maximum0.138891858 /
0.157156866 respectively), consistent with the separate wrong-clock coefficients.
The foreground configured scalar differs for explicit0.7, but ordinary foreground
adaptation currently uses scale0; its stored mu therefore does not change this
control's waveform. This is source-confirmed, not an assumption that foreground
learning scalars are generally interchangeable.

## Independent time-constant proof (source-grounded, not measured output latency)

For block B256 and intended smoothing time100ms, the documented recurrence
coefficient is `a=exp(−B/(Fs*0.100))`. A fixed48k coefficient applied once every
B/Fs seconds instead has effective time constant
`tau=−(B/Fs)/ln(a48k) = 0.100*48000/Fs` seconds.
Promotion hold is `ceil(0.133*Fs/B)` consecutive advantage blocks. The current
constructor always uses25 blocks. Independent f64 calculations give:

| Rate | Intended power alpha | Old alpha | Old effective tau | Intended hold blocks | Old hold duration |
|---:|---:|---:|---:|---:|---:|
|44100|0.943602873103|0.948063938493|108.843537ms|23|145.124717ms|
|48000|0.948063938493|0.948063938493|100ms|25|133.333333ms|
|96000|0.973685749353|0.948063938493|50ms|50|66.666667ms|

These coefficients are calculated from the inspected constructor and recurrence,
not obtained from private fields through the public probe. Existing cfg(test)
backend getters `power_alpha()` and `transfer_threshold()` permit permanent
independent scalar assertions without adding production API or duplicating DSP.

## Other rate preparation checked

The bug is confined to the TwoPath constructor call:

- AEC `sample_rate` metadata/clock guard stores the requested rate.
- Echo-tail sample count and prepared partition count already use that rate.
- Post-filter construction uses `new_with_timing(..., B, sample_rate)` for its
  10/80ms gain smoothing,50ms power detector and250ms leakage smoother.
- Public post-filter mix step uses requested `rate*0.010`; no constructor-rate
  discrepancy found there.
- The256-frame FIFO and FFT dimensions are rate independent.
- `from_params` validates rate/config, rebuilds TwoPath at the requested rate,
  and leaves the already-correct same-rate post-filter prepared by `new`.
- `initialize` rejects zero before changes, rebuilds both rate-dependent
  processors, and resets the stream. The valid-call construction parity probe
  confirms this route agrees with from_params.

`new(0)` remains an existing infallible-constructor corner case (from_params and
initialize reject0). Changing zero-rate admission or constructor return type is
outside this positive-rate parity proposal; do not weaken AUD100 or silently
introduce an initialization requirement to address it.

## Proposed narrow correction

Within `AecPlugin::new` only, prepare TwoPath through `new_with_sample_rate` using
the same canonical step scalar that is stored/exposed by this instance:

- Bind the canonical default step once (current0.5).
- Use foreground `step_size*0.6`, background `step_size`, requested `sample_rate`.
- Store that same scalar in the plugin field/cache.

Keep existing `from_params`, rebuild/initialize/reset, validation, process rate
preflight, FIFO, drain policy, metadata, post-filter and public signatures.
The backend's explicit legacy `TwoPathAec::new(...)->48k` helper may remain for
its own existing internal tests; the rate-aware public plugin should not call it.
No host/native/config-layer changes are required.

## Permanent regression scope after approval

1. Public zero-history constructor/new+initialize/from_params equality for canonical
   defaults at44.1/48/96k, deterministic echo input, varied callbacks and reset;
   preserve one-frame constructor-time processing and the AUD100 rejected-call test.
2. Existing backend getters assert the constructor's power alpha against an
   independent f64 exponential and promotion count against time/block arithmetic.
   At48k additionally ensure the new route matches default0.5, and differs from
   explicit0.7 once adaptation is observable; no production-fast-math oracle.
3. Metadata/scalar/default and post-filter construction parity, including existing
   exact initialized-vs-from_params control. Optional nondefault step fixture
   should preserve current from_params behavior unchanged.
4. Full AEC suite, strict all-target/all-feature Clippy, and existing cold process,
   drain/reset allocation AND deallocation checks. Construction is control-side;
   the corrected call adds no realtime work or storage.

## Compatibility judgment

This audibly changes direct `new(rate)` processing to honor its advertised
canonical defaults and requested clock. At48k it removes hidden0.7 learning in
favor of reported0.5; away from48k it also restores intended timing. Existing
from_params and normally initialized factory/host paths should remain unchanged.
Users relying on the old direct constructor's hidden learning behavior will get
a different convergence history; explicit0.7 remains available through normal
configuration, while the incorrect rate scaling should not be preserved as a
preset feature. This is an existing defect revealed during AUD100 review, not
an introduced regression in that guard.
