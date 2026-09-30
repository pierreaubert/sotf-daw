# AUD143: phase compensation for multiband BandSplit

Status: **public LR24/LR48 accuracy reproduction and actual DawHost chain
confirmed; complete pre-edit waveform archives saved and checked. Astra accepted
the bounded design for implementation. Reset after audio processing now
reproduces six LR48 allocations; the LR24 control allocates nothing**. This is distinct from AUD141's incorrect Crossover branch
selection. No corrected production implementation is accepted yet. Review requirements are
recorded in [`AUD143-astra.md`](reviews/AUD143-astra.md).

## Current route and limitation

`sotf-plugin-band-split/README.md` explicitly documents unequal group delays
in cascaded three/four-band operation. It supports LR24 and LR48, whereas
the separate Crossover plugin currently exposes LR24 and FIR.

`src/lib/crossover_mode.rs` selects the shared math
`MultibandLr4Crossover<f32>` or `MultibandLr8Crossover<f32>`. The workspace's
`Cargo.toml:301` patch resolves these to the sibling `math-audio` checkout;
this finding therefore checks that actual dependency, not an assumed git copy.
Both multiband implementations split the highpass carry at successive
frequencies without applying later all-pass sections to earlier low bands.

For three stationary LR24 bands, with `A_i = L_i + H_i`, the current sum is
`L0 + H0*A1`. The phase-compensated decomposition would be
`L0*A1 + H0*L1 + H0*H1 = A0*A1`. These differ even with all band gains at
unity. The equations describe fixed-cutoff responses; automation requires
separate time-varying checks.

An independent calculation using
`s = j*tan(pi*f/Fs)/tan(pi*fc/Fs)`,
`L = 1/(s*s + sqrt(2)*s + 1)^2`, and `H = s^4*L` predicts:

| LR24 cuts at 48 kHz | Probe | Current cascade summed magnitude | Gain |
|---|---|---|---|
| 1,000 / 1,200 Hz | 1,100 Hz | 0.2240978791 | −12.991245 dB |
| 500 / 2,000 Hz | 1,000 Hz | 0.9470834447 | −0.472235 dB |
| 1,000 / 1,100 / 1,200 Hz | 1,100 Hz | 0.4031389728 | −7.890904 dB |

These are analytical predictions, **not measured SOTF output**. No LR48
numerical accuracy claim is made by this LR24 calculation.

The independent LR48 prediction uses the squared fourth-order Butterworth
section. With `q1 = 1/(2 sin(pi/8))`, `q2 = 1/(2 sin(3pi/8))`,
`B4(s) = (s*s + s/q1 + 1)(s*s + s/q2 + 1)`, the reference is
`L = 1/B4(s)^2`, `H = s^8/B4(s)^2`, using the same bilinear prewarp. For
three bands with cuts 1,000/1,200 Hz at 1,100 Hz, this predicts the legacy
complex sum `0.9247620148 - j0.2439868786`, magnitude `0.9563965`
(`-0.386 dB`, rounded). This is derived separately from the LR24 formula.

## Public pre-edit reproduction

The new public-plugin capture test drives two distinct stereo sine channels at
48 kHz and 1,100 Hz (left amplitude 0.31 / phase 0.15 rad; right amplitude
0.23 / phase 1.05 rad). It warms each plugin for 12,000 frames and measures
24,000 coherent frames. The independent reference compares each band and the
sum as complex responses, with a 0.002 absolute complex tolerance. The
predeclared unity-gain sum gate is magnitude `1.0 ± 0.005`; the two-band
controls pass and the representative close-cut three-band cases fail:

| Path / slope | Bands / cuts | Probe | Expected complex sum | Measured complex sum (L; R) | Magnitude |
|---|---|---:|---|---|---:|
| Public BandSplit LR24 | 3 / 1,000, 1,200 Hz | 1,100 Hz | `0.2033717989 + j0.0941263557` | `0.2033730188 + j0.0941286610`; `0.2033725524 + j0.0941284217` | 0.22409995 |
| Public BandSplit LR48 | 3 / 1,000, 1,200 Hz | 1,100 Hz | `0.9247620148 - j0.2439868786` | `0.9247459614 - j0.2440066586`; `0.9247456632 - j0.2440079345` | 0.95639654 |
| DawHost split → merge LR24 | 3 / 1,000, 1,200 Hz | 1,100 Hz | `0.2033717989 + j0.0941263557` | `0.2033730186 + j0.0941286609`; `0.2033725523 + j0.0941284214` | 0.22409995 |
| DawHost split → merge LR48 | 3 / 1,000, 1,200 Hz | 1,100 Hz | `0.9247620148 - j0.2439868786` | `0.9247459605 - j0.2440066598`; `0.9247456632 - j0.2440079346` | 0.95639654 |

The public test also captures per-band complex responses for two-, three-, and
four-band close/wide cutoff cases with both slopes; all matched the independent
legacy cascade reference within the 0.002 complex tolerance. The baseline
matrix uses 2-band `[1000]`, 3-band `[1000,1200]` and `[500,2000]`, 4-band
`[1000,1100,1200]` and `[250,1000,4000]` cuts. Passing finite/nonzero and
stereo-distinct checks are smoke results only; they are not unity-recombination
acceptance. Separate ignored tests were run explicitly as expected-red gates
for LR24 and LR48. Their exit status and full waveform archive provenance are
recorded below after the final pre-edit capture command.

The actual `DawHost` integration test appends public `BandSplitPlugin`
(6-channel output), then `BandMergePlugin` (stereo output), and measures the
result. It matches the same independent complex reference and fails the same
unity gate. This closes the host-route reproduction; direct calls to the DSP
alone would not establish this.

## Reset allocation observation

The original public reset probe uses the facade's existing `CountingAlloc`
and `assert_no_allocs` harness around `BandSplitPlugin::reset()` after
initializing a 4-band LR24 or LR48 instance at 48 kHz. It did not process audio
before reset. The initialized LR24 control passes with zero allocations; the
explicit LR48 gate exits 101 and reports six allocations. These results prove
the initialized-reset behavior only. Source inspection shows LR8 reset
reconstructs `Self::new` and its `Vec`s, while LR4 resets in place.

Root identified the missing nonzero-state preparation. Luna has refined the
fixture to process 4,096 frames of distinct three-tone stereo audio and verify
all eight band-major outputs are finite/nonzero before entering the reset
allocation guard. Root executed that refined pair under one Cargo flock. LR24 passes with zero
allocations; LR48 reaches the intended allocation assertion and exits 101,
reporting six allocations after audio processing. Both commands have matching
305-file selected-source/lock manifests at
`b8972995a0d59b37b0b80d90c35d2725275aeaa20eaedf3ce9f9c4823ee9c0d1`.
These are allocation counts; deallocation-free behavior remains an explicit
implementation acceptance requirement.

BandSplit inherits `Plugin::tail_length() == TailLength::Unknown`; preserve
that report unless evidence supports a more precise contract. The reset finding
is scoped to BandSplit's public path and does not establish every shared-math
consumer's behavior.

## Typed application route is currently two-band only

The DSP's multiband API does not establish an equivalent application route.
Engine `PluginSettings::BandSplit` (`plugin_settings.rs:1232–1242`) contains
only channels, one frequency and crossover type. `convert_band_split`
(`plugin_config_converter/spatial.rs:372–389`) emits one frequency and type;
`plugins/chain/plugin_chain.rs:866` consequently propagates twice the channel
count. This is internally consistent for its current two-band settings, but
cannot persist or select the DSP's three/four-band configuration through that
typed route. Include the required settings, conversion, control and channel
propagation work when exposing phase-compensated multiband splitting. A raw
factory or direct-DSP test alone cannot close the application feature gap.

## Required work

1. Reproduce the current LR24/LR48 behavior through public BandSplit and
   through an actual DawHost BandSplit → BandMerge chain, using independent
   complex per-band and summed references and distinct input channels.
2. Design phase compensation with an explicit compatibility decision for the
   documented existing cascade. Capture meaningful legacy vectors before
   changing behavior. Avoid silently changing every shared-math consumer.
3. Cover two through four bands, close/wide cutoffs, supported rates/channel
   layouts, band gain and frequency automation, partition invariance, reset,
   initialization and rejected updates. Preserve band-major order, structural
   slope selection, bounded coefficient updates and truthful recursive tails.
4. Check cold/repeated processing and reset allocation behavior, actual CPU
   cost, state/preset/control routes and independent response accuracy.
5. Implement with Luna xhigh and validate with Astra medium after AUD141 and
   the current native/HAL checkpoints.

The existing `plugin_high_channel_tests.rs:295` split/merge case exercises
BandSplit directly, checks finite/nonzero output and widths, and does not
establish unity reconstruction or actual DawHost routing. BandMerge's
`reconstruction_error_db` compares its output against its own input-band sum;
it cannot detect a defect introduced before those bands reached the merger.

## Source provenance

Inspected 2026-09-29. The math graph was one commit behind; findings above
were verified against current file contents rather than trusting graph lines.
SHA-256 values for those files:

| File | SHA-256 |
|---|---|
| `sotf-plugin-band-split/README.md` | `b71d2d66c875234dca4db7819ce93b41d9215ef090d64ee1209a0e564d0be5f2` |
| `sotf-plugin-band-split/src/lib/crossover_mode.rs` | `ddf0d37c5c5b81f516778b58a89509940e108a11d24722d1a01354248b942384` |
| `sotf-plugin-band-split/src/lib/band_split_plugin.rs` | `7b5882d3be36fb5599780ab4800ebf1d1dba217de07fc367758a89c50af70459` |
| `../math-audio/crates/math-iir-fir/src/lr4_crossover.rs` | `ae7b04cb4ee9909c614e5fd89a7eb39f2b90345de5a83c96c0e0b9096d8ff088` |
| `../math-audio/crates/math-iir-fir/src/lr8_crossover.rs` | `bb76e344afd9e709430a6fec555743a921bda5c36586e5a7c8b76910f7edb7e8` |

The first three paths are relative to `crates/sotf-plugins/crates/`.

## Capture artifacts and exact run provenance

The opt-in pre-edit captures are stored under the ignored Cargo target tree at
`crates/sotf-plugins/target/audit-baselines/aud143/`. They contain the full
stereo input and every output f32 sample, including warmup, in a versioned
little-endian binary format; the tests also print measured complex branch and
sum responses. No production BandSplit source has changed.

| Saved archive | Bytes | SHA-256 |
|---|---:|---|
| `split-waveforms.bin` | 9,504,620 | `73b30f81f14464e87fc6982ddcd5c2e3c618ca41486a95ec9b840a3debc16ebc` |
| `host-waveforms.bin` | 864,126 | `e9efc5c52975d07357bde1e36dae67a6ac471b42cd6f8d0508543799783923bc` |

Root independently parsed both archives to exact end-of-file, checked their
headers and all ten split records, identical stereo inputs, and finite/nonzero
sample arrays. Across all 36,000 frames including startup, summed direct bands
agree with actual host output to maximum absolute error
`1.1175870895385742e-8` for LR24 and `2.2351741790771484e-8` for LR48.
Receipt: `/tmp/sotf-aud143-root-artifact-check.json`. This establishes archive
integrity and legacy route consistency, not corrected phase compensation.

Terminal logs, all under `/tmp/`:

| Log | Result | SHA-256 |
|---|---|---|
| `sotf-aud143-public-capture.log` | Exit 0; one test covers ten legacy cases | `b75227d084bc9b1ea3cd07b3f2e946d5902fefb6f79ff95c197e7d964eb84a6a` |
| `sotf-aud143-host-capture.log` | Exit 0; one test covers both host slopes | `c80b806fa08206b98bde96df6c0d4e29f9a28ad618cd6656f79f17438e4b8270` |
| `sotf-aud143-lr24-reset.log` | Exit 0; initialized-reset control | `4a02d93648a77c1463b89d48bf04524df18c93015c2b4d5c6482179f07005eea` |
| `sotf-aud143-lr48-reset-red.log` | Expected exit 101; six allocations in initialized reset | `fc96153b67d1aaac242e5f8c3000271e7ad82a6ba2326551c860fcceafeeb474` |
| `sotf-aud143-lr24-reset-after-audio.log` | Exit 0; reset after nonzero processing, zero allocations | `c4c683dc7f9ade9ce38705da48cadc305742833effd146613d06e0dd1a9b5ccb` |
| `sotf-aud143-lr48-reset-after-audio.log` | Expected exit 101; six allocations after nonzero processing | `01cb5eda0318943dcb71f378acd3fbc535605f0e8c3a710b8226772ee67cad08` |

Each command used the shared Cargo flock, offline/locked resolution and the
warm `crates/sotf-plugins/target` tree. The captures select
`captures_public_legacy_complex_controls_before_phase_changes` in package
`sotf-plugin-band-split`, target `aud143_phase_capture`, and
`dawhost_band_split_merge_chain_matches_legacy_complex_capture` in package
`sotf-plugins`, target `aud143_bandsplit_host_chain`. The opt-in output variables
are `SOTF_AUD143_SPLIT_CAPTURE_PATH` and `SOTF_AUD143_HOST_CAPTURE_PATH`.
Preserve these original files; any later capture must use separate paths.

The recorded BandSplit and LR4/LR8 production hashes above were rechecked
unchanged on 2026-09-30. Cargo.lock SHA-256 is
`c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`.
Run-bound start/end source manifests were not retained for these original
capture commands; the current hash recheck must not be described as one or as
a whole-workspace gate. Other agents continue modifying native/HAL host files.

Executed refined reset commands, with the same Cargo environment and flock:

```sh
cargo test --offline --locked -p sotf-plugins --test aud143_bandsplit_host_chain aud143_populated_lr24_reset_has_zero_allocations_control -- --exact --nocapture
cargo test --offline --locked -p sotf-plugins --test aud143_bandsplit_host_chain aud143_populated_lr48_reset_is_allocation_free_gate -- --ignored --exact --nocapture
```

The second command reaches its expected behavioral failure before the fix.
Each refined log has sibling `-receipt.json`, `-start.sha256` and `-end.sha256`
files under `/tmp/`. Combined receipt:
`/tmp/sotf-aud143-reset-after-audio-summary.json`. The first command also
establishes a successful default-feature core host/facade compile after the
concurrent HAL typed-error correction; it does not compile the HAL plugin or
prove the feature-enabled native backends.

## Design proposal

The correction, compatibility default, route propagation, and acceptance
matrix are specified in
[`audit/proposals/band-split-phase-compensation.md`](proposals/band-split-phase-compensation.md).
