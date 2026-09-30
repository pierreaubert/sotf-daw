# AUD143 performance evidence and recovered baseline

Status: **pre-edit source recovered; matched comparison still required**.
Inspected 2026-09-30. This note does not claim a new benchmark pass.

## Existing measurement

`/tmp/sotf-aud143-cpu-before.log` is a successful pre-edit QA run in the
optimized development profile. SHA-256:
`50b898b3db508100a506059387cd2050e7d724327d692adf5f23d4b05fb1391a`.
It reports 12-channel, four-band LR48 setup median 18.565 microseconds and
automated 512-frame callback median 758.557 microseconds. Setup and callback
are separate; printed maxima are observations, not worst-case execution bounds.
The simpler QA loop also reports 5 seconds of program processed in 8.66 ms.

No original CPU run-bound source manifest was located. Do not divide a later
release-profile timing by these values and call it a matched regression test.
The original waveform captures and populated-reset manifests are separate
evidence and do not retroactively establish that CPU run's full source state.

## Recoverable source

The saved pre-edit populated-reset source manifest is
`/tmp/sotf-aud143-lr24-reset-after-audio-start.sha256`, SHA-256
`b8972995a0d59b37b0b80d90c35d2725275aeaa20eaedf3ce9f9c4823ee9c0d1`.
Root verified all nine selected BandSplit package paths and 59 math-iir-fir
paths against either unchanged current bytes or Git history:

- DAW commit `93027970f412ce47c0cd2b8e4b7b1a5b0e5f0261`.
- Math commit `9174ba7edd52de7349a141a9d173ed9d6b63edca`.
- Nine BandSplit paths: three unchanged, six recovered from Git.
- 59 math-iir-fir paths: 57 unchanged, LR4/LR8 recovered from Git.

All 68 files are preserved under
`crates/sotf-plugins/target/audit-artifacts/aud143-preedit-dsp-recovered/`.
The `source.tar.gz` hash is
`b499d74680b81b39522e16035f9ff26dc835f28095f8d6f21f8ce04d177bb4c9`;
`receipt.json` records every path, hash and recovery source. This is selected
DSP source preservation, not a transitive workspace or original CPU snapshot.
Three host files in the older manifest have changed and were not recovered;
they are listed in `/tmp/sotf-aud143-preedit-source-recoverability.json`.

## Reconstructed source and replay checkpoint

Luna constructed isolated `recovered` and `current` source trees under
`/tmp/sotf-aud143/`, retaining the same current dependency manifests/lock.
Root independently checked all 66 archived non-`Cargo.toml` files in the
recovered tree against the original receipt: all match and are regular files.
Both snapshot root lockfiles have SHA-256
`db9133949f6efa67544b407207feea40db05b4b2dd35f9964930415e5cfb1f18`.
Verification receipt: `/tmp/sotf-aud143/recovered-source-root-verification.json`.
The two current manifests are intentional comparison adapters, so this is
not an exact reconstruction of the original complete workspace.

The recovered release/offline/locked fixture replay passes 1/1, including
comparison of the complete captured waveform against the durable gzip fixture.
Root inspected `/tmp/sotf-aud143/recovered-legacy-fixture-replay.log`, SHA-256
`8a5564df25fc6e6274769bba63ea83a85910026593376044f0fce22b9db25144`.
Do not confuse this new run with the historical, similarly named
`/tmp/sotf-aud143-legacy-replay-checkpoint.log`.
The common benchmark compiled for both snapshots. Final executables were
copied to distinct per-variant paths and hashed before timing, resolving the
shared compilation-cache concern. Six direct runs completed in opposite
variant orders, retaining 1,890 raw samples. The durable report, source
archives, receipts, timing logs and scoped helper-lint result are linked from
`aud143-controlled-cpu-results.md`. Astra review is active; the limitations
below still apply, and this is not an original-QA or worst-case reproduction.

## Concrete next measurement

Luna should build an isolated comparison using the recovered old BandSplit
and LR4/LR8 implementations alongside current LegacyCascade and
PhaseCompensated implementations. Hold other dependency versions, compiler,
profile, harness, input, event schedule and machine conditions constant.
Use separate source copies and targets; never restore old files into the live
worktree. Capture the exact resolved source/lock/compiler setup for both runs.
Call this a reconstructed controlled comparison, not a reproduction of the
unbound original QA run.

Check legacy full-vector equality against the durable fixtures before trusting
the reconstructed baseline. Record any adapter needed for constructor/schema
differences; adapters must not change timed DSP behavior. Use the same setup
and processing lifecycle on all variants, preallocate input/output, keep output
observable, and verify finite/nontrivial audio outside timed regions.

At minimum compare the old 12-channel/four-band LR48 automated 512-frame case
and ordinary stereo two/four-band LR24/LR48. Include small/large callbacks and
both steady cutoffs and an identical absolute sample-timed automation schedule.
Separate construction/initialization from processing. Use warmups and repeated
trials; report distributions and measured variance, not one stopwatch result.
Run timing without concurrent Cargo builds and report system/compiler/profile
details. Do not infer a hardware realtime deadline guarantee from throughput.

Report current legacy versus recovered legacy, and compensated versus current
legacy, separately. The first examines cost changes to the old route; the
second quantifies the new feature. A cost increase must remain visible and be
assessed against the measured callback duration. Neither a passing allocator
test nor a preserved waveform is performance evidence by itself. Final Astra
review must inspect the actual comparison and its reconstruction limits.
