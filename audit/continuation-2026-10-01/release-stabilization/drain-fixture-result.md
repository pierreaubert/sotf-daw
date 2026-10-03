# Drain fixture result (AUD140)

## Cause
`drain_preflight_rejects_expansion_and_contraction_scratch_extents` assumed graph scratch stays fixed at `8192*32`. `DawHost::build` instead calls `rebuild_graph_drain_plan` and preallocates emission scratch to the declared drain bound, so the oversized declaration fits: diagnostic `input 174764/prepared 262144`, `output 262146/prepared 262146`. Rejection expectation was invalid; reproduced serially, not a stale binary.

## Change
Test-only repair in `crates/sotf-plugins/tests/aud140_channel_changing_eof.rs`:
- Added shared `AtomicUsize` capacity control to `FiniteFirProducer`; default fixtures unchanged.
- Helper builds and processes at fitting capacity `1`, then stores an explicit monotone larger stale bound and asserts the live host bound matches it.
- Preserved exact `scratch` rejection, no-begin, no-drain, and unchanged-output assertions for both expansion (4→6) and contraction (64→16).
- No production host allocation/preparation change.

## Verification
Root runs focused 16 tests and lint; this worker had shell disabled and did not execute them.
