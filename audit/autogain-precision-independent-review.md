# AUD110 independent source and oracle review

Reviewed 2026-09-28. Read-only: no Rust edits or duplicate builds. Proposal: `/tmp/sotf-autogain-precision-plan.md`; source: `sotf-host/src/auto_gain.rs`; tests: `src/auto_gain/smoothing_tests.rs`.

## Result

No correctness blocker found in the reviewed implementation. The deliberate f64/accurate-exp correction supersedes AUD105 waveform compatibility; the archived 20 exact scalar baselines remain historical evidence rather than a new assertion.

- The selected target still uses existing f32 measurement subtraction/clamping before widening. Both poles and coefficients now retain f64 state. The original strict 1e-5 dB snap is preserved, including its pre-step ordering and immediate target assignment only when smoothing coefficient is zero.
- The second pole selects attack/release from the intermediate converted gain versus the current retained linear gain. The independent reversal fixture proves that release continues for more than 100 samples after the final target has already reversed downward; it would catch incorrectly choosing from final target direction.
- Cached conversion is keyed by the exact internal dB value. Scalar, block and void bulk advancement share one recurrence. The stationary shortcut requires both internal f64 states to repeat after a real step. Equal rounded f32 outputs alone do not qualify; the 192 kHz/5000 ms fixture explicitly verifies internal progress while consecutive output gains compare equal.
- Disabled calls freeze state and return unity; the disable setter retains its existing target-zero/coefficient-zero semantics. Zero-frame calls do not access or change audio/state. Block processing touches only the requested prefix. Reset restores both states and conversion cache. Time/rate updates preserve gain history and recompute coefficients in f64.
- The closed-form first pole and base-ten `powf` conversion are independent of the recursive production first pole and natural-exponential conversion. The second reference pole uses the independently selected intermediate direction. Its 8e-8 relative bound allows one f32 output rounding, not the old multi-stage approximation error. Constant-target snap handling uses the previous closed-form distance, matching the documented strict pre-step policy.
- The long 192 kHz/5000 ms/20 s case covers four configured time constants without reaching the snap region, so its closed form remains appropriate. Separate 4/30-second plateau tests at four rates and both gain directions test the accuracy floor. Exact threshold controls below/at/above 1e-5 prevent silently broadening the snap.

## Limits

This review inspects source and tests; execution and cold allocation/free results belong to the implementation owner's gate. Caller meter clocks and invalid sample-rate/control transactionality are unchanged, outside this scoped correction. The old XTC 0.01 dB analytical test must remain unchanged and pass in the coordinated caller rerun. The reviewed lifecycle rate/time test checks state preservation and coefficients directly; subsequent scalar/block equality and independent constant/reversal trajectories cover the shared recurrence.
