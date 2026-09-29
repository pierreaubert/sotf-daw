# AUD131 independent design review

Status: causalization policy aligned; implementation approval awaits explicit
provenance/numerical refinements and pre-edit rendered baseline capture.
Validator: Astra medium; no production edits or Cargo run.

Official SOFA FIR specification confirms double-valued additional delays in
samples and IR/MR dimensions:
https://sofacoustics.org/mediawiki/index.php/SOFA_specifications . The proposed
negative-delay support is an explicit SOTF capability/policy, not a claim that
every convention requires signed values. A dataset-wide common shift preserves
relative timing and permits causal finite support. Existing integer-only fast
paths must remain bit-exact.

Requested refinements:

- Expose source-rate common offset and source sample rate (or physical seconds)
  through both consumer preparation paths. Merely adding LoadedSofa metadata
  is insufficient when Binaural immediately discards the wrapper and XTC retains
  only delay_applied. Make added absolute timing inspectable without changing
  the host latency protocol.
- Fractional zero must produce a literal unit tap; floating sinc evaluation at
  integer arguments must not create tiny nonzero tails in mixed datasets.
- Include fractions approaching zero/one, signed integer controls, and explicit
  phase unwrapping. The proposed phase-equivalent delay error is phase delay,
  not derivative group delay; label and test the intended metric accurately.
- Check conversions, common offset and output sizes before allocating, including
  enormous negative common offsets with small relative spreads.
- Capture actual pre-edit loader samples and full Binaural/XTC rendered output
  artifacts when the shared Cargo slot is available. Source backups and older
  scalar logs alone do not establish complete-output preservation.
- XTC must test the loaded plant response and resulting cancellation/inverse
  behavior separately: a common plant phase shift does not imply the inverse
  filter output simply receives the same delay.

65-tap passband limits are proposed approximation bounds through0.45 cycles
per sample, not a Nyquist-band guarantee. Tests must compare to independent
analytic phase/magnitude rather than duplicate coefficient generation.

Revised design approved for bounded implementation. Active consumer offset
getters preserve provenance, mixed integer members use literal copies, phase
metric is explicit, and XTC plant/inverse cascade is separately tested. Captured
pre-edit actual loader and all three Binaural/two XTC route arrays are present
and checksums verify; capture log passes1/1. Preserve these and exporter source
until post-change comparisons are recorded. DC must be a separate gain check;
phase-delay division applies only to positive frequencies. Implementation and
final numerical/preservation evidence remain subject to independent review.

## First implementation source review

**P1 fractional boundary arithmetic:** coefficient center `32+fraction` can
round to an integer for valid tiny or near-one fractions, and unguarded
`sin(argument)/argument` then produces NaN at that tap. Separately adding the
common offset in f64 before flooring can round the integer prefix upward while
retaining the original near-one fraction, introducing roughly one extra sample
of delay. `ceil(32-minimum)` can itself round down at integer boundaries.
Examples include1e-16, -1e-16 and the representable predecessor of1.0.
Requested robust checked integer/fraction decomposition, sinc zero handling,
and nextafter/tiny signed boundary regressions with finite output and analytic
phase-delay checks. A direct Python arithmetic probe reproduced the rounded
integer centers; no validator Cargo run.

Consumer provenance commits occur on successful preparation and failure tests
start from active fractional data. Loader analytic DTFT and XTC plant/cascade
oracles are meaningfully independent. XTC crossfade target correctly accounts
for the existing identity/inverse blend. Cross-rate Binaural tests compare
complete process/EOS output to independently materialized IR fixtures. Review
acceptance pending the numerical boundary fix and corresponding final gates.

Boundary revision inspected: floor-based common offset/prefix no longer forms
rounded fractional effective sums; tap-relative sinc uses its zero limit.
Signed minimum-subnormal and adjacent-one tests assert offset33, finite output,
DC and independent analytic magnitude/unwrapped phase delay. Prior NaN and
one-sample-shift finding closed. Focused16/16 plus one manual ignored capture,
strict scoped lint and format logs pass. Source cleared for required coordinated
workspace gate and final report/manifests; implementation acceptance awaits
those final evidence artifacts. No additional source defect identified.

## Scoped final acceptance

Reviewed completed `audit/sofa-fractional-delay.md`. Current scoped source and
lock verify against manifest `37df45680a7704c83861d0969a033fb60f8ff313b3b597cab0e887105d7575d9`.
Independently compared pre/post loader and five complete consumer arrays:
byte-identical. All scoped findings closed; AUD131 implementation accepted.

Workspace gate completed6064 passed/1failed/14skipped in271.106s, with sole
failure the parallel AUD132 rejected no-input-delay experiment. This is not a
green workspace result. Equal historical start/end manifest021d0b33… reflects
that executed snapshot; current full manifest differs in later documentation
and active Upmixer work, while AUD131 scoped source remains identical. Report
accurately states this limit. Coordinate one later green broad gate after
AUD132 stabilizes; no docs-only duplicate gate requested. AUD128 and broader
spatial/plugin audit remain open.

Coordinated final workspace requirement closed: AUD132's final stable tree
passed6071/6071 tests with15skipped in269.739s. Equal start/end/current
manifest1e4bb4097b37cc238911b211606d76f8e3c4f071de6683b5a388e9c0a61d6aeb
and lockc161c74a… verified. Historical6064/1failure result remains documented;
the new green gate supersedes its outstanding verification requirement.
