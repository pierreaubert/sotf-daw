# AUD077 independent review

2026-09-28. Root reviewed wrapper preparation, native output-capacity composition,
Resampler work-bound arithmetic and the pinned backend planner. Dynamics reviewed
the host cursor/quota lifecycle and engine removal of the global call limit.
Plugin-chain review covered the eleven native scalar bounds and Binaural/Upmixer.

## Wrapper and Resampler conclusions

No remaining blocker found. Wrapper preparation retains all generated output,
uses existing prepared storage, finishes at most two input/up-filter chunks, and
prepares the child before querying its current-state work bound. Repeated begin
is idempotent; partial DSP failure requires reset. The composed allowance counts
queued output, child calls, cached transfers and down-filter completion separately.
The trait explicitly forbids preparation from increasing preflighted output
capacity or changing channels/rate.

Review did expose an integration mistake in the new Downmix capacity query:
it advertised zero before input, although oversampling preparation could later
create a 1,024-frame native tail. Minimum-capacity 2x wrapping reproduced the
error. Downmix now advertises its structural hop capacity during setup. Actual
2x/4x regressions verify complete audible tails within the composed work bound,
with zero allocations and deallocations on a fresh callback thread. The complete
Downmix suite passes 72 tests and strict all-target/all-feature Clippy.
See [Downmix finite stream](downmix-finite-stream.md).

The Resampler proof uses the backend's exact next-block classification, finishes
one possible ramp, then bounds constant-step work using source endpoint distance
or the remaining fixed-clock output count. Integer source clocks are subtracted
before conversion to floating point. The pinned backend permits anchors at or
below its block boundary; the query's outward padding covers equality. The
648-case public matrix includes extreme rates, small chunks, ramps and partial
drainage. Its maximum observed bound/actual ratio is 5.75. This is scheduling
metadata; it does not add a new audio-accuracy claim.

## Host/engine conclusions and separate finding

Dynamics found no introduced blocker in scalar cursor/quota ordering. Successful
native calls are charged before downstream processing, including zero-output
calls. Native errors do not charge; completed prefixes are skipped while rate
metadata propagates. Accepted active-stage controls refresh only that stage's
allowance. Real topology/bypass changes, reset and new accepted input rearm the
host; unchanged rebuild/bypass operations do not.

The engine still has a preexisting same-format replacement boundary defect:
a replacement committed while an old terminal tail frame or old EOS is blocked
can retain the old completion and skip the replacement's drainage. The existing
replacement-near-4,096 fixture switches on a nonterminal old result and does not
prove these terminal cases. A separate deterministic reproduction and worker-only
proposal are being prepared. No broader manager transition or branch-queue
proposal is included in AUD077.

Detailed read-only host review: `/tmp/sotf-aud077-host-independent-review.md`.
Root review did not rerun unchanged spectral tests. The workspace integration
checkpoint is recorded separately in AUDIT.md.
