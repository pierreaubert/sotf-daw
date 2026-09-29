# AUD115 — causal shared multichannel AutoGain clock

Status: read-only proposal; no production/test changes or new probes run for this review. MIDI/IAMF, host queues, and engine scheduling are excluded. TokenSave status/context and current source were inspected; the index was rebuilding, so relevant live source was also checked. Existing Rust/DSP/workflow skill requirements apply before implementation.

## Reproduced defect and provenance

Root's public bridge probe is `/tmp/sotf-spatial-autogain-clock-probe.rs`; results are `/tmp/sotf-spatial-autogain-clock-probe.log`, hashes `/tmp/sotf-spatial-autogain-clock-manifest.json`. Source hash: `93321c341878e8b6956e59d8dc747e4ba4f71d7450db761443da32d7874f7619`. It constructs through `plugins_bridge::create_plugin`, initializes at 48 kHz, and checks finite full-frame output. Default configurations, AutoGain 100 ms / max 12 dB, programme length 6 seconds + 17 frames:

| Plugin | AG off, callback 137 vs 512 / 8192 | AG on, 137 vs 512 maximum error | AG on, 137 vs 8192 maximum error | Same input prefix, different future: prefix error |
|---|---|---:|---:|---:|
| AAE | Exactly equal | 0.00013354793 | 0.0028697848 | 0.000041787513 |
| Upmixer | Exactly equal | 0.001336515 | 0.017378747 | 0.00024104957 |

The prefix probe first warms 109 callbacks of 4096 frames. Its next 8192-frame inputs agree for 4096 frames, then differ by left ×0.1 / right ×10. AG-off prefixes are exact; AG-on differences begin at output sample 30 for both plugins. This demonstrates dependence on later input within the current callback, beyond a mere choice of smoothing speed.

`host/src/multichannel_auto_gain.rs::measure_input` ingests and refreshes the entire input block. `measure_and_apply` folds/ingests/refreshes the entire output block, setting a new target before applying gains to the first output frame. Both callers use those two operations around their raw renderer. The accurate AUD110 scalar recurrence is not the cause and should remain unchanged.

## Minimal shared helper addition

Add an explicitly paired method, tentatively:

```
measure_aligned_and_apply(
    aligned_stereo_input, output, num_frames, actual_output_channels, speaker_config
)
```

The input contract is exactly two samples per output frame, already aligned to that frame's transport delay by the caller. This method owns a relative sample-clock phase and a refresh interval `max(1, sample_rate / 10)` frames. This is a fixed 100 ms interval at the common supported rates, with integer-floor timing documented for other rates; no callback counter or absolute transport position is required.

For each segment bounded by the next refresh boundary and the prepared 8192-frame scratch capacity:

1. Ingest the aligned input segment without refreshing its loudness value.
2. Fold the corresponding **uncompensated** output segment and ingest it without refreshing.
3. Apply `next_gain_linear()` exactly once per output frame, multiplying all actual output channels by that scalar.
4. Advance the sample phase. At the boundary, refresh input then output measurements. The resulting target first affects the **next** output frame.

For output index n, gain g[n] therefore uses measurements ending strictly before n. Ingesting the whole bounded segment before its gain loop does not violate this rule: gain advancement cannot see those meter updates until the explicit refresh after the segment. No refresh occurs merely because a callback or ring span ended. Preserve the existing AUD110 f64 state, exact scalar recurrence, dB snap, attack/release, and conversion cache.

Validate checked input/output products and exact shapes before advancing meter/phase/gain state or modifying output. Zero-frame calls are no-ops. This is a shape-error guarantee, not a new claim that arbitrary underlying meter errors roll back an already processed stream. Caller validation and prepared supported configurations must make valid-path meter ingestion infallible in practice; retain explicit error propagation.

### Folding and compatibility

Preserve the current folding equations and f32 accumulation order:

- Actual two-channel output is metered directly; mono duplicates into stereo.
- For wider output, iterate speakers in existing metadata order; skip LFE and indices outside actual width.
- Azimuth > +10° routes left, < −10° right; center uses the existing f32 `FRAC_1_SQRT_2` contribution to each side.
- Apply the resulting gain to every actual channel, including LFE. LFE exclusion is only a meter rule.
- A two-channel binaural preview stays direct stereo even when its retained speaker metadata describes a wider layout.

Keep the existing public `measure_input` and `measure_and_apply` methods and their externally observable refresh schedule. Document that causal paired timing requires callers to use the paired method throughout a metering epoch. Do not silently reset or reject mixed use of older public methods.

The current folder uses `resize` above 8192 frames (and its debug capacity assertion fails first). The new method must use bounded spans and never resize. Recommended small companion change: make the legacy output method ingest successive bounded folded spans, refresh **once after its original whole callback**, then run its existing gain loop. That retains its old block timing while removing the existing oversized-callback allocation/panic. Verify identical legacy output for normal blocks and full-capacity/oversized blocks. This companion change needs explicit inclusion in implementation scope rather than accidentally changing the legacy method to the new timing.

## AAE integration

Relevant source: `sotf-plugin-aae/src/lib/aae_plugin.rs`, `ensure_auto_gain` near 499, `apply_auto_gain` near 514, initialize/reset near 825/907, process near 933 and final stages near 1111. Its constructor already prepares AutoGain even when disabled through its existing helper.

Remove the up-front whole-callback `measure_input` call. After the unchanged raw per-sample renderer, call the paired helper using the original stereo input and full raw output. The reference delay is zero, matching `latency_samples() == 0`. Reverb/pre-delay/ER/FDN response is effect behavior, not a global scheduler delay to remove from the loudness comparison.

Keep the exact stage order:

1. Existing direct/ER/FDN/LFE renderer and denormal handling.
2. Paired AutoGain.
3. Existing linked output safety limiter and denormal handling.
4. Existing audible bypass crossfade.

FDN injection uses the raw ER average and its own internal histories, not this final compensated output. The proposed pass does not enter the feedback path. Audible bypass currently keeps the wet renderer and AutoGain warm; preserve that policy. Do not split the raw DSP loop into smaller plugin process calls.

AAE remains a recursive-tail plugin with its existing native drain policy. Ordinary explicit zero continuation naturally advances the paired AutoGain clock. This work adds no finite-tail declaration or AAE native drain implementation.

## Upmixer integration and reference timeline

Relevant source: `sotf-plugin-upmixer/src/lib/upmixer_plugin.rs`: constructor 508, helper 1085, from_params 1454, resize_fft 1608, initialize/reset 1936/2088, process 2214, latency 2249, process_stream 2259, direct bypass 2389, final AG pass 2646; `src/drain.rs`; `src/output.rs::apply_final_safety_cap`.

The normal renderer emits fixed N-frame transport latency; direct `bypass_all_processing` reports zero. Add a prepared stereo reference ring with N frames (2N f32 values), reset to zero, independent of the STFT input buffer. Its memory cost is 8N bytes, e.g. 16 KiB for N=2048. It must resize only with prepared FFT geometry, never in process.

Run the existing raw renderer once using its original callback size. Then walk original input and emitted output in spans ending at reference-ring wrap:

- Before overwrite, the ring span contains x[n−N], with zero prehistory.
- Pass that span and the raw output span to the paired helper.
- Copy x[n] into the consumed ring span and advance it.

Do not write a whole callback into the ring first: callbacks longer than N would destroy the reference still needed for earlier output. The ring must advance even while AutoGain is disabled so a later enable can compare the correct current output with its delayed input. This adds bounded copy cost while disabled, but must leave disabled samples bit-identical.

For direct bypass use current input, with no delay. Preserve its existing reset on a real bypass mode change; clear reference and metering clocks through that reset. Preserve preview/layout folding by supplying actual output width.

### Accepted/emitted frames

The normal prepared scheduler should emit one frame per accepted frame: H=N/2, N startup zeros, half-window prefix and negative-window discard provide enough synthesized frames after startup. The intended integration relies on that existing property and must test it for 1-frame, mixed, and oversized callbacks and every supported FFT geometry. Do not silently advance the reference by un-emitted frames if a legitimate partial-output route is found. Report that case before implementation expansion; it would need a separate bounded reference-retention design.

Do not subdivide raw rendering to make AutoGain causal. Upmixer currently advances several parameter smoothers and HR envelope state by callback. Changing that schedule would alter AG-off audio and widen this task.

### EOS

`UpmixerDrain` generates canonical blocks using `process_stream` with `min(remaining, H)` zero-input frames, caches them, then serves arbitrary caller capacities. Place the paired pass inside `process_stream` so reference, meter, and gain clocks advance once per generated frame. Serving a partially consumed cache must not advance any of those clocks again. The final short canonical block advances only its real generated frame count.

The ring first supplies the last real programme reference, then zeros. Continue the existing adaptive AutoGain behavior during zero continuation; do not introduce a frozen-gain drain policy. The helper is multiplicative, so it cannot create audio after the renderer's existing finite support or extend existing drain quotas. Compare with an ordinary zero-continuation oracle using the same absolute measurement clock, not a claimed frozen-gain reference.

### Separate existing output-cap limitation

`output.rs::apply_final_safety_cap` obtains the maximum over the entire emitted callback and advances its release once per callback, after AutoGain. When active it can independently change an early frame based on a later peak and break callback partition equivalence. Preserve that source for AUD115; qualify the new causality claim as the AutoGain stage, with public end-to-end exact tests using an inactive cap. Include active-cap ceiling checks, but do not promise final-output partition invariance in that configuration. Other raw callback-sized smoothing limitations likewise remain explicit.

## Lifecycle and preparation choices requiring agreement

| Boundary | Proposed behavior / recommendation | Existing behavior to preserve or explicitly change |
|---|---|---|
| No frames | No ring, phase, meter, or gain change | Preserve true no-op |
| Reset | Clear paired phase, reference ring, meters and gain; preserve configured controls | Existing caller resets already reset gain |
| Rate/FFT reinitialization | Prepare interval/ring for the new rate/geometry; reset relative phase/reference | Upmixer already calls reset; AAE `set_sample_rate` clears meters but currently preserves AutoGain gain/target. Retain that AAE gain behavior unless a separate fresh-gain initialization change is approved |
| AG enabled -> disabled | Unity application; reference ring continues | Existing meter/gain state is paused except the existing set_enabled target behavior |
| AG disabled -> enabled | **Recommended explicit choice:** start a fresh meter/phase/gain epoch at the first enabled frame, retaining raw DSP/reference history; same-value snapshots do nothing | This changes the old paused-history resume behavior. If compatibility is preferred, retain paused meter/gain/phase instead, with a documented active-frame clock. Both are causal, but the implementation/tests must choose one |
| Max gain / smoothing changes | Preserve phase and ring; existing continuous scalar semantics | No new target clamp timing or smoothing redesign |
| Same-rate valid control snapshots | Must not reset interval/history or allocate | Explicit regression |
| EOS | Existing supported drain advances through canonical generated zeros; completed calls do nothing | No new AAE drain or Upmixer support policy |

Preparation is also necessary for the real-time claim: Upmixer currently stores `auto_gain: None` when initially disabled, and its first enable can construct monitors. Prepare the helper during control-side construction/initialization even when disabled. Synchronize enabled/max/smoothing values after `from_params` writes them; merely preparing a default disabled helper in `new` would cause from_params(enabled=true) to retain a disabled inner helper. Preserve valid direct-constructor processing, public constructor signatures, and existing rate-error behavior. AAE already prepares normally, but retains its defensive ensure path. Tests must prove valid production callbacks do not enter that path.

The recommended scope is shared helper + both callers + narrowly required private prepared reference state; no public parameter IDs, schema, renderer geometry, host/native wiring, or engine protocol changes. The enable-epoch choice, AAE reinitialize gain preservation, and legacy oversized-method companion change are the only policy decisions needed before coding.

## Permanent independent regression plan

1. Capture public factory-route partition and same-prefix failures before editing. Preserve the AG-off exact negative controls and root's source signal. Check first differing sample and maximum error, not only RMS.
2. Shared helper: independent manual stereo fold for mono/stereo/multichannel, center boundaries, LFE, invalid metadata indices, and two-channel preview. Check unchanged accumulation order and all-channel application. Compare paired output against an independent fixed-boundary driver built from the existing scalar AutoGain ingest/refresh/next-gain primitives; do not use the new paired method as its own oracle. Existing AUD110 f64 analytic scalar tests remain authoritative for gain recurrence.
3. Known-gain steady inputs: independent level-ratio expectation; delayed neutral identity/reference gives zero dB target after aligned warmup. Include a wrong-delay negative control demonstrating the oracle can distinguish startup and programme-level transitions.
4. Shapes/lifecycle: invalid checked dimensions leave destination and helper state unchanged; empty calls; reset/fresh equivalence; same-value controls; agreed enable policy; max/smoothing changes at fixed absolute frame positions. Use 44.1/48/96/192 kHz and intervals−1/interval/interval+1, callbacks 1/17/137/512/8192/8193/32769.
5. AAE: original AG-off waveform preserved; dry/direct and wet configurations, audible bypass, low-level limiter-inactive exact causality; active limiter preserves ceiling and stage order. Prove raw feedback histories do not depend on final AG (private state comparison or matching later AG-off raw output after identical raw histories).
6. Upmixer: all supported FFT sizes, latency modes, HR on/off, representative layouts and preview; delayed first/final impulses, ring wrap, callbacks>N, direct-bypass zero-delay/reset. Keep raw baseline qualification for modes with unrelated callback-sized smoothing.
7. Upmixer EOS: input ending around measurement and FFT phase boundaries, native capacities 1/partial/full, cached-read delivery, tiny and empty programmes, completion/reset and invalid-preflight controls. Compare all emitted samples with canonical ordinary-zero continuation; unchanged finite counts/bounds.
8. Cold allocations **and frees**: prepare on control thread, run on a fresh callback thread, cross actual first and second loudness refreshes, wrap the delay ring, use oversized >8192 callbacks and 192 kHz intervals (19,200 frames), then reset and EOS cache paths. Measure enabled and disabled processing and a prepared enable transition. Do not infer deallocation safety from an allocation-only helper.
9. Full host/AAE/Upmixer suites and strict all-target Clippy. CPU measurements should distinguish steady enabled cost, 10 Hz loudness refresh cost, and disabled reference-ring copy overhead. No claim of zero additional disabled cost.

## Expected user-visible result and limits

With the same controls applied at the same sample positions, AutoGain follows the same trajectory across callback partitions and cannot use later samples of a callback to change its earlier samples. Upmixer compares output with its actual N-frame-delayed source. Fold weighting, the shared accurate smoothing law, gain limits, raw DSP, and final safety stages remain as specified above. Automatic gain behavior changes audibly from the defective whole-callback target jumps. Active Upmixer callback-wide safety limiting and unrelated callback-sized parameter smoothing remain separate known constraints; this proposal does not claim universal partition invariance for those paths.

## Approved implementation decisions

Root approved AUD115 implementation after review. Preserve paused meter/gain/phase across disable/enable: the paired interval is an active-frame clock; the existing inner `set_enabled` target behavior remains unchanged. Upmixer's source-reference ring advances while disabled. Preserve AAE initialization's existing gain history through `set_sample_rate`, resetting the new relative measurement phase with the recreated meters. Include bounded legacy output folding/ingestion with exactly one whole-callback refresh before its original gain loop. Prepare Upmixer's helper even when disabled and synchronize constructor JSON settings. Preserve raw callback schedules, fold order, feedback and limiter placement, canonical Upmixer EOS, and the active safety-cap qualifications above. No host queue or engine changes.


Peak metadata verification: the pinned vendored `math-dsp::EbuR128` accumulates sample peaks across ingestions until `prev_sample_peak(&mut self)` snapshots/resets them. Therefore the existing AutoGain refresh already reports the complete interval/callback; no local peak accumulator is needed. Independent early-marker/quiet-interval regressions will verify this with bounded ingestion. The provisional local-cache change was removed after checking the actual pinned dependency.
