# Deferred audit and feature work for the next cycle

User decision (2026-10-03): prioritize a stable sotf-daw merge checkpoint so sibling integration and release can proceed. Original requirements remain recorded; deferral does not mean implementation or acceptance is complete. Scope and original74IMPLEMENT/INTEGRATE IDs: [requirements](requirements/README.md), [inventory](continuation-2026-10-01/current-feature-inventory.md).

## Deferred work

| Area | Remaining work | Release handling |
| --- | --- | --- |
| AB Compare/host | Coefficient-derived uniform mask support; variable-producer identity-probe repair; multi-source admission; double-active-mask EOF; terminal-sink tail precision; expanded consumer proof | Restore conservative validated tail reporting. Do not ship unproven finite support. Keep existing quota/error safeguards. |
| Declick | Stronger real-music repair accuracy, quadratic estimator/ring-tail correction, detector-envelope research and new demanding synthetic characterizations | Restore R47 validated detector/emission; retain existing regression gates. Known piano accuracy limits remain documented. R48/R49 experiments retained as deferred evidence. |
| Denoiser | Captured-profile file/blob persistence across all adapters; blind wanted-tone/transient preservation with compatible defaults and false-protection analysis; further corpus/performance evidence | Keep stable curve/audition integration; fix actual fade/restore defects. Defer uncompleted profile expansion and preservation algorithms. Known blind-mode quality limitations remain explicit. |
| FIR | Original M1 response tolerance, including 96 kHz/1024 taps/phase 0 at 500 Hz | `multiband_response_matches_on_every_tap_count_and_phase` explicitly deferred/ignored; original measured error −0.245743895 dB, now −0.2462965578 dB after centered-phase correction. Stimulus and bounds unchanged; run with `--ignored` to reproduce. Unpaired-endpoint phase leakage is fixed; magnitude accuracy remains unresolved. |
| Speech/other plugins | Held-out corpus/STOI/listening and remaining per-plugin IMPLEMENT/INTEGRATE/VERIFY items | Preserve validated current behavior; use original requirement files for next-cycle assignments. |
| Platforms/native UI | Remaining hardware, macOS/iOS/AU, physical widget gestures and fresh loaded native coverage where not available here | Separate release-platform validation by owning checkout/platform; no platform claim from Linux tests. |

## Required before this merge checkpoint

- Existing accepted regressions restored; no unsafe experimental behavior hidden behind ignored tests.
- Workspace build and strict lint, broad tests with explicit environment/platform exclusions.
- Focused affected host/engine/DSP/FFI/native adapter tests.
- Every newly deferred characterization identified by name and reason in stabilization receipts. Ignored/deferred tests are not counted as passing.
- Scoped commit contents, exact verification summary and sibling integration notes. Keep generated build artifacts and raw worker event streams out of commits.

Tracking: [stabilization issue](continuation-2026-10-01/release-stabilization/ISSUE.md), [plan](continuation-2026-10-01/release-stabilization/PLAN.md). Final per-lane dispositions and checks will be linked there as they complete.

## Stabilization handoffs

- [Declick restoration](continuation-2026-10-01/release-stabilization/declick-result.md) and [accuracy backlog](continuation-2026-10-01/declick-boundary-accuracy-r1/deferred-backlog.md).
- [AB conservative checkpoint](continuation-2026-10-01/release-stabilization/ab-result.md).
- [Denoiser checkpoint](continuation-2026-10-01/release-stabilization/denoiser-result.md) and [parked profile restoration](continuation-2026-10-01/denoiser-feature-continuation-r1/parked-profile-r6/RESTORE.md).
- [Merge handoff and limitations](continuation-2026-10-01/release-stabilization/MERGE-HANDOFF.md).

## FIR-M1 fixed-support investigation (2026-10-21)

Cumulative relative-error fitting still fails the unchanged FIR-M1 gate
(−0.21736 dB at 96 kHz/1024 taps/500 Hz). An independent scalar check of a
4097-point annihilating-functional witness gives a conservative 0.21120 dB
full-spectrum error lower bound for that centered support. This numerical bound
includes finite moment leakage and matches the independent target to 2.87e−12
relative. It concerns the full sampled target, not just the four isolated
FIR-M1 probes, and is not interval-arithmetic certification. The solver was
removed and production source restored; no completion or changed contract is
claimed. Reproduction and raw evidence:
[validation](artifacts/plugin-audit-20261018/fir-m1-lawson-validation.json),
[scalar verifier](artifacts/plugin-audit-20261018/verify-fir-m1-lawson-witness.py).
