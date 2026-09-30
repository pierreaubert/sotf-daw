# AUD140 Astra design review

Status: **bounded serial channel-changing EOF implementation ACCEPTED**.

## Implementation acceptance

Reviewed the four frozen tested files under
`crates/sotf-plugins/target/audit-baselines/aud140-implemented-final/source/`,
not subsequent shared-host edits. Verified every saved file against manifest
aggregate `002b6a4e6f8a52c02780cc3f9a351ffc532c7b3d787f30de332f1d4773d1b244`.
The inspected host SHA is
`9b7bccab1760b536cb609b83709c648c995872f29ae804a472d0a52d1033bed8`;
report SHA is `f631f1076ce26ead35b9be5e728abc3b8a1fd23e5f6483f1cbc6153f0adcac6a`.

The separate drain validator preserves ordinary equal-width eligibility and
requires a single adjacent audio chain without maps, offsets or sidechains.
Active nodes require explicit identity-frame capability, host-rate input/output,
contiguous widths and finite tail metadata. Width-changing bypass is refused.
Ambisonics' successful single/dual-band process branches return the input frame
count; its capability does not override the Unknown dual-band tail rejection.

The new route checks caller alignment/capacity and checked source/intermediate
sample extents against both prepared scratch buffers before any begin/drain.
The causal walk remains stage ordered. The two-producer regression explicitly
asserts downstream process calls precede its begin/drain, and checks the full
four-frame suffix against two independent f64 FIR passes surrounding a separate
production decoder. That is composition evidence; AUD133 supplies independent
decoder mathematics. Expansion/contraction refusals preserve caller sentinels
and zero begin/drain counters. Exact-capacity retries, bypass cases and
identity/rate/width/Unknown-tail refusals cover the accepted design gates.

Verified all seven reported log hashes and terminal results: 13 integration
tests passed / 3 manual utilities ignored; explicit saved ordinary-audio replay
1 passed; host library 551 passed / 1 ignored; Ambisonics library 59 passed;
host, facade and Ambisonics all-target strict Clippy completed successfully.
The final integration helper-only formatting delta from the earlier host lint
snapshot is correctly disclosed. No new broad workspace or generic heap/WCET
claim follows from these gates. No reviewer Cargo or production edits.

Acceptance covers this finite, identity-frame, same-rate serial nonzero-output
route. Existing queued-control application and post-advance error semantics are
unchanged and are not transactional guarantees. Unknown recursive tails,
branches, unequal rates, terminal sinks, native consuming-host integration and
manager protocols remain outside this acceptance; AUD138 stays separate.

## Historical design review

Reviewed proposal SHA
`97dc22070325df23f46fa475b7a874a0934c9ffd4b94a7db8602b8a0695a2c3a`
and report `a24339c87881acd788aa01970accb7dc9b70bb11903dfaf84e65ef3cf337cb1b`.
The public zero-tail Ambisonics and finite FIR-to-Ambisonics failures identify
the overly restrictive shared eligibility guard. The independent f64 FIR tail
and retained-marker recovery are meaningful; downstream production Ambisonics
is a separate execution oracle, not independent decoder mathematics. AUD133
owns that decoder-accuracy evidence.

## Accepted bounded approach

- Add drain-only serial eligibility; preserve the equal-width helper and
  ordinary f32 fast-path selection.
- Validate graph/edge structure, adjacent active widths and bypass invariants,
  explicit per-node identity capability and negotiated same-rate geometry.
- Admit finite tails only for the new width-changing route. Source inspection
  supports Ambisonics frame identity for all successful process modes, including
  dual-band, but dual-band's Unknown tail remains excluded from this route.
- Validate caller geometry and every intermediate prepared extent before any
  producer begin/drain mutation. Provision during build or refuse; no drain
  allocation. Preserve the existing causal stage-by-stage drain order.
- Keep queued control application and post-advance plugin errors qualified as
  existing host behavior, not new all-operation transactionality guarantees.

## Implementation acceptance gates

In addition to the proposal's exact marker/count, rejection, bypass and baseline
replay tests, cover both expansion and contraction scratch extents, and two
finite producers separated by a width-changing stage. The admitted all-finite
serial scope needs evidence that earlier tails reach downstream state before
the latter is finalized. Verify all inadequate intermediate capacities reject
before either producer begins. Include unknown-tail rejection (dual-band) while
ordinary processing and existing equal-width EOF remain unchanged.
Exercise successful bypass of a width-preserving intermediate stage and
transactional refusal to bypass a width-changing stage, retaining ordinary
waveform and pending-tail evidence in both cases.

Pre-edit ordinary and refusal/recovery captures are preserved before production
work. Their scope is correctly limited; no current implementation acceptance is
inferred. Shared host edits must coordinate with AUD138's separate sink path;
no competing geometry flags, generic unequal-rate/branch queues or manager
protocol changes are authorized. No reviewer Cargo or production edits.
