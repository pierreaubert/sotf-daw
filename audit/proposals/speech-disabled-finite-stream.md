# SpeechDenoiser disabled finite stream

2026-09-28. Proposed AUD073 extension; no source changes yet.

The corrected RNNoise wrapper delays raw dry audio by 960 frames. Its enabled
wet path retains recursive high-pass/model history and has no established finite
support. The disabled route, however, uses a bounded output crossfade followed
by pure delayed dry audio while continuing to advance the model.

## Bounded scope

Implement native drain only when the accepted EOF configuration is disabled.
Keep enabled wet tail metadata Unknown and its existing immediate completion
behavior until a separate render policy is defined. No backend algorithm,
latency, model or configuration change is proposed.

When EOF selects disabled, every subsequent zero-input processing call sets the
bypass target to one. Starting anywhere in [0,1], the existing linear bypass
increment reaches exactly one well before 960 frames (nominal 480 steps; account
for f32 accumulation). After that, wet model history is inaudible. The dry ring
is an exact 960-frame delay, so exactly 960 zero-continuation frames cover all
observable response, including an interrupted enabled/disabled transition.
This proof depends on freezing the enabled control until reset after accepted
nonempty finite EOS. It does not claim the model's internal audio becomes zero.

Add plugin-only `has_input` and optional remaining-tail state. Disabled metadata
is Finite(960), initialized enabled metadata Unknown. Advertise structural native
capacity 480 before input and in completed state so prepared wrappers can size
correctly. Bound full-capacity calls by ceil(remaining/480), minimum one; enabled
unsupported paths return one successful terminal call without a finite claim.

Preflight initialized rate, whole output-channel frames, checked dimensions and
positive destination space for positive tails before latching or touching output.
Empty-stream completion stays unfrozen. Repeated completion writes no output.
For each call, clear only the prefix min(capacity,480,remaining), run the existing
backend with disabled=true, publish diagnostics through the existing prepared
cache, and return only that prefix. The backend is already sample-clock invariant;
no new cache or full model copy is required. No allocations, deallocations or
blocking waits may be introduced.

Reject changed enabled controls and nonempty ordinary process after accepted
finite EOF; allow recognized same-value snapshots without allocations. Apply
this through both scalar and borrowed/owned parameter-map routes. Reset and
successful initialize rearm the stream; failed initialization preserves the
existing epoch. Native enabled EOS behavior remains unfrozen for compatibility.

## Evidence required

- Red independent disabled final-marker test against the current default drain.
- Exact 960-frame delayed source, startup zeros and complete final marker for
  mono/stereo, all 480 model-frame phases and irregular/oversized callbacks.
- Identical-history ordinary zero-continuation twins for enabled→disabled,
  disabled→enabled→disabled, changes immediately before EOF and partial fades.
  Compare complete returned waveform bit-for-bit over varied drain capacities;
  verify further disabled zeros have exactly zero audible output.
- Empty/complete/reset/reinitialize, invalid rate/layout/capacity replay, output
  canaries, accepted-control freeze and repeated snapshot behavior.
- Fresh callback thread measures zero allocations and zero deallocations for
  first drain, query, completion, same-value controls and reset. Existing model
  and enabled wet waveform tests remain unchanged.
- Complete backend/Speech crate tests and strict scoped Clippy, followed by
  independent review and the next workspace integration checkpoint.
