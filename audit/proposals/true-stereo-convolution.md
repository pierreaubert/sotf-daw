# AUD-134: true-stereo convolution proposal

**Status:** Revised proposal for Astra review; no production behavior change has been made.

## Finding and scope

`AUDIT.md` records diagonal IR routing as a verified convolution gap. The
implementation decodes and retains all IR channels, but the UPC path chooses
one IR channel for each output and convolves only the same-numbered input. The
NUPC loader likewise creates one engine per output channel. For a two-channel
plugin and a four-channel IR, paths 2 and 3 are consequently never used. This
is reachable from the public API today, so it is a silent routing omission
rather than an unavailable file format.

This proposal adds an explicit structural `true_stereo` option. It defaults to
`false` for source, serialized-preset, and factory compatibility. When enabled,
it is valid only for a two-channel plugin with a four-channel IR. The four IR
channels are ordered by source then destination:

| IR channel | Path |
|---|---|
| 0 | left input to left output (LL) |
| 1 | left input to right output (LR) |
| 2 | right input to left output (RL) |
| 3 | right input to right output (RR) |

Thus `wet_L = conv(input_L, LL) + conv(input_R, RL)` and
`wet_R = conv(input_L, LR) + conv(input_R, RR)`. This is the order documented
for true-stereo impulse responses in the [Yamaha VST Rack plug-in reference
manual](https://manual.yamaha.com/pa/software/vstrack/pr/en/11_reverb_plugins_en.html).

With `true_stereo = false`, all routing cases keep their exact current
behavior: mono IR broadcast, matching/cyclic per-output IR mapping, and
non-stereo layouts. In particular, a stereo plugin currently uses IR channel
0 for output left and channel 1 for output right from a four-channel file;
channels 2 and 3 are unused. This legacy four-channel behavior is captured
before edits below. Old serialized presets that omit the new option continue
to select it by default.

## Public route and preset behavior

Add a structural boolean `true_stereo` parameter with default `false` to the
plugin parameter schema and serialized `Params`; expose it beside the
partitioning/latency controls in the Advanced tab. It is structural because
enabling the matrix changes the prepared engine topology, like the existing
NUPC and direct-head options. Keep the existing public
`ConvolutionPluginParams` struct and `from_params` constructor source
compatible; add a public `from_params_with_routing` entry point for callers
that opt in. The factory reads the optional `true_stereo` boolean from its
existing JSON state, defaults it to false, and calls that entry point. This
keeps old source callers and presets on legacy routing while making the mode
available through the host's existing parameter/preset route.

The four-channel file itself remains selected through the existing public
`ir_file` setup parameter and file picker. A `true_stereo` preset then enables
the matrix when the host constructs the plugin. Tests must assert the
`true_stereo` schema entry is a structural toggle in the Advanced control list
and reconstruct an actual plugin through factory JSON persisted state. A bad
or mismatched IR load in explicit true-stereo mode must fail without replacing
the last-known-good matrix. Explicit true-stereo mode on a plugin width other
than two is a construction error, even without an IR. Width two with the mode
enabled and no IR remains a valid neutral/delayed-dry state; subsequently
loading an IR other than four channels fails and leaves the plugin unloaded or
keeps its last-known-good matrix. Non-boolean factory JSON for the setting is
an error, rather than silently coercing to false.

## Implementation outline

- Carry the explicitly selected routing flag through synchronous and
  asynchronous load requests. Reject true-stereo mode unless the prepared
  plugin has two channels and the IR has exactly four channels.
- UPC keeps its two input spectra/FDL lanes. For each output and partition,
  multiply-accumulate the corresponding two source lanes against the two
  matrix paths, then feed the existing overlap-add/output ring. Keep dry/wet,
  gain envelopes, 1024-frame latency, and EOS flow unchanged.
- NUPC prepares all four path kernels and instantiates one stateful engine per
  path. Feed each input sample to its two outgoing path engines and sum their
  outputs into the destination channel. Preserve the existing backend and
  direct-head latency contract.
- Keep all IR preparation, FFT planning and vector allocation off the audio
  thread. For the opt-in NUPC matrix, estimate four independent engine states,
  including each path's IR spectra, FDL, FFT scratch, overlap/input/output
  buffers, output delay, and optional head history. Keep the 512 MiB realtime
  backend budget and reject an over-budget matrix before preparing engines.
  Tests will prove four engines are installed for a valid matrix and exercise
  both sides of the estimate's budget boundary.
- Leave load failure/replacement, reset, tail bound and retained-state
  reclamation rules intact. The longest of the four paths already defines the
  IR length bound.

The `true_stereo` parameter requires a small factory JSON read so it reaches
the source-compatible opt-in constructor. Coordinate ownership for that
factory source and its test with the audit lead; do not expand into other host
or UI modules.

## Pre-edit baseline

The full convolution crate source was copied before adding the capture probe
from clean HEAD `93027970f412ce47c0cd2b8e4b7b1a5b0e5f0261` to
`crates/sotf-plugins/target/audit-artifacts/aud134-preedit-9302797/source-snapshot/`.
The source-plus-lock manifest is `/tmp/sotf-aud134-preedit-source.sha256`,
aggregate SHA-256
`50b447849c8b8145fa33b7ad9bc4600aa4b433eaa174138884e242d5f93fffe1`;
`Cargo.lock` SHA-256 is
`c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`.

An ignored capture test rendered actual public plugin output for two-channel
IRs using irregular input partitions and native EOS drain. Binary output is
under `crates/sotf-plugins/target/audit-artifacts/aud134-preedit-9302797/render-baseline/`:

| Backend | Frames | Latency | Binary SHA-256 |
|---|---:|---:|---|
| UPC | 5,127 | 1,024 | `86a45b6f39b7006ff4f97438c1c01c3b0d609b53187c5b83b5f2dd08488db982` |
| NUPC | 5,127 | 1,024 | `9a67b379f9a3c9be85ee811cb9b5053668ab947f954001d40bccef6fc098fa61` |
| NUPC with zero-latency head | 4,103 | 0 | `d80ddfb5c156164fab4443bc7d4f02b89196dd886fab441929d56ef8c779686b` |

Capture log: `/tmp/sotf-aud134-preedit-capture.log`, SHA-256
`684007e2614032406b72ce3d383ebbffab5b7fd11f1fc008e6e3f4ac014a02ea`.
Command: `cargo test --offline --locked -p sotf-plugin-convolution --test
direct_convolution capture_aud134_preedit_diagonal_stereo_outputs --
--ignored --nocapture`, with `CARGO_TARGET_DIR` set to
`crates/sotf-plugins/target` and the output directory supplied through
`SOTF_AUD134_CAPTURE_DIR`. Production source was still exactly the captured
clean version during this render.

The four-channel legacy capture was added as an ignored test probe without
changing plugin implementation. Source/lock manifests were identical before
and after the focused run. Arrays are under
`crates/sotf-plugins/target/audit-artifacts/aud134-preedit-9302797/legacy-four-channel-baseline/`:

| Backend | Frames | Latency | Binary SHA-256 |
|---|---:|---:|---|
| UPC | 5,127 | 1,024 | `2140300bda21451d462124cc3a74d4da8d1dfc07553789344a5a35b7edae1a18` |
| NUPC | 5,127 | 1,024 | `3101a548ce7591db588ee05fb0d031ea0cd42af870a61bfabfe4ea7716573b7a` |
| NUPC with zero-latency head | 4,103 | 0 | `06c6eef630312951eaa77d0d1c49115fb10dbcc079b50e372492d35948f1be00` |

This records exact legacy behavior: channel 0 feeds output left, channel 1
feeds output right, and the distinctive channels 2/3 are not used. Capture log
`/tmp/sotf-aud134-preedit-fourch-capture.log`, SHA-256
`f938516dd9b9af08c83c0b68cc91b6faf5aa1c10992cfd1e16327e3aebf31a50`.
Before/after crate source manifests
`/tmp/sotf-aud134-preedit-fourch-source-before.sha256` and
`/tmp/sotf-aud134-preedit-fourch-source-after.sha256` match (manifest-file
SHA-256 `4d0f3fb7c71b8384c69d30d3bc3a893fc23d461627cffababee12d67d1efd8de`);
the Cargo.lock manifests also match.

## Independent acceptance oracles

Use deterministic but distinct dense and sparse IR paths so channel swaps,
omitted crossfeed, accidental mono summing, and path aliasing all fail. The
reference is a separate f64 time-domain matrix convolution. Predeclare
`max(abs(error)) <= 1e-5` and `RMS(error) <= 1e-6` over the complete rendered
vector after including declared latency, gain, mix, and EOS samples. For source index
`i` and destination index `o`, IR path index is `2*i + o`; expected wet output
is summed over both input sources. Compare the complete shifted output vector,
not only peak positions.

Acceptance evidence should include:

1. Four one-path isolation cases: exercise LL, LR, RL, and RR separately with
   impulses on only the corresponding source channel; assert the other output
   is silent where the matrix says it should be.
2. Dense full-matrix cases against the independent f64 oracle for UPC, NUPC,
   and NUPC with direct head. Include distinct signed taps at frame 0, around
   the 1024 partition boundary, and at the last response frame.
3. Input callback sizes covering one frame, irregular sizes, a full 1024-frame
   partition, and multiple partitions. Require partition-invariant rendered
   output and identical EOS tail samples.
4. Exact latency checks: 1,024 frames for UPC and normal NUPC, zero additional
   frames for NUPC with direct head. Verify output frame count is
   `input_frames + latency + longest_ir_frames - 1`, plus a final-tap and
   post-tail silence check.
5. Exact equality to both captured old two-channel and old four-channel stereo
   outputs with `true_stereo = false`, plus mono broadcast and non-stereo
   cyclic routing regression coverage.
6. Constructor and factory JSON routes with legacy `false` default, explicit
   opt-in persistence, and matrix → legacy → matrix reconstruction. In matrix
   mode, test a good matrix, a mismatched/invalid replacement that fails while
   retaining the active response, then a valid replacement and reset.
7. A 44.1 kHz four-path fixture resampled to 48 kHz. A test-only, separately
   invoked Rubato `Fft::new_custom` whole-clip reference with the declared
   Blackman-Harris 2 window and fixed-input sync is plumbing evidence only; it
   uses the same resampler library and is not an independent resampler
   algorithm. Compare every reference-resampled path through the full f64
   matrix convolution oracle.
8. Audio callback **and native drain** allocation/deallocation evidence for
   UPC and NUPC matrix processing. NUPC engine-count and checked-memory-boundary
   tests must account for all four prepared paths.

The zero-latency-head full oracle should characterize its existing head/tail
transfer, not assume a generic delay. Any NUPC or resampling mismatch must be
reported with explicit error bounds and corrected before acceptance; do not
relax a bound to make the candidate pass.

## Exclusions

This batch does not add arbitrary matrix sizes, channel-layout conversion,
Ambisonics decoding, separate per-path gain controls, fractional-delay
alignment, or UI for editing path matrices. MIDI and IAMF remain out of scope.
