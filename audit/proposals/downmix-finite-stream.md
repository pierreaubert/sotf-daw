# Downmix finite-stream follow-up

2026-09-28. Implementation proposal after the verified AUD081 startup fix.
No production changes in this proposal. MIDI and IAMF are excluded.

## Scope and support proof

Own only `sotf-plugin-downmix` source, tests and documentation. Keep the existing
2048-frame spectral delay, 1024-frame hop, coefficients, phase coherence and Lt/Rt
math. No host or routing changes.

The last input-containing window starts at `floor((S - 1) / 1024) * 1024`.
For nonempty source length S, its last synthesized sample is at that start plus
2048 delay plus 2047 window support. Thus the remaining conservative output count
is `3072 + ((1024 - S % 1024) % 1024)`; uniform native tail bound is 4095 frames.
This describes implemented window support, including trailing zeros.

Finite support is eligible if the layout has no LFE, Lt/Rt unconditionally skips
LFE, or all LFE left/right smoothers have both current and target exactly zero.
An ITU target alone does not establish eligibility during its fade. Retained
pre-switch OLA and analysis history are covered by the full transform bound once
future recursive LFE input is unobservable. The coefficient and energy-weighted
phase sums both multiply those LFE gains, so a zero gain removes both pathways.
Simple mode under the same eligibility condition has finite zero support.

Observable LFE uses two recursive lowpasses. Report an infinite tail and preserve
the existing immediate default completion for that unsupported case, without
latching or discarding state. Explicit offline duration remains the available
policy for rendering a chosen recursive interval.

## Implementation shape

- Add scalar accepted-input flag, accepted phase modulo hop, EOS latch and
  remaining output count. Shared stream clearing resets all four. Keep structural
  mode reconstruction and existing reset/initialize behavior.
- Extract the existing processing body into a private kernel that can read
  ordinary input or structural zeros. Do not duplicate the spectral loop or
  allocate scratch during process/drain. Sample-clock scheduling and per-hop
  smoother advancement already make output independent of destination capacity;
  no new output cache is needed.
- A drain call performs at most one hop of zero continuation and returns at most
  `min(destination_frames, 1024, remaining)` stereo frames. Caller storage past
  that prefix stays unchanged. All no-input/complete cases remain idempotent.
- Preflight initialization, matching sample rate, checked sizes and whole stereo
  frames before first latch or any DSP. Invalid calls must preserve output and
  state. Empty source completes without freezing. Nonempty eligible simple mode
  completes and freezes with zero frames.
- After eligible EOS starts, reject new nonempty input and changed/unknown
  controls until reset. Permit exact known scalar snapshots without recomputing
  coefficients; preserve validation for unknown/wrong-type values. Reset or
  successful initialize rearms ordinary input. Unsupported recursive completion
  does not freeze setters or input.
- Finite tail metadata and capacity queries use only scalar/smoother reads and
  allocate nothing. Do not claim generic serial host call bounds before AUD077.

## Required evidence

1. Red default-drain loss followed by independent coefficient-scaled delayed
   dense input and first/final impulses, all 1024 final hop phases, phase/LtRt
   modes, layouts without LFE and selected rates/partitions. Exact total frames.
2. Nontrivial spectral output versus a separately ordinary-zero-continued twin,
   including Lt/Rt surround quadrature and current coefficient transitions;
   continue beyond the structural bound and verify absence of output.
3. Observable LFE remains infinite and unfrozen; settled ITU is finite, a live
   switch toward ITU remains infinite until both gains settle, and Lt/Rt skips
   nonzero recursive LFE while retaining finite prior spectral history.
4. Invalid capacity/rate/pre-init calls preserve output and match an untouched
   twin afterward; reset and same-rate reinitialization match a fresh configured
   instance; repeated completion and same-value snapshots remain stable.
5. Cold first/final drain on a fresh callback thread records zero allocations
   and zero deallocations across simple, phase, Lt/Rt and supported layouts.
6. Full crate tests, strict all-target Clippy and scoped formatting/diff checks.

Source review: `src/lib/downmix_plugin.rs`, particularly `clear_stream_state`,
`compute_coefficients`, `process_simple`, `process_fft_block`, `set_parameter`,
`initialize` and `process`; existing startup tests establish the unchanged clock.

Independent agent review found no eligibility or phase-bound blocker. It checked
both ordinary spectral sums and phase-energy vectors for the zero-LFE proof and
confirmed that the existing structural mode clearing must be preserved. This was
a source review; no finite-drain implementation or new Downmix run is claimed.
