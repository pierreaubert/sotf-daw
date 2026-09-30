# AUD139 native callback review handoff

Reviewer: Astra medium, as requested by the user. Implementation owner: Luna
xhigh. **Review completed: scoped scalar/CLAP and artifact/workload-qualified CPU
acceptance**, recorded in `audit/reviews/AUD139-astra.md`. Earlier dispatch
thread-cap failures were resolved when the reviewer resumed successfully.
The following brief is retained as the completed review scope.

## Immutable evidence

- Packet: `audit/artifacts/aud139-native-clap-scalar-checkpoint-r1/`.
- Outer 61-entry index: `5b0ed123c12fe9f3f197a7b99f8e877835516550140b10e231621590ad9e135a`.
- Root verified all 50 actual source entries and seven logs. Original inner
  source manifest has one stale self-checksum; retain it and read the explicit
  `root-verification-receipt.json`. Outer packet index has no self entry.
- Full DynamicEQ package: 71 passed, two manual utilities ignored. This includes
  exact Peak replay and shelf lifecycle regressions; strict all-target lint passes.
- Full NIH library: 116 passed, one ignored; strict all-target lint passes.
- CLAP lifecycle r7: one selected actual callback test passes.
- Sources archived after terminal runs; no pre-run manifest or build-bound
  executable/full source closure is claimed.

## Required review

Check direct getter reads for all globals and eight stored slots, preserving
legacy active aliases and exact dormant IDs. Check scalar setters for valid
exported dormant threshold/ratio controls, global override/smoother/dry-wet
semantics and allocation guards. Earlier getter allocation was reproduced under
GDB; a later all-controls allocation exposed dormant rejection. Both are fixed
in this checkpoint; keep their red history separate.

Check shared getter helper remains generic across its 15 families, with
DynamicEQ-specific assertions limited to that family. Check all-exported
primitive control coverage includes eight stored slots versus four active bands.

Check actual CLAP deferred background/main restart dispatch, shape/slope
coalescing, same-value echo, ignored host request retaining old Peak output,
same-value retry, refused 8 kHz shelf preparation retaining values, valid 48 kHz
reactivation and full native/direct-core shelf audio equality. Normalized slope
readback allows 1e-6 for f32 quantization; full audio comparison remains exact.
Check the reference controls are sensitive to shelf-versus-Peak output.

Append bounded disposition and actionable findings to
`audit/reviews/AUD139-astra.md`. No production changes or overlapping Cargo runs
are required for source/evidence review. Read repository instructions and skills.

## Remaining work

Controlled old/new Peak CPU now passes all 12 cases at 0.989306–1.060697
against the 1.10 budget; see `audit/artifacts/aud139-controlled-peak-cpu-r1/`.
Both variants have fresh distinct executables, matched current dependencies and
compiler/profile, exact shared archived benchmark, and 475 selected inputs
stable across compilation. This is not historical dependency reconstruction.
Check the raw ABBA trials and paired-run ratios; shelf CPU remains in progress.
VST3 restart, actual SOTF consuming host, mounted UI and AU remain open. The VST3 proposed hook queues `Task::TriggerRestart(kReloadComponent)` for
the UI thread through a default-no-op `ProcessContext::request_restart`; it must
not call `schedule_gui` in a way that executes restart inline on the current UI
thread. This is design research, not implemented or accepted evidence.
