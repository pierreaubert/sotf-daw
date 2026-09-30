# AUD141: LR24 multiway recombination

Status: **bounded multiway LR24 correction accepted by Astra; package, stereo
host-chain, saved-audio replay and strict lint pass**. Updated 2026-09-30.

## Finding

`sotf-plugin-crossover/src/crossover_plugin.rs:113` describes multiway
recombination as the product of the split all-pass sums. The pre-edit loop at
lines 1163–1179 routes each band through lowpass whenever `band <= split`,
otherwise through highpass. For two split frequencies, the three outputs are:

```text
B0 = L0 L1
B1 = H0 L1
B2 = H0 H1
```

Their sum omits `L0 H1` from the claimed `(L0 + H0)(L1 + H1)` response.
The four-band case similarly omits terms. Initial construction accepts unique,
closely spaced frequencies; 1,000/1,200 Hz and 1,000/1,100/1,200 Hz are valid
at 48 kHz. No minimum spacing excludes these configurations.

This concerns the multiway IIR path. AUD074's accepted finite FIR drain and
the existing two-way/per-channel LR24 paths are separate.

## Independent analytical evidence

For each cutoff `fc`, use the analog LR24 prototypes and the prewarped bilinear
substitution, independently of production coefficient generation:

```text
s = j tan(pi f / Fs) / tan(pi fc / Fs)
L = 1 / (s^2 + sqrt(2) s + 1)^2
H = s^4 L
A = L + H
```

The primary derivation gives unit magnitude for the LR24 low/high sum, with
phase rotation; it is not sample-transparent identity.
[Linkwitz's crossover analysis](https://www.linkwitzlab.com/crossovers.htm)
provides the reference filter functions.

Evaluating the source topology at 48 kHz gives:

| Splits (Hz) | Probe (Hz) | Predicted summed magnitude | Predicted level |
| --- | --- | --- | --- |
| 1,000 / 1,200 | 1,100 | 0.83229510 | −1.594453 dB |
| 1,000 / 1,100 / 1,200 | 1,100 | 0.59046258 | −4.576152 dB |
| 1,000 / 1,001 | 1,000.5 | 0.75050093 | −2.492975 dB |

The independent all-pass product has unit magnitude to f64 rounding in each
case. These are analytical predictions from inspected routing, not measured
SOTF output. Probe receipt: `/tmp/sotf-aud141-multiway-analytic-probe.json`,
SHA-256 `472eeafa8c0e1df0e05dd01f59d688870dafe0012475abf51ad2a4e379077b72`.

The existing test
`tests/integration.rs:364::lr_multiway_recombination_is_allpass_at_every_test_frequency`
uses widely spaced splits 500/2,000/6,000 Hz, six probe frequencies and a 1%
RMS-ratio tolerance. The analytical topology's largest magnitude deviation at
those six probes is only 0.00877093, inside the tolerance. That passing test
therefore does not establish the stated all-pass property for supported
overlapping splits. Its finite-window RMS estimate also does not verify phase.

## Public plugin regression captured before production edits

Added the isolated test
`crates/sotf-plugins/crates/sotf-plugin-crossover/tests/aud141_recombination.rs`.
It constructs the public `CrossoverPlugin` with LR24 splits at 1,000/1,200 Hz,
feeds a 1,100 Hz coherent tone, discards 48,000 warm-up frames, and projects
each returned band over a coherent 9,600-frame measurement window. Its f64
reference uses the analog LR24 prototypes with the prewarped bilinear
substitution above; it does not reuse production coefficients or internal
filter state.

The public regression failed before any production edit, as expected. The
measured band transfers were
`[0.23776950−j0.00549375, 0.34853704−j0.00805307,
0.24576649−j0.00567852]`; the reference bands were
`[0.40542965−j0.00936760, 0.34853704−j0.00805307,
0.24576649−j0.00567852]`. The complex summed transfer measured
`0.83207302−j0.01922535` (magnitude `0.83229510`), against the expected
all-pass product `0.99973318−j0.02309920` (unit magnitude). The largest
band-vector error was band 0 at `0.16770490`; the other two branches match the
reference within f32 rounding. This is an actual public-plugin measurement,
not just the earlier analytic prediction.

Command:

```text
flock /tmp/sotf-daw-audit-cargo.lock env CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target TMPDIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target/audit-tmp CARGO_NET_OFFLINE=true cargo test --offline --locked -p sotf-plugin-crossover --test aud141_recombination -- --nocapture
```

The expected-red process exited 101. Log SHA-256:
`ab62754668190d09d7f2e803ec36feed1bb8457775444663c81f2e1b16901287` at
`/tmp/sotf-aud141-public-red.log`. The selected crossover production/test
manifest matched before and after (aggregate SHA-256
`ba6c1b53506fa5a9ea2f99c1b1e321fbd3f15ea081bc17a7213b67d9a8f6a7d9`);
the production source was then at the pre-edit hash listed below. That
checkpoint was test-only reproduction, before the correction described later.

The command used `Cargo.lock` SHA-256
`c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5` and
`sotf-plugin-crossover/Cargo.toml` SHA-256
`5f4d616c6fd4c2ee56ca24464ad7f7a09cff64dfddb097c5b23143460dea0f22`.
Shared host sources were held stable during the run and have these recorded
hashes: `sotf-host/src/plugin.rs`
`113ba4529a5bf1ea58bad10baae8b781855c96fcc8d3bfaf0d3a5f19d998f469`,
`sotf-host/src/host/daw_host.rs`
`b83336eb24729e6a9a30c70a717c003173c68c2b36875407895d80bbd09a1e1a`, and
`sotf-plugin-hal-output/src/lib.rs`
`7c4fcef345c98e7cea53f484f057e3aeac367e389d5574152a0535d77ff8d644`.

## Required reproduction and correction evidence

1. **Complete:** the expected-red public regression measures each band's
   complex response and the sum against the independent f64 reference. The
   exact command, result and hashes are recorded above.
2. Capture meaningful unchanged two-way, per-channel and FIR audio before the
   correction. Preserve existing IDs, ordering, modes, 16-sample smoothing
   cadence, structural contracts and FIR EOS behavior.
3. Specify multiway phase compensation explicitly. For three bands a suitable
   all-pass decomposition is `L0 A1`, `H0 L1`, `H0 H1`; generalization must
   cover every earlier branch through all later splits. Review the design
   before implementation rather than extending the wrong selection rule.
4. Validate three/four bands, close and wide spacing, multiple rates, distinct
   channels, low/high/both output selection, reset/reinitialization, unchanged
   input after invalid updates, and callback partitions through frequency
   automation. Include the actual split-to-merge host route.
5. Use an independent complex reference with magnitude and phase criteria.
   Check cold/repeated process and reset allocation and deallocation behavior.
   Do not infer comparative CPU cost without a paired benchmark. Recursive-tail
   policy remains open under AUD073; this correction cannot turn an IIR
   response into finite support.

## Implementation and verification checkpoint

The accepted correction is implemented in the existing multiway LR24 loop.
For each output band it now selects highpass before the band's split, lowpass
at that split, and low+high through later splits. This realizes the reviewed
all-pass decomposition without adding filter instances or state. The separate
two-way, per-channel and FIR branches remain outside the production hunk.

Before production editing, actual 12,288-frame stereo output arrays and the
original source copies were saved under
`crates/sotf-plugins/target/audit-baselines/aud141-preedit/`. The captured
controls are two-way LR24, per-channel LR24, four-way FIR and four-way LR24
final-high mode. Root independently decoded all four outputs and verified
finite, nonzero samples on both channels. Receipt:
`/tmp/sotf-aud141-root-control-check.json` (SHA-256
`87143a00197fc7b45ba7d92dedb0e8c65ed4e410431666439f0e058349d17982`). These
arrays preserve prior behavior; they are not response-accuracy references.

The original complex-response case and exact pre-edit replay passed after the
correction. The direct test measured summed magnitude `0.9999999990611071`
and input-normalized complex branch-transfer error `1.953707796e-9`; the separate pre-edit
arrays replayed bit-for-bit. Logs are
`/tmp/sotf-aud141-complex-response.log` and
`/tmp/sotf-aud141-baseline-replay.log`.

The expanded crossover package gate then passed: 55 library tests, 1
allocation/deallocation guard, 6 AUD141 response/mode/automation/state/reset
tests, 5 block-kernel tests, 4 finite-stream tests, 29 integration tests and
4 property tests. Two pre-edit capture/replay utilities remain ignored in
ordinary runs; the replay itself was explicitly executed. Gate log:
`/tmp/sotf-aud141-package-r2.log` (SHA-256
`b91f1bd41953beba653a044019cbb5edd2cbdf04c896eba1ae6f4997dd9c128d`). The
selected crossover source, tests, manifest and lock start/end snapshots are
identical (aggregate SHA-256
`6e1b8e791923a57d7f367f0a84461c18746f387c37a4c3fab249cb87e1f6c94b`).

At this checkpoint the new multi-rate response matrix, 3/4-band low/high
selection, absolute-frame automation partition comparison, rejected-update
non-mutation comparison, reset/reinitialization comparison and process/reset
allocation plus deallocation guard all pass. The actual stereo `DawHost`
Crossover→BandMerge chain, final baseline replay after all edits, and strict
Clippy are still pending. No CPU comparison is claimed. The reviewed bounded
design is recorded in `audit/proposals/crossover-multiway-recombination.md`;
implementation review and final AUD141 acceptance are pending.

## Final implementation gates

After three test-only lint corrections, the package again passes 104 tests
with two manual capture/replay utilities ignored. The explicit saved-array
replay passes separately, comparing all four preserved stereo controls
byte-for-byte. The actual `DawHost` Crossover → BandMerge `2→6→2` route passes
with distinct left/right tones, independent all-pass phase and magnitude,
complete measurement-window waveform and cross-channel leakage checks.

| Final gate | Result | Log and SHA-256 |
|---|---|---|
| Crossover package | 104 passed; 2 manual utilities ignored | `/tmp/sotf-aud141-package-accepted.log` — `0d91d9be8f6fea76b8301002c5a745286981300d3f89ea1f5c5dbb1e2b3d6397` |
| Explicit legacy replay | 1 passed, all four arrays byte-exact | `/tmp/sotf-aud141-replay-accepted.log` — `7e6bb9cc95ccae663e0d6fad1ac132a03765a87e0f0c950a35b3cab264ba83b8` |
| Stereo host chain | 1 passed | `/tmp/sotf-aud141-host-chain-final.log` — `24a377cf49186b35792ae8dba00bda404127515571ca74ea7a441e10d4ae8ac8` |
| Crossover all-target strict Clippy | Passed | `/tmp/sotf-aud141-clippy-pass.log` — `4df9ce141f99444bcc10ccea2972508785cdea0a02b3500d25566a9101bb438f` |
| Facade host-test strict Clippy | Passed | `/tmp/sotf-aud141-facade-clippy.log` — `e414c7c7c11121ae654b162abfbd6f10a9154147182f43a74b137a8bfb158c14` |

The filenames containing `accepted` identify successful execution checkpoints;
Astra subsequently accepted the implementation after the assertion refinement
below. Selected package and
crossover-Clippy manifests match at aggregate
`cb69595ccab212c041b2f15cca62692464fedcf56643941f8c38c362229bde33`.
The matching host-chain manifest hashes to
`0426eeaee0b5a9df334a82f695fa8a8c7b7dbd90e2a7c88ef43e63756647746c`;
facade lint uses `6bf4071667e4d14699a0bfd1cf560d29921b6b5333d431869b32e64a572a3c81`.
These are selected-file receipts, not whole-workspace snapshots. Current
crossover production source hashes to
`0c4357938dcf6b74141eee65416592b5c6db0b3a61a0efebcf9e8a742df28053`.

The 132-case direct matrix covers three/four bands, close/wide splits and
44.1/48/96 kHz. Maxima below are rounded values printed by the final tests:

| Measurement | Gate | Observed maximum |
|---|---:|---:|
| Direct branch complex transfer error | `2e-3` | `4.828e-8` |
| Direct summed complex transfer error | `2e-3` | `5.195e-8` |
| Direct branch fitted waveform residual / input RMS | `0.01` | `6.532e-8` |
| Direct sum fitted waveform residual / input RMS | `0.01` | `8.325e-8` |
| Host summed complex transfer error | `2e-3` | `5.910e-9` |
| Host independent waveform residual / input RMS | `2e-3` | `4.855e-8` |
| Host other-channel tone projection | `2e-4` | `1.419e-9` |

The direct fitted residual measures energy outside the fitted output tone;
the separate complex error compares its amplitude and phase with the
independent reference. Neither is the original complex branch-vector error.
The host waveform check reconstructs every measured sample from the
independent transfer and input projection. No CPU comparison or recursive IIR
tail completion is claimed. AUD142 filter choices and AUD143 BandSplit phase
compensation remain separate open work.

## Reviewer-requested automation assertions

Astra's implementation review found no production defect and requested one
test correction: check both complete automation output vectors for finite
samples and a conservative peak bound before the partition comparison uses
`f32::max`. Luna added these assertions with a predeclared peak ceiling of
`2.0`. The comment ties that ceiling to the fixture amplitudes `0.31` and
`0.23`; it exceeds six times the largest input amplitude and is not fitted to
the measured output.

The focused automation test passes 1/1, and crossover all-target Clippy with
warnings denied passes. Root independently checked both terminal logs and
their hashes:

| Gate | Log | SHA-256 |
|---|---|---|
| Focused automation test | `/tmp/sotf-aud141-automation-finite-bound.log` | `a02f1581fcc8d49e61bc200aa68c155fa4f490a9f435fc4e8ea4046351c89052` |
| Strict crossover lint | `/tmp/sotf-aud141-automation-clippy.log` | `8cb4a5c514618b15c8d4c12952a6bb82afe9f43f375339527ae3860bc9c7769f` |

The updated `tests/aud141_recombination.rs` hashes to
`7fb35ab37eb50685eba30ef8096bdfa80f1894ebe4afe5230bf4d9ef6013068c`.
Luna's matching start/end selected manifests have aggregate
`a73fd397d7d011f96c7956a4c116fdf6e5365fbb1f68896bd31c9a07cc007577`.
Production source is unchanged from the full package and host-chain gates
above. Astra verified the assertion delta and terminal receipts and accepted
the bounded implementation in `audit/reviews/AUD141-astra.md`. The separate
AUD142/AUD143 work, CPU comparisons and recursive-tail policy remain open.

## Pre-edit source provenance

The graph was rebuilding during exploration; all findings were checked against
current source slices and full-file hashes:

- `src/crossover_plugin.rs`: `ed8bc6a21aac4f95ef4a1d113ee5503b23de5101e084d364c2ccb0fdeaa8a121`.
- `tests/integration.rs`: `40638223f7fa6eed3e533754f870b9ae41650cf368d7aa83a13d81435bfcc2af`.

Paths are relative to `crates/sotf-plugins/crates/sotf-plugin-crossover/`.
The public regression is captured as an expected failure before production
changes. Astra accepted the bounded design proposal before production edits;
its acceptance is recorded in `audit/reviews/AUD141-astra.md`.
