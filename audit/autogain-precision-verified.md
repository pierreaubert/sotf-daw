# AUD105 / AUD110 AutoGain smoothing and precision verification

## Final contracts

AUD105 makes scalar, block, one-frame and void `next_n` calls advance the same two-stage recurrence. Configured smoothing controls the dB pole, followed by the existing 20 ms gain-reduction / 300 ms gain-recovery linear pole. `next_n(0)` and zero-frame application are no-ops, surplus output is untouched, and stable scaling requires both retained states to repeat exactly after a real step. Unity eligibility is exact.

AUD110 deliberately changes enabled scalar waveforms to remove the independently demonstrated f32/approximate-exponential precision floor. Private dB and linear states, coefficients and conversion cache are f64; `exp(db * LN_10 / 20)` supplies accurate cached conversion, and public gain/audio remain f32. Target selection retains the original f32 measurement subtraction and clamp, then widens that selected target. Public APIs, defaults, schemas, meter clocks and caller scheduling are unchanged.

The original strict `abs(current_db - target_db) < 1e-5` dB pre-step snap remains. It is not a floating-point stall workaround. Nonpositive configured time preserves coefficient zero and immediate target-assignment behavior. Time/rate updates preserve gain history, disabled calls freeze state, the disabling setter retains its zero-time snap behavior, and reset restores zero dB / unity gain and cache. Exact stationary tests use both f64 states, never equality of rounded f32 output.

## Historical compatibility checkpoint

The AUD105 intermediate preserved all 20 public scalar-caller renders exactly: LoudnessCompensation pre/post, ABCompare, AAE and Upmixer; 48/96 kHz; 25/400 ms controls; six seconds plus 17 frames of stepped tonal/dense audio, irregular callbacks, and a live time change. The 124,420,896 output bytes, source snapshots, binaries and checksums are retained in `target/audit-tmp/autogain-aud105/`. `aud105-intermediate-checkpoint.json` identifies the original and intermediate source hashes and records all 20 exact comparisons.

That scalar-preservation goal was explicitly superseded by AUD110 accuracy. The original XTC independent 0.01 dB fixture failed with the preserved scalar floor; it was neither weakened nor lengthened. The original and intermediate +20log10(2) scalar trajectories were bit-identical at four and thirty seconds. At 44.1 kHz the old gain stayed at 6.00958824 dB, and at 192 kHz at 5.99452114 dB, instead of the selected 6.02059984 dB. Longer tests cannot remove that floor.

## Independent numerical evidence

- The permanent 192 kHz / 5000 ms / 20 second closed-form first-pole and independent base-ten second-pole oracle failed before AUD110 with maximum relative gain error **0.0167288738**. It now measures **5.9582541e-8**.
- Across 44.1/48/96/192 kHz, negative/zero/25/100/1000/5000 ms and both target directions, the independent cascade's worst relative error is **5.9601437e-8**. The permanent bound is `8e-8`, covering f32 output rounding and independent f64 evaluation order.
- The independent target-reversal oracle measures **5.9585519e-8** and includes 5,014 release steps after the final target becomes negative, verifying the intermediate-gain branch rule.
- At four seconds the corrected octave target is 6.02058840 dB at all four rates (about 0.00001144 dB remaining settling error); at thirty seconds its public dB equals the selected target exactly. The existing XTC 0.01 dB amplitude/loudness assertion passes unchanged.
- Exact scalar/block/bulk state and audio comparisons cover 1/17/137/8193 frame partitions, control changes, multichannel layouts, reset/disable/rate changes and prefix canaries. A new test explicitly proves that consecutive equal f32 outputs must not stop a still-changing f64 state. Strict near-target snap boundary cases are retained.
- Cold gain calls, conversion, changed controls, reset and reuse have **zero allocations and zero deallocations** on fresh threads (mono/stereo/eight channels).

The 20 public scalar-caller renders remain deterministic and finite after AUD110, with intentional differences from the exact intermediate. Maximum absolute sample delta is **0.0005633533** (ABCompare, 96 kHz / 400 ms); maximum RMS delta is **0.00009801174**, and maximum relative RMS delta is **0.0011162102** (0.111621%). These are compatibility measurements, not independent accuracy thresholds. Full per-case results are `target/audit-tmp/autogain-aud105/aud110-scalar-deltas.json`.

## CPU measurements and limits

The optimized same-process helper harness includes immutable original, intermediate and final implementations with identical external dependencies. It covers 144 combinations: mono/stereo/eight channels, callbacks 1/17/512/8193, disabled/unity/settled/ramp, and block/scalar/bulk entry. Seven rotating-order trials discard the first and report medians. It measures 24,017 frames at 48 kHz; these are local relative CPU costs, not whole-plugin release deadlines or cross-platform guarantees.

Representative stereo / 512-frame values, milliseconds for approximately 0.5 seconds of audio:

| Mode / entry | Original | AUD105 intermediate | AUD110 final |
| --- | ---: | ---: | ---: |
| Disabled block | 0.000230 | 0.000230 | 0.000230 |
| Unity block | 0.004258 | 0.003527 | 0.003577 |
| Settled block | 0.118615 | 0.003596 | 0.003656 |
| Transition block | 0.117252 | 0.180632 | 0.230747 |
| Unity scalar | 0.166676 | 0.152900 | 0.152468 |
| Settled scalar | 0.197724 | 0.154022 | 0.152179 |
| Transition scalar | 0.197914 | 0.199177 | 0.266896 |
| Transition bulk | 0.001563 | 0.183197 | 0.182867 |

Thus true conversion adds approximately 27.7% to the intermediate active block transition and 34.0% to its scalar transition in this fixture. The original bulk approximation was O(1) and numerically different; correct per-frame advancement has a real O(N) transition cost. Exact unity and stationary paths retain bounded shortcuts. All disabled/unity audio comparisons were exact. Some tiny timings are sensitive to compiler code layout: e.g. disabled scalar measured 0.041379→0.070404 ms, despite unchanged behavior; no universal speedup is claimed.

An isolated actual ABCompare source harness compares its unchanged unity fast path with the permanent private prepared near-unity scenario for 137 frames before a new meter publication: 17-frame callbacks cost 0.008607→0.011292 ms; one 137-frame callback costs 0.007614→0.009859 ms. Maximum audio difference is 4.47e-7, which the old approximate shortcut would omit. This synthetic internal history is not publicly reachable through runtime path removal: the earlier public-removal scenario was retracted because structural path setters reject it. Permanently empty public paths retain equal meter histories and the unity shortcut.

## Gates and exact artifacts

- Focused helper: **22 passed**, `target/audit-autogain-precision-green.log`.
- Complete host, ABCompare, LoudnessCompensation, AAE, Upmixer: **1119 passed / 8 existing ignored**, `target/audit-autogain-precision-full.log`. This includes concurrent already-approved host correlation regressions, so the count is not exclusively new AutoGain tests.
- Same five packages, strict all-target Clippy: clean, `target/audit-autogain-precision-clippy.log`.
- Parent independent EQ/Crossfeed/XTC complete gate: **432 passed / 1 existing ignored XTC doctest**, `/tmp/sotf-autogain-precision-callers-full.log`; strict all-target Clippy clean at the matching `...-clippy.log`.
- Source review and independent oracle review: [independent review](autogain-precision-independent-review.md).
- Precision red: `target/audit-autogain-precision-red.log`; before/final plateau: `target/audit-autogain-precision-plateau.log`.
- CPU: `target/audit-autogain-precision-cpu.csv` and `target/audit-autogain-precision-ab-cpu.csv`.
- Raw 20-case renders: `target/audit-tmp/autogain-aud105/callers-before`, `callers-after` (intermediate), and `callers-aud110`; final source/manifests/checkpoint JSON in the parent directory.
- Owned source formatting and scoped `git diff --check`: clean.

## Files and remaining scope

Production is restricted to `sotf-host/src/auto_gain.rs` and AUD105's exact unity predicate. New tests are `sotf-host/src/auto_gain/smoothing_tests.rs`, `sotf-host/tests/auto_gain_heap.rs`, and ABCompare's private `src/auto_gain_tests.rs` plus its module registration. Independent caller tests are owned by the dynamics agent. AUD108's separate EQ reset-counter correction is not scalar-waveform preservation evidence.

The original smoothing proposal describes the historical AUD105 compatibility stage; this report and [the precision proposal](proposals/autogain-precision.md) supersede that part. EQ/Crossfeed callback-based metering and within-callback future influence remain separate pending AUD112/AUD113 findings, documented in `autogain-caller-clock-proof.md`; this correction does not fix or conceal those clocks. No host graph queues, engine protocols, MIDI or IAMF changed.
