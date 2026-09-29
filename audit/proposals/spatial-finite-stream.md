# AUD073 spatial finite-stream follow-up; AUD080 / AUD081 prerequisites

2026-09-28. Read-only production audit. No repository source/test/dependency edits and no Cargo builds. The small executables in this directory use public factory/Plugin APIs and prebuilt workspace rlibs; exact linker inputs are in `build-command.txt`. MIDI/IAMF and AAE are excluded.

## Executed evidence versus source conclusions

TokenSave status/context/search/read were used first, then current file slices verified the relevant processing paths. The index predates some aggregate-wave edits; none of the three audited production files was modified in this task. Inventory baseline: `/tmp/sotf-finite-stream-inventory.md` and its public probe.

`probe.rs` / `probe.log` execute 108 marker/partition configurations: three families, markers 0/1/73/512/1024/3000, callbacks 1/17/137/512/2048/4096. Each accepts the marker then enough ordinary zeros to reveal its response. This is startup/timing evidence, not a passing drain test: every current public drain still returns COMPLETE with zero frames.

### AUD080 — XTC needs startup and accepted-input correction first

Configuration: stereo, `bypass_xtc_filters=true`, `auto_gain_enabled=false`, N=2048, H=512. This is the public diagnostic identity STFT path; no acoustic transfer-model expectation is involved.

- Marker at frame 0 is completely lost for all tested callback sizes. The periodic Hann analysis window begins at zero, and no negative-time window is supplied.
- A 0.25 marker at frame 73 becomes 0.000025989. Peak index is 2120 with callbacks1, 2113 with17, 1991 with137, 1609 with512, and73 with2048/4096. Declared latency is always2048.
- A later marker at frame3000 has approximately correct amplitude0.25, but peak indices5047/5091/5055/4536/4536/4536 for those same partitions. Thus this is not just one-off startup attenuation.
- `acceptance.rs` / `acceptance.log`: callbacks4096 then137 repeatedly preserve markers4095/4196/4506/4597/4607/4644, but discard markers4608 and4643 exactly. Those are endpoints of a whole36-frame lost interval. The loop can consume101 of137 input frames, fill the requested137 output frames from queued output, exit on `output_pos == num_frames`, then advance `block_start` by137. The remaining36 accepted samples never enter the analysis buffer.

Source: `xtc_plugin.rs:1566` reset starts input_fill0; `:1665-1730` callback-oriented input/output loop; `:1789` appends shortage zeros at callback end; `:1797` latency comment explicitly allows callback-sized compensation error. The public fixed-latency contract cannot be met by that scheduler.

### AUD081 — Downmix startup loss; output scheduling itself is stable

Stereo phase-coherent configuration provides a neutral front-channel oracle (single active left input; no LFE channel).

- Frame0 marker is completely lost.
- Frame73 marker becomes0.003121830; frame512 becomes0.124999985 instead of0.25; frame1024 and3000 are approximately0.25.
- All tested callback partitions produce identical samples and stable2048-frame delay for surviving markers.

Source: `downmix_plugin.rs:284` resets input_fill0; sqrt-Hann analysis and synthesis omit the preceding half-window. `:932-969` already advances input/output per frame and explicitly waits2048 samples. This is the same missing negative-time-window class as earlier Upmixer/Beamformer startup corrections, not a reason to replace its stable output scheduler.

### PND — tested neutral startup is already correct

`correction_strength=0`, stereo: all tested markers preserve amplitude approximately0.25 at exactly marker+2047, bit-identical across tested partitions. Its existing N−H zero prefix and per-sample scheduler must be retained. Source: `phase_vocoder_channel.rs:77`, `pnd_plugin.rs:287-380`, `consts.rs`.

### Cold callback allocation evidence

`allocation.rs` / `allocation.log` use explicit TLS counters around only the first prepared process call on a fresh thread, counting allocations AND frees. Seven configurations all report0/0: XTC N128/2048/16384 with large callbacks, Downmix stereo spectral / six-channel simple / six-channel LtRt, PND neutral stereo. This is bounded evidence, not a universal allocation guarantee for all parameter updates or external files.

XTC and Downmix call realfft `process()`, which creates the backend scratch vector internally. Pinned realfft's selected tested power-of-two plans require no allocated scratch, explaining the passing measurements. Any implementation should explicitly prepare maximum forward/inverse scratch and use `process_with_scratch` if changing planner/backend usage; no heap regression should be inferred solely from the wrapper call name. Existing XTC publication retirement/try-lock ownership must remain intact.

## Proposed first implementation: XTC startup + exact input acceptance only

Own `sotf-plugin-xtc` source and focused tests; no host, generic queue, graph/PDC, native wrapper, factory or oversampling changes.

1. Replace callback-output-driven control flow with a per-input-frame clock (or an equivalent bounded chunk loop that provably consumes every input frame). Each accepted stereo input frame advances the STFT input and output clocks exactly once. A pending output backlog never terminates input consumption.
2. Prefix P=N−H zero history so the first programme sample participates in every relevant Hann/WOLA window. The first three analysis windows begin at −3H, −2H and −H; treat their negative synthesis indices explicitly.
3. Preserve declared D=N. Schedule synthesis frame for analysis start w at output time D+w. Emit exactly D startup zeros; discard negative-time synthesis contributions and never accumulate unobservable prefix cells that could reappear after circular wrap. Clear skipped cells/counters explicitly.
4. At each sample, process a completed analysis window before trying to emit that sample's scheduled output. Starting the first real output at N leaves all overlapping contributions for that sample available. Each input callback returns exactly its accepted frame count. Output beyond the caller's declared region is untouched, subject to the existing exact-length API.
5. Shared scalar counters track input phase, synthesis placement/read phase, startup delay and available output. Reset and successful initialize restore the same zero-history epoch. Initialization should clear old-rate audio history; validate initialized rate and checked channel products before adopting pending filters or mutating audio state.
6. Keep public hard-disabled direct routing behavior as a separate branch in this first step. Do not silently introduce a delayed bypass or change its declared latency semantics without review. Diagnostic bypass remains inside the STFT and is the independent neutral oracle. Enabled↔disabled transitions can reveal old frozen STFT history today; choose explicit reset-required transition semantics in a separate reviewed lifecycle decision before claiming general finite metadata for that branch.
7. Preserve current filter publication/crossfade ownership. This step does not claim that callback-throttled AutoGain estimation becomes partition invariant: its current measurement cadence is every10 callbacks. Disable AutoGain for exact neutral timing tests; test default processing for finiteness, full acceptance and allocation safety. A later drain should use a canonical fixed internal hop/cache so destination capacity cannot retime AutoGain measurements.

Independent red→green oracle: full dense dyadic stereo waveform plus zeros must match `[N zeros] + programme` within a fixed FFT reconstruction tolerance, at every startup hop phase and after circular-buffer wrap. Include 1/17/137/H/N/4096/8193 callbacks and a mixed4096→137 schedule, impulse endpoints around4608/4643, chunks larger than scratch, all supported N128..16384, selected rates44.1/48/96/192k, reset/reinitialize, input/output canaries, invalid-call state comparison, and cold allocations/frees. This directly tests lost input and declared timing, rather than accepting peak-location tolerance or merely comparing two copies of the same faulty scheduler.

## Downmix prerequisite proposal

Own only `sotf-plugin-downmix` source/tests after separate approval. Keep its D=N=2048 sample scheduler and H=1024. Add H zero analysis history and discard the one negative-time synthesis hop. Ensure skipped OLA cells cannot wrap into later output. Preserve existing coefficients and spectral operations. Neutral front-channel full-waveform and every initial-hop-phase oracles should compare against independent coefficient-scaled delayed programme; surround LtRt uses an independent frequency-domain quadrature oracle. Check all public layouts, reset and rates without changing the recursive LFE policy. No drain is necessary to reproduce/fix the startup defect.

## Exact finite-support derivation after scheduler correction

Let N be transform length, H its fixed hop, D the declared physical neutral-path delay, and S>0 accepted input frames since reset. All three transform kernels synthesize at most N samples for each input window. Pitch ratios in PND change bin placement, not synthesis time/window length. XTC transfer matrices and room/reflection data are multiplied into the current finite spectrum, not a recursive audio convolution state. Downmix phase coherence/LtRt likewise acts on the current spectrum.

The final potentially input-containing window starts at

`w_last = floor((S−1)/H) * H`.

Its final synthesized sample appears at output index

`D + w_last + N − 1`.

Therefore an exact conservative scheduling count after the S emitted frames is

`R(S) = D + (N−H) + ((H−(S % H)) % H)`.

This includes startup padding and complete final-window support once, may include trailing zeros, and is not a claim that all settings have a nonzero last tap. A uniform native bound measured from the final input is `D + N − 1`. Use checked integer arithmetic and retain the accepted input phase modulo H, avoiding unbounded absolute sample counters.

| Family / eligible path | N / H / D | Uniform bound | Phase-dependent drain count |
| --- | --- | --- | --- |
| PND, all finite valid correction/formant states | 2048 /512 /2047 |4094 |3583 + padding to next512 boundary |
| XTC enabled after corrected scheduler | N /N÷4 /N |2N−1 |2N−N÷4 + padding to next hop |
| Downmix spectral, LFE provably unobservable |2048 /1024 /2048 |4095 |3072 + padding to next1024 boundary |
| Downmix simple, LFE provably unobservable | no transform |0 |0 |

PND already uses the D=N−1 relationship above: the first prefilled transform is processed at input H−1 and its synthesis index0 begins emitting that same call. This formula preserves its existing2047 latency, rather than applying XTC's corrected N convention to it.

Current XTC has no callback-independent finite timeline: do not install the proposed metadata/count before correcting its scheduler and input acceptance. Current Downmix's missing prefix still obeys its finite support bound, but draining alone would preserve an already attenuated beginning; fix the prefix first.

## Eligible Downmix states and transition proof

LFE input is filtered by two recursive lowpasses before either processing path. Thus ordinary layouts with an observable LFE gain have Infinite response, even if the most recent input happened not to use LFE. Minimum allowed lfe_gain_db=−60 is not zero.

Finite support is provable when:

- The fixed speaker layout has no LFE channel; or
- LtRt mode discards LFE unconditionally in spectral mixing; or
- Every LFE coefficient's current smoothed left/right values AND targets are exactly zero (for example settled ITU mode).

A target-only switch into ITU mode is insufficient during its fade. More subtly, already accumulated spectral output may contain pre-switch LFE energy, but that retained OLA contribution is finite and covered by the full transform bound. Future filtered LFE is unobservable once both current/target gains are exactly zero and controls are frozen. LFE signal remains in the analysis window, but has no output contribution under the zero coefficients. Recursive filter state itself can remain nonzero without invalidating that observation proof.

For unsupported observable-LFE states, preserve Infinite metadata and the agreed legacy COMPLETE/no-tail behavior without freezing; explicit ordinary-zero rendering via the new offline duration API handles a chosen duration. No arbitrary epsilon/feedback cutoff. Simple mode with no observable LFE legitimately has Finite(0), not an invented transform tail.

## PND drain state policy

At the first valid nonempty drain, latch the effective pitch ratio and formant controls. Stop feeding synthetic zero samples to pitch/drift estimators or changing their learned correction state. Continue the phase-vocoder's finite analysis/synthesis bookkeeping, onset/phase transport and OLA using that fixed effective ratio. Do not reset phases or envelopes at EOS. This makes EOS a continuation of accepted programme under its last learned correction, with bounded response independent of analyzer convergence length.

This frozen-estimator policy intentionally differs from ordinary `process(zeros)`, whose confidence/reference logic may move the correction ratio toward unity. A test must not call that adaptive continuation an independent frozen-policy oracle. Use an analytic unity delayed-waveform oracle for all hop phases, plus a separately fixed-ratio continuation fixture for a nonunity learned state, check unchanged estimator/ratio snapshots across drain, and verify that shifted/formant frame output is zero beyond the structural bound. Existing phase-vocoder tone/onset tests remain relevant but are not EOS proof on their own.

## XTC EOS state policy after prerequisites

Freeze pending filter adoption at accepted EOS; allow an already active old/new crossfade to finish using its retained snapshots. No asynchronous late publication may change a rendered tail's waveform or output width. Keep the existing retirement mailbox ownership so no final Arc or file/filter resource is destroyed in callback/drain. Pending publications remain deferred until a reset/new epoch; reset should invalidate old-generation pending work or document deterministic adoption ordering rather than admit stale old-epoch work accidentally.

All filter matrices map a zero spectrum to zero, and AutoGain/limiter states multiply current audio; they do not extend audio support beyond the transform bound. A long head-filter fade therefore does not require additional silent tail solely to reach fade completion. Room/HRTF filter files are spectral preparation inputs here; this bound describes the implemented finite WOLA support, not a claim of exact full-length acoustic impulse convolution.

Because AutoGain currently measures per callback, process drain zeros in fixed hop-sized internal blocks into a prepared output cache and serve arbitrary caller capacities from that cache. Freeze the total R(S) budget once and ensure partial last processing does not overpublish samples. This avoids output changing with drain destination size. The ordinary callback-cadence AutoGain limitation remains separate.

## Shared proposed drain lifecycle and tests

No shared host hooks are needed: all three implement Plugin directly.

- Prepare zero input and output scratch on setup: stereo input / dynamic speaker width for XTC, channel-count input / stereo output for Downmix, equal width for PND. Maximum internal work one hop per refill; caller capacity can be any positive whole output frame.
- Preflight initialization/sample rate, checked products, exact destination channel shape and positive capacity before EOS latch, pending-publication adoption or output mutation. Capacity errors preserve output and DSP/history.
- Empty stream completes without freezing. Valid nonempty supported EOS rejects changed controls/new input until reset; same-value snapshots remain no-op. Repeated completion is idempotent. Unsupported recursive Downmix states retain legacy behavior.
- Explicit counters for accepted source phase, remaining finite response and cached output. Tail metadata is scalar and allocation-free, never reads an async ArcSwap/publication container.
- Independent first/final/dense neutral programme oracles; sparse transfer matrix or direct small DFT/WOLA oracle for nontrivial XTC/LtRt; programme lengths1/H−1/H/H+1/N−1/N/N+1 and final markers at every hop phase; capacities1/irregular/large; exact length, no duplicated suffix, zero after bound; reset and invalid-call retry comparison.
- Cover pending filter completion/crossfade retirement with callbacks cold on fresh threads, counting both allocation and deallocation. PND tests must distinguish frozen correction from adaptive zero continuation. Downmix includes settled-zero vs transitioning LFE gains and simple Finite(0).

## Requested scope order

1. Review/implement AUD080 XTC startup plus accepted-input correction; prove full neutral waveform and timing first.
2. Review/implement AUD081 Downmix negative-time window; preserve D=2048.
3. Add finite drain/metadata separately for PND, corrected XTC and eligible Downmix with the explicit state policies above.

No implementation has started. The new parent-owned explicit offline tail API has19/19 focused offline tests passing, according to parent; this audit did not execute its engine tests.
