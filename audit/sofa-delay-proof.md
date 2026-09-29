# AUD109 — public SOFA integer-delay loss: executed proof and local adapter scope

2026-09-28. Independent same-rate public API probe. No production, permanent-test,
manifest, sibling-checkout or Cargo changes. Rustc used already-built compatible
libraries. The probe generated only tiny temporary SOFA fixtures.

## Reproduction artifacts

- Source: `/tmp/sotf-sofa-delay-probe.rs`.
- Executable: `target/audit-tmp/sotf-sofa-delay-probe`.
- Files: `target/audit-tmp/aud109-sofa-delay/`.
- Build log: `/tmp/sotf-sofa-delay-probe-build.log`.
- Final complete measurements: `/tmp/sotf-sofa-delay-probe-final.log`.
- Earlier partial fixture run: `/tmp/sotf-sofa-delay-probe.log`.

Pinned reader: `sofa-reader` 0.2, revision
`6e6e9d9ce2b02525ae6f6f04b4639bc264a1fc2f`. The fixture uses its low-level
`SofaWriter`, not the frequency-domain convenience writer. Files contain f64
FIR samples, sampling rate, source/receiver positions, and delay variables.
Dimensions are M=3, R=2, N=16, I=1, C=3; rate is 48000 Hz. Source azimuths
+30/-30/0 degrees exactly match the tested speaker directions. Ear tap amplitudes
are [1,0.2], [0.2,1], and [0.6,0.4].

Compiled without Cargo:

```sh
TMPDIR=/home/pierre/src/all_of_sotf/sotf-daw/target/audit-tmp rustc --edition=2024 -C opt-level=1 /tmp/sotf-sofa-delay-probe.rs -L dependency=target/debug/deps --extern sotf_host=target/debug/deps/libsotf_host-2fefcc4bacc7f28b.rlib --extern sotf_plugin_binaural=target/debug/deps/libsotf_plugin_binaural-914357a854e7d50f.rlib --extern sotf_plugin_xtc=target/debug/deps/libsotf_plugin_xtc-f8a6e7ddc30056cd.rlib --extern sofa_reader=target/debug/deps/libsofa_reader-a769c7065b091c4f.rlib -o target/audit-tmp/sotf-sofa-delay-probe
target/audit-tmp/sotf-sofa-delay-probe target/audit-tmp/aud109-sofa-delay
```

Final probe exits successfully after asserting the measured *current defect*
and all positive/negative controls. It is a red-proof harness, not a passing
acceptance test for corrected production.

## Independent loader and DFT evidence

Two pairs of physically equivalent fixtures:

1. Shared Data.Delay shape [1,2], values [0,3], stored taps at zero; twin has
   explicit zero delay and right-ear taps moved to index3.
2. Per-measurement shape [3,2], rows [0,3], [2,7], [1,5]; twin has explicit zero
   delay and each stored tap moved to its specified index. M=3 makes receiver/
   measurement transposition observable.

Low-level `Hdf5File::dataset_dims` and `SofaReader::read_f64` return these exact
shapes and values. However, `SofaFile::load` produces raw IRs bit-identical to
an all-zero-delay baseline in both layouts. No delayed samples are materialized.

The independent f64 DFT of each loaded ear at bin7 of a64-point transform is
compared with the analytic single-tap response `A exp(-j 2 pi 7 d/64)`.
All 12 manually shifted twin responses agree exactly in this calculation.
Eight nonzero-delay entries fail for metadata-only files:

| Layout / measurement / ear | Delay | Complex magnitude error |
|---|---:|---:|
| Shared / 0 / R | 3 | 0.343091449 |
| Shared / 1 / R | 3 | 1.715457220 |
| Shared / 2 / R | 3 | 0.686182898 |
| MR / 0 / R | 3 | 0.343091449 |
| MR / 1 / L | 2 | 0.253757317 |
| MR / 1 / R | 7 | 1.343117910 |
| MR / 2 / L | 1 | 0.404267840 |
| MR / 2 / R | 5 | 0.791341220 |

Zero-delay entries remain exact. Expected values come only from tap amplitude,
integer shifting and the DFT shift theorem; no production FFT/interpolation or
onset detector generates the oracle.

## Public Binaural process plus drain

N=128, room reflection order0, externalization/near-field0, diffuse EQ disabled.
Stereo tests query +30/-30; mono tests query0. The original trial incorrectly
used the third lane of a three-channel configuration, which is LFE (2.1), and
was corrected to the public mono center route. No production fault is inferred
from that fixture mistake.

The source has 73 frames with markers +0.03125 at0 and -0.015625 at72. Public
callbacks use [1,17,127] subdivision, followed by ordinary public finite drain.
Declared scheduler latency is128 frames. For each ear, independently expected
marker indices are128+d and200+d.

Across both layouts, all 24 manually shifted twin marker peaks occur at exactly
those indices. Metadata-only files instead put every first/final peak at128/200,
up to seven samples early. Metadata-only output is also bit-identical to the
zero-delay baseline. Example MR row [2,7]: actual first peaks128/128 versus
expected130/135, actual final peaks200/200 versus expected202/207.

Maximum absolute full-waveform metadata/twin errors by speaker query:

| Layout | +30 | -30 | Center |
|---|---:|---:|---:|
| Shared | 0.004947916 | 0.024739582 | 0.012500000 |
| MR | 0.004947916 | 0.024739582 | 0.018750001 |

## Public XTC process plus drain

N=128, measured-HRTF mode, AutoGain and spectral normalization disabled,
otherwise existing defaults. Same quiet first/final markers, one selected
stereo source at a time, same callback schedule and complete public drain
(320 emitted frames per probe).

Metadata-only files are again bit-identical to the zero-delay baseline. The
manually shifted twins change actual public output:

| Layout / selected desired source | Maximum waveform difference |
|---|---:|
| Shared / source0 | 0.000000001 |
| Shared / source1 | 0.018320596 |
| MR / source0 | 0.003247987 |
| MR / source1 | 0.016423773 |

The near-zero shared/source0 result is a valid independent negative control:
shared receiver delays multiply plant rows, `C=D C0`; the inverse is
`C0^-1 D^-1`. With first receiver delay0, its first inverse column is unchanged.
It would be incorrect to demand every selected source change for that shared
fixture. MR delays break that common-row structure and both source routes
change. The final probe asserts this distinction.

XTC's private prepared plant arrays were not accessed; its result is a public
integration equivalence proof. The separate loader DFT calculation establishes
the exact missing phase without relying on XTC inversion as an oracle.

## Meaningful zero-delay control

An absent Data.Delay file and an otherwise identical explicit shared-zero file
produce exactly equal high-level raw IRs, all three public Binaural direction
waveforms, and both public XTC source waveforms. This verifies that file loading,
query mapping, and plugin processing operate on the generated fixtures and that
the observed failure is specific to nonzero delay metadata.

## Minimal local adapter proposal — no implementation yet

A single control-side FIR loading/normalization helper in `sotf-host/src/sofa.rs`
can return the existing public SofaFile type. Binaural's synchronous/background
loads and XTC's HRTF load should use it before existing prepared-state adoption.
No callback delay line, host queue, public SofaFile field, dependency revision,
or read-only sibling change is required for the measured integer cases.

- Read source bytes once and create a low-level Hdf5File from that immutable
  buffer. Do not call the old path loader and separately reopen metadata, which
  could combine two file versions during replacement.
- Preserve the existing supported coordinate conversion and required metadata
  behavior while constructing the public fields. The pinned high-level loader
  is about130 lines and currently reads only raw IR/position/rate data; its
  path-only constructor cannot attach metadata from the same parsed bytes.
- Check Data.Delay's actual [1,2] or [M,2] shape and broadcast/index receiver rows
  correctly. For the reviewed same-rate nonnegative integer cases, prepend exact
  zero samples per IR and pad all ears/measurements to a checked common length.
  Missing or all-zero metadata must preserve samples and original IR length.
- Validate checked dimensions, delayed storage/support and declared sampling
  rate before preparing/publishing filters. Existing Binaural linear-convolution
  capacity must see the enlarged IR. XTC must not silently truncate newly
  delayed support beyond its FFT. Keep preparation failure transactional.
- Route old SQLite caches through their existing loader; they have no source
  delay metadata to recover. Cache format/migration remains a separate concern.

This proof establishes only same-rate integer delay. It does not choose a
fractional/negative-delay policy, validate nonconstant rate vectors, prove
cross-rate resampling accuracy, or authorize those broader changes. Those
boundaries require the parent review requested in `audit/proposals/sofa-delay.md`.
No production implementation is included here.

## Artifact identity at execution

Source SHA256 `d3a513d0777c233a4da40bee3cd40d846d4e22e941cc59b2869b420e9a5d5605`.
Library SHA256 values:

- host2fefcc4bacc7f28b: `c2e32682068952ecbdd6e4fb51a4c6054371b93982dd748a0f2e75b717badb76`
- Binaural914357a854e7d50f: `d821139c59e1b8ea3042edd20f94b5f35719c673c2ba4958e921af7eaefc0bf8`
- XTCf8a6e7ddc30056cd: `250e38ee6f7bf67327935e2340f32b873dfd5d71be8871ba7cd0d0650ea56f74`
- sofa-readera769c7065b091c4f: `933ef853e12c61b80e7362aff31c17a80b5a382597c2770baf86453e4f45da64`
