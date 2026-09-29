# AUD124 independent validation — programme maximum true peak

Validator: Astra, medium. Implementer: Luna, xhigh.
Status: **CORE/API STAGE ACCEPTED** (2026-09-28).
AUD124 remains open for TUI/GPUI display integration; the full audit remains open.
No AUD124 production changes were present at initial review, 2026-09-28.

## Confirmed requirement and source finding

[EBU Tech 3341 section 2.8](https://tech.ebu.ch/docs/tech/tech3341.pdf) includes
measurement and display of Maximum True Peak Level. The current host consumes
per-channel interval peaks in `take_interval_peak`; snapshots contain the latest
interval, and EOS merges only the immediately preceding snapshot with the final
suffix. An earlier larger published peak is not retained as a programme scalar.
This is a feature-parity gap, not evidence that SOTF advertises EBU Mode.

## Design review findings

1. **Clarify silence/serialization before implementation.** An optional dBTP
   value of `Some(-infinity)` serializes as JSON null and returns as `None`.
   A finite-only optional scalar can use `None` for no positive observed peak,
   including silence, with existing supported/enabled status fields providing
   context. Alternatively use an explicit valid-silence state. Do not claim that
   `None` exclusively means unsupported/unobserved while encoding silence the
   same way accidentally. Test the chosen representation and older payloads.
2. **Complete snapshot wiring.** Update `LoudnessData::new`, `Default`,
   `update_from`, `reset_loudness_data`, explicit struct literals and manual
   consumers as applicable. A monitor-only assignment would leave stale values
   in reused/cleared snapshots.
3. **Protect epochs.** Include rejected callbacks, empty query/finish and reset
   while all snapshot generations are held. Releasing readers after reset must
   publish only the current epoch's maximum. Deferred publication must retain
   the authoritative programme maximum without allocation.
4. **Retain the display obligation.** The core/API change is a coherent staged
   batch within the writable DAW repository. The TUI/GPUI integration in sibling
   `sotf` remains explicitly open; core acceptance alone cannot close the whole
   measurement-and-display requirement. Proceed with authorized local work;
   respect sandbox and existing review blocks for sibling changes.
5. **Avoid deciding future pause semantics accidentally.** Future I/LRA pause
   must not clear the programme peak. Whether peak measurement continues during
   that pause belongs to AUD126's explicit contract.

Existing unrelated non-finite snapshot fields may limit full cold JSON
roundtrips. Inspect the new field's cold/silent JSON representation separately
and use an otherwise valid finite fixture to verify old-payload defaults and
roundtrips; do not silently expand this batch into unrelated serialization work.

## Acceptance checklist

- Red-to-green public early-high/later-low, cross-channel and query-frequency
  tests preserve interval vectors while retaining the programme scalar.
- Independent published convolution and custom-rate reconstruction/complete
  support expectations, including final suffix, arbitrary callback partitions,
  unqueried intervals and no sample-peak substitution.
- Reset/reinitialize/disable-enable, unsupported rates, empty/silent input,
  rejected callbacks, strong/Weak retained readers and repeated drain.
- Complete snapshot/serialization wiring and explicit consumer/UI disposition.
- Cold processing/query/publication/reset/drain adds no allocation/free/lock.
- Scoped tests and strict lint, with integration gates appropriate to public
  `LoudnessData` changes. Inspect executed logs and final source before acceptance.
- Full audit and the sibling display portion remain open until their own
  implementation and evidence are reviewed.

## First implementation review

The proposal now defines a finite-only optional dBTP value, with `None` for
cold/silent/unsupported measurements. Source inspection confirms initialization,
default, `update_from`, monitor reset, cleared-cache reset, ordinary queries and
cache reconstruction carry/reset the scalar. The query loop reduces the same
per-channel interval peaks used by existing telemetry, retaining earlier maxima
without extra allocations or a new per-sample path. No production defect found.

Inspected five new public tests and the updated nested/heap assertions. They
cover published-table programme expectations, early-high/later-low intervals,
custom rates, rejected/empty callbacks, reset with all generations retained,
reinitialization/enable epochs and JSON defaults/finite roundtrips. The executed
host log `/tmp/sotf-aud124-host-full.log` completes successfully, including the
new suite and 308.31-second LRA allocation stress test; strict lint is reported
at `/tmp/sotf-aud124-host-clippy.log`.

**Requested evidence correction:** the new programme fixture's final impulse
is quieter than its early maximum, so it cannot detect failure to include EOS
in the new scalar. Existing finite-stream tests only excluded the new field
from unrelated-telemetry comparisons. Add final-only or late-largest impulses
where drain must raise `maximum_true_peak_dbtp` to the independent complete
response at 48/96 kHz and a custom rate, including held strong/Weak publication
and stable repeated drain. Then run current workspace integration after the
public `LoudnessData` extension, without duplicating another full host gate.

Sibling UI display remains open. No full AUD124 acceptance is implied by this
core source review.

## EOS evidence revision

Reviewed the added final-only unit impulse at 48/96/8 kHz. The test requires
the pre-drain published maximum to be absent or at least 0.1 dB below the final
reference, fills all prepared generations with strong or nested Weak readers,
checks blocked drain preserves the old snapshot identity/value, then releases
readers and checks the new scalar against the complete response. Repeated drain
must preserve the exact recovered scalar. Published rates use the independent
coefficient-table oracle to 2e-12 dB; 8 kHz uses a separate full-support Lanczos
reconstruction with 0.03 dB tolerance. This would fail if the new programme
scalar omitted the final FIR suffix; the previous vacuous fixture is repaired.

Inspected `/tmp/sotf-aud124-eos-focused.log` (six passed) and
`/tmp/sotf-aud124-eos-clippy.log` (strict all-target lint passed). No remaining
core source/test blocker found. The final offline workspace nextest gate is
running; acceptance awaits its result. Sibling display remains the next
implementation stage rather than a silently dropped requirement.

## Staged acceptance

Inspected the completed `/tmp/sotf-aud124-workspace-nextest-final.log`:
**6,003 passed, 11 skipped, 355 binaries, 347.721 seconds**. FFI is included;
MIDI/IAMF are excluded. Six focused programme-maximum tests and strict host
all-target lint pass after the final EOS test refinement. Together with the
previously reviewed source, independent numerical oracles, snapshot/epoch
checks, serialization checks and zero-heap fixtures, this satisfies the staged
core/API acceptance criteria. No remaining core finding requires a change.

Validated end-state SHA-256 values against the implementation report:

- `src/analyzer.rs`:
  `3f3a4f89271a54b9ea0e115b116677dfdda772523ae00c4190e15a289993d20a`
- `src/analyzer_loudness_monitor.rs`:
  `78de07ceb22697adce4acf715412d36358fc878ad20cdf1854d33bfc6f083cf1`
- `tests/programme_true_peak.rs`:
  `fa7a0b9cc5927fd718b998442f77428838c399f656570b333afcef7add12d0fd`

Paths are relative to `crates/sotf-plugins/crates/sotf-host`.
No pre-run manifest was captured. The implementation owner reports that AUD124
paths stayed unchanged during execution; parallel Upmixer work used separate
paths. These hashes identify the reviewed end state and do not prove that the
entire concurrent workspace was frozen. Future broad gates require explicit
start/end snapshot manifests and coordinated source freezes.

The scalar adds a bounded finite check/comparison at interval query time; no
separate CPU microbenchmark was run. Existing interval vectors and math-dsp
scope are preserved. Authentic EBU corpus certification remains outstanding.

Next: prepare and review concrete TUI/GPUI programme-maximum display integration
under the sibling repository's instructions and permission boundary. Keep
AUD124's display obligation visible until actual integration and UI evidence
are complete. Core acceptance alone does not satisfy the EBU display requirement.

## UI proposal review

Reviewed `audit/proposals/programme-maximum-true-peak-ui.md` and the actual TUI
heading/redraw signature and GPUI meter fallback. The scoped display, locale,
manual-fixture/query and tests are coherent. No sibling source was edited.

Two corrections are required before producing the concrete patch:

1. A finite programme scalar must take precedence over cold status flags.
   `set_spatial_enabled` reconstructs cache snapshots with the retained scalar
   while the generic snapshot constructor leaves true-peak status false. A UI
   condition requiring `true_peak_is_compliant` would hide that known finite
   maximum. Format finite `Some` directly; treat non-finite `Some` defensively;
   apply unavailable/empty-state flags when the scalar is `None`. Test a finite
   scalar with both status flags false. Preserve GPUI's existing sample-peak
   fallback heading/bars independently of the programme summary.
2. Redraw quantization must match displayed precision. TUI's current signature
   rounds to 0.1 dB, while the proposal's example uses two decimal places.
   Either display one decimal or hash the scalar at 0.01 dB; test a visible
   rise within one old 0.1-dB bucket so it cannot remain stale on screen.

Also check narrow widths and longer translations. Requested a concrete patch
artifact under the writable audit tree (or approved isolated staging) for the
next review. Preserve sibling user work and permission boundaries; a proposal
alone is not the final reviewable implementation.
# UI patch review — first staged draft

Authorization update: the user explicitly authorized sibling writes with
"you can patch sotf". Parent relayed this authorization on 2026-09-28.
The reviewed UI integration may be applied and completed without another user
confirmation. Prior unrelated approval blocks remain separately scoped.

Revision disposition: the later draft with SHA-256
`b0271636944fbac6edc2f3baf8dc813b5b3af96e862706c8dbc9ea6a1c92ab1d`
addresses the concrete source findings below. The TUI always renders status for
an available snapshot, stacks German unavailable text at 24 columns, GPUI uses
separate label/value rows, and the invalid pseudo-length cap is removed.
Independently verified read-only `git apply --check` against the sibling tree.
The artifact is acceptable as a concrete proposal for boundary authorization;
this does not accept the completed UI feature. No sibling source was written.
The proposal correctly leaves GPUI rendered-layout and routed endpoint tests,
compilation and execution pending. Those remain required before AUD124 closure.

Reviewed `audit/proposals/programme-maximum-true-peak-ui.patch` without applying
it to the sibling checkout. Scalar propagation and finite-value precedence are
consistent with the accepted core contract. Requested these corrections:

- The pseudo locale expands `Unavailable` to 16 characters, exceeding the
  draft component test's 15-character maximum.
- The TUI content gate hides an unsupported snapshot when its interval vector
  is empty and its programme maximum is None; the specified unavailable marker
  must still be displayed.
- A 24-column English finite fixture does not cover longest translated
  unavailable copy. German copy exceeds the 22-column interior, while GPUI
  overflow hiding can clip the summary. Require localized constrained layouts.
- GPUI string-helper and query-helper tests do not exercise the rendered tree
  or routed endpoint; the proposed integration gates remain outstanding.

The staged artifact is not accepted for application yet. No sibling file was
modified and no Cargo gate was started by the validator.

## Applied UI source review

Following explicit user authorization, Luna applied the reviewed UI mapping.
The German narrow TUI fixture reportedly passes. Independently inspected the
GPUI compile log: twelve broader API mismatches prevent test execution (analog
plugin variants, gate fields and a Windows sandbox enum). No GPUI runtime pass
is claimed.

The new E2E scenario only verifies wrapper bounds and reads back assigned app
state. Requested actual rendered-text observation, later lower interval values
with retained maximum, German unavailable state, summary/bar panel containment
and non-overlap, and the still-missing routed endpoint integration. A blank or
incorrect text child can pass the present bounds/state assertions. Stale
proposal language about authorization and no compilation must be updated.
Application-stage acceptance remains pending; parent was informed of the
compatibility blockers. No Cargo was run by the validator during Upmixer's slot.

## Final applied-source revision

Mounted Studio LoudnessMonitor E2E now traverses the real product route. It
checks instrumented painted text through finite/high-then-lower and German
unavailable snapshots, plus summary/bar containment and non-overlap. Compact
component coverage remains supplemental. The live authenticated HTTP test now
makes three requests and owns an explicit-run-ID server/task with shutdown and
Drop cleanup, removing unsafe environment mutation.

Verified final logs: two render tests, one three-request route test and GPUI
all-target compilation pass. Current files match manifest SHA
`cede3dbc05cea747a8838784b3c08a90669f51628e6ca3c811ab83cac5c1398b`.
Sibling lock restored to
`2c87468c46063817fee68c909bf46f9117724a70654b04c933790420b335a31f`;
the passing Cargo gates used the temporary resolved lock, not that restored
resolution. Source review is clear. Requested remaining focused planned
evidence for finite TUI rendering/redraw, translation/helper checks and pseudo
generator check before full display acceptance; no broad repeat requested.

## Final AUD124 display acceptance

Accepted the applied TUI/GPUI programme-maximum display and routed query stage.
Independently inspected final logs: TUI finite renderer 1/1, redraw signature
1/1, earlier narrow German unavailable fixture 1/1, GPUI translation
completeness 1/1, mounted/compact render 2/2, live three-query route 1/1 and
all-target GPUI compile success. Pseudo generator check reported exit zero;
format/design-token/diff checks passed per implementer. Current source again
verified against the unchanged `cede3dbc…c1398b` manifest.

Tested temporary resolved sibling lock hash:
`3d1581cebf529b878c201e52ebe9beec3099434f5409a1f1206038da3bff8009`.
Original lock was restored exactly to `2c87468c…335a31f`, independently checked;
no subsequent Cargo gate ran on that restored resolution. This qualification
must remain in the handoff. The acceptance covers reviewed source plus the
tested resolution, not a claim that the restored lock passes `--locked`.

AUD124 core/API and required application display stages are now accepted.
This does not establish EBU Mode/certification or complete AUD125–AUD128.
Suggested next metering batch: AUD125 M/S maximum display using the same
reachable surfaces and lifecycle rules; its core/API is already accepted.
