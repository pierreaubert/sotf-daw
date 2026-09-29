# Independent PND drain design review

2026-09-28. Reviewed `/tmp/sotf-spatial-finite-review/pnd-drain-proposal.md` against current source through TokenSave and direct source slices. No repository source/test changes or PND Cargo runs. Existing public probe logs were inspected; this review does not rebrand those runs as newly executed tests.

## Verdict

**No finite-support or scheduler blocker found.** The proposed 3583..4094-frame continuation, unchanged 2047-frame latency, frozen effective correction, and bounded sample-at-a-time synthesis design are consistent with current implementation. The implementation/test clarifications below should be retained before calling it verified. No AUD077 implementation is involved.

## Support and off-by-one proof

Source references (relative to `crates/sotf-plugins/crates/sotf-plugin-pnd`):

- `src/lib/consts.rs:1-16`: N=2048, H=512, prefill P=N-H=1536, latency D=N-1=2047.
- `src/lib/phase_vocoder_channel.rs:75-83`: input prefill and zero output cursor/fill.
- `src/lib/pnd_plugin.rs:344-380`: write source sample, transform if full, then emit the first available synthesized sample **on the same external sample**.
- `src/lib/phase_vocoder_channel.rs:420-444`: N-sample overlap-add at output_read+output_fill; advance available output by H and retain N-H source samples.

For zero-based transform number m, the analysis origin is `a_m=mH-P`. Its synthesis origin is emitted at external index `b_m=(m+1)H-1`; hence `b_m-a_m=P+H-1=D`. The last potentially occupied analysis origin for S>0 is `w=floor((S-1)/H)H`. Its contribution occupies at most indices `D+w` through `D+w+N-1`. Therefore exclusive endpoint `E=D+w+N`, and `R=E-S=3583+((512-S%512)%512)`.

This is a conservative support count, not a claim that the final output sample is nonzero. In particular a marker exactly on the left Hann endpoint contributes zero in its latest containing window. It is appropriate to retain the same maximum count across contents.

| Accepted S | Exclusive E | Drain R |
| ---: | ---: | ---: |
| 1 | 4095 | 4094 |
| 511 | 4095 | 3584 |
| 512 | 4095 | 3583 |
| 513 | 4607 | 4094 |
| 2047 | 5631 | 3584 |
| 2048 | 5631 | 3583 |
| 2049 | 6143 | 4094 |

An independent Python enumeration of analysis-window origins and emission origins for **all S=1..8192** matched this formula. It did not invoke the production scheduler or a prospective drain helper. TailLength::Finite(4094) is valid under the proposal's stated inclusion of delayed output in its continuation bound. Empty accepted input must return no padding.

The amplitude proof also holds: current FFT magnitudes overwrite analysis magnitude (`phase_vocoder_channel.rs:171-183`); envelope floors affect only bounded gain (`:311-324`); transported magnitudes are cleared each hop (`:306-309`) and contain no added audio floor. The IFFT spectrum is cleared and bins with magnitude <=EPSILON are skipped (`:370-376`). Old phases/peak assignments cannot create amplitude after an all-zero window. OLA read cells are cleared (`pnd_plugin.rs:370-373`). This proof assumes finite valid DSP state as the proposal explicitly says; it is not a huge-input overflow fix.

## EOS ratio and controls

The latch formula matches current arithmetic at `pnd_plugin.rs:343-347`: the f64 current ratio is combined with the **current smoothed f32 strength**, then converted to f32. It need not equal the ratio of the most recent transform when EOF lands between transforms; using the final effective per-sample control state is the proposed and defensible policy. Freeze it once, after all validation and before the first accepted padding. Do not advance the strength smoother even once before latching.

The fixed path must bypass external analyzer feed/consensus, drift/reference-transition updates and UI cadence (`pnd_plugin.rs:289-341,384-416`), while retaining vocoder onset/phase evolution for residual windows. Otherwise accepted EOF acquires an accidental learning rule tied to padding length.

**Scoped test correction:** synthetic zero-support tests should include `q=1/0.95≈1.0526316`, not assume 1.05 is the complete upper correction range. The reference matcher accepts drift ratios within ±5% (`analysis.rs:333-338`) and the controller uses their reciprocal (`pnd_plugin.rs:326`); temporal matching uses ±3% (`analysis.rs:413`). The proof is valid beyond the proposed three test ratios, but the stated test coverage should reflect actual reciprocals.

Identical recognized values may return success during EOS only as true early no-ops. Guard against accepting unknown IDs or wrong types merely because a getter is absent; do not execute the reference setter's analyzer reset (`pnd_plugin.rs:607-627`) or rebuild metadata (`:641`) on a supposedly harmless snapshot. Structural setters currently reject even identical values after initialization (`:556,:579,:618,:628`); if the EOS no-op exception allows those values, document it explicitly or limit the exception to live scalar controls. No additional parameter API route exists in this crate.

## Transactionality and reset

- Process already validates exact lengths, rate and finite input before touching DSP (`pnd_plugin.rs:700-735`). Keep accepted-source phase/flag changes **after successful kernel processing**. A zero-frame process remains the existing no-op.
- Drain needs the same preflight before setting remaining/q: initialized vocoder, nonzero channel count, matching rate, whole output frames, then positive capacity only if work remains. Empty input and repeated completion must accept zero capacity without becoming a noncomplete busy loop. Use the actual processed prefix `min(capacity,H,remaining)*channels` when calling the existing exact-length kernel; passing the whole larger caller buffer would violate that kernel's length check (`:275-283`). Preserve the caller suffix.
- If `mem::take` lends prepared zeros, restore it before propagating any Result. Avoid routing through public process after the EOS latch, because that route is meant to reject new programme input and might also enlarge the source count.
- Calling reset after successful initialize addresses a real existing state gap: initialize currently rebuilds analyzers/vocoder but leaves current_ratio and last_drift_ratio (`:667-693`), whereas reset clears both (`:740-755`). Validate rate/reference/channels and checked scratch size **before** mutating the existing prepared instance; test failed reinitialize against an untouched active twin.
- Reset preserves configured targets and sets current strength to its target, matching fresh initialization. Vocoder reset clears retained source/OLA/phase state (`phase_vocoder_channel.rs:105-139`), and analyzer reset clears the ring/tracking history (`analysis.rs:359-386`). Scratch FFT arrays are overwritten before use. Channel consensus scratch currently survives reset, but every configured analyzer refreshes its slot before first consensus after reset; no retained-audio defect is established from that scratch alone. Clearing it is reasonable deterministic cleanup, not a separately proven audible bug.
- Retain existing publication ownership checks, including the two-held-generation reset test (`src/lib/tests.rs:548`). Zero allocations **and frees** should be measured on a fresh callback thread for drain/reset/getters. Calling initialize itself is a control-thread allocating operation and should not be included in an allocation-free claim.

## Oracle independence: required clarification

The neutral `[2047 zeros]+programme` reference is genuinely independent and should use explicit FFT numerical tolerance with normal-amplitude fixtures. Small FFT magnitudes are intentionally dropped by EPSILON, so it is not an exact bitwise/subnormal identity contract.

For nonunity tests, “identical accepted history” must mean the **complete adaptive pre-EOF vocoder state**: source/OLA cursors and contents, previous magnitudes/phases, transient position and phase-lock ownership. A fresh vocoder replaying the entire input with the final latched q will generally differ for valid reasons. Use a test-only snapshot after identical ordinary processing, or independently drive the exact recorded pre-EOF control trajectory, then advance zeros using a standalone sample scheduler and independently calculated q. Do not call the new fixed-path dispatch or drain helper from the expected-output side.

This shared-vocoder reference verifies EOS dispatch, estimator freeze, partitioning and support. It is not an independent oracle for the pitch-shifter's spectral algorithm; the neutral analytical reference and explicit zero-amplitude proof cover the relevant independent properties without inventing proprietary pitch parity.

Snapshot private controller state directly: the public cache is throttled every ten callbacks and may be stale (`pnd_plugin.rs:381-409`), and the correction-strength getter reports the target (`:645-647`), not the current smoothed value. Add failed-call-before-first-drain, failed-call-mid-drain, and failed-reinitialize replay comparisons. These refinements are narrow tests/lifecycle details; no replacement of the proposed support design is needed.
