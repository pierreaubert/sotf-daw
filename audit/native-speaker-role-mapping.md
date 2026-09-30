# AUD135 named speaker role mapping

Status: source/specification research, 2026-09-29. Astra independently verified
all eight role arrays, masks, permutations and CLAP maps in snapshot
`714341ef697f09c182b1c28def73ded293c955a3eb6cfb6c7d1849d93eef74fb`.
No table corrections were required. Native implementation, wide-target CLAP
policy and setup/state ownership still require their separate checks; no Rust
changes or native execution are claimed by this note.

## Local source

[SpeakerConfig definitions](../crates/sotf-plugins/crates/sotf-host/src/speaker_config/consts.rs)
at lines 866–1634 define these eight layouts. Inspected full-file SHA-256:
`cedba01402f1b76947b02ab35ad31c383a697e8629c732739855aabd7c2bbd7c`.
The rows below were extracted in numeric `channel` order.

SOTF fronts are ±30°, sides ±110° in the 5.1 family and ±90° with separate
backs, backs ±150°, and wides ±60°. Heights use 45° elevation; the two-height
layouts use **front** heights. TMiL/TMiR are ±90° top-middle positions. Preserve
these physical roles when selecting an adapter arrangement.

## Proposed VST3 mapping

Masks and bus order are derived from the official
[speaker definitions and index helpers](https://raw.githubusercontent.com/steinbergmedia/vst3_pluginterfaces/master/vst/vstspeaker.h).
Each permutation is **native bus index → SOTF decoder output index**:
`native[j] = decoded[permutation[j]]`. It is not the inverse permutation.

| SOTF layout | SOTF output roles by index | VST3 arrangement / mask | Bus index → SOTF index |
|---|---|---|---|
| 5.1 | FL, FR, C, LFE, SL, SR | `k51` / `0x000000000000003f` | `[0,1,2,3,4,5]` |
| 7.1 | FL, FR, C, LFE, SL, SR, BL, BR | `k71Music` / `0x000000000000063f` | `[0,1,2,3,6,7,4,5]` |
| 5.1.2 | FL, FR, C, LFE, SL, SR, TFL, TFR | `k51_2` / `0x000000000000503f` | `[0,1,2,3,4,5,6,7]` |
| 5.1.4 | FL, FR, C, LFE, SL, SR, TFL, TFR, TBL, TBR | `k51_4` / `0x000000000002d03f` | `[0,1,2,3,4,5,6,7,8,9]` |
| 7.1.2 | FL, FR, C, LFE, SL, SR, BL, BR, TFL, TFR | `k71_2_TF` / `0x000000000000563f` | `[0,1,2,3,6,7,4,5,8,9]` |
| 7.1.4 | FL, FR, C, LFE, SL, SR, BL, BR, TFL, TFR, TBL, TBR | `k71_4` / `0x000000000002d63f` | `[0,1,2,3,6,7,4,5,8,9,10,11]` |
| 9.1.4 | FL, FR, C, LFE, SL, SR, BL, BR, WL, WR, TFL, TFR, TBL, TBR | `k91_4_W` / `0x180000000002d63f` | `[0,1,2,3,6,7,4,5,10,11,12,13,8,9]` |
| 9.1.6 | FL, FR, C, LFE, SL, SR, BL, BR, WL, WR, TFL, TFR, TBL, TBR, TMiL, TMiR | `k91_6_W` / `0x180000000302d63f` | `[0,1,2,3,6,7,4,5,10,11,12,13,14,15,8,9]` |

The surround-role mapping is contextual: the 5.1 family's only surround pair
maps to Ls/Rs; layouts with separate sides and backs map sides to Sl/Sr and
backs to Ls/Rs. This interpretation and all permutations need independent
role fixtures in native tests. The existing 7.1.2 uses front heights and thus
requires the TF variant. Wide targets require the W variants. Default 5.1
retains its existing mask and identity ordering.

These computed masks are design expectations, not evidence that the locked
binding exposes each symbolic constant. Verify or define the exact standardized
bits in the adapter without updating unrelated dependencies.

## CLAP surround maps

The official [surround extension](https://raw.githubusercontent.com/free-audio/clap/main/include/clap/ext/surround.h)
supports an ordered channel map. The following maps retain SOTF buffer order.
Numeric IDs refer to that header's roles; front-center is 2, side-left/right
are 9/10, rear-left/right 4/5, and top-side-left/right 18/19.

| SOTF layout | Proposed standard CLAP role map |
|---|---|
| 5.1 | `[0,1,2,3,9,10]` |
| 7.1 | `[0,1,2,3,9,10,4,5]` |
| 5.1.2 | `[0,1,2,3,9,10,12,14]` |
| 5.1.4 | `[0,1,2,3,9,10,12,14,15,17]` |
| 7.1.2 | `[0,1,2,3,9,10,4,5,12,14]` |
| 7.1.4 | `[0,1,2,3,9,10,4,5,12,14,15,17]` |
| 9.1.4 | Not representable: WL/WR have no standard role |
| 9.1.6 | Not representable: WL/WR have no standard role |

CLAP's current list has no wide role. FLC/FRC describe front-center neighbors
and must not substitute for SOTF's ±60° wides. AUD135 must specify a truthful
format-specific disposition for the two wide targets before advertising their
standard surround interoperability. Their bridge/FFI/VST3 scope remains intact.
An unspecified discrete port with an explicit application mapping, or a future
capability extension, is a separate documented compatibility policy requiring
review; a named configuration alone does not establish standard surround
metadata.

## Required checks

- Compare every map against separately written physical-role fixtures, not
  only the implementation table itself.
- Exercise all channels with distinguishable signals and compare complete
  output waveforms after the proposed permutation.
- Verify equal-width targets remain distinct: 7.1 versus 5.1.2 and 5.1.4
  versus 7.1.2.
- Preserve the current default order-1/5.1 packaged output bit-for-bit.
- Verify metadata and actual buffer order through both packaged native
  callbacks and the SOTF loader, including state restore and isolated loading.
