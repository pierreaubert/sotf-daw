# MIDI protocol coverage — decision record (Final)

## Status

Final. Scope contract for stage 1 accepted 2026-09-27 ("accepted and go").
Stages 2–4 remain deferred proposals.

## Goal

Implement the missing MIDI protocol families in `sotf-midi`
(lib `sotf_audio_player_midi`) on top of the existing MIDI 1.0 message
model, SMF support, clock scheduling, and hardware profiles.

## Non-goals (pending confirmation)

- MIDI 2.0 transport over OS APIs (out; `midir` is a 1.0 byte-stream transport).
- DAW engine/sequencer integration of new messages (deferred stage; this
  record covers the `sotf-midi` API only until accepted otherwise).

## Settled decisions

1. **Order of work**: time/transport first — MTC (full-frame encode/decode
   plus quarter-frame assembly) and MMC transport commands. Then control
   surfaces (full Mackie Control + HUI as first-class modules), then
   structured SysEx (identity, GM/GM2, tuning), with MIDI 2.0/UMP last as a
   model-only addition. (Accepted 2026-09-27: user chose option 1.)

## Settled decisions (continued)

2. **Record location**: `crates/sotf-midi/PROTOCOLS.md`, following the
   `sotf-engine/ARCHITECTURE.md` crate-root doc convention.
   (Accepted 2026-09-27: user chose option 1.)

## Settled decisions (continued)

3. **API shape**: typed variants directly on `MidiMessage` — single
   canonical model (MTC full-frame, MTC quarter-frame, MMC), accepting
   that downstream exhaustive matches must be updated in the same stage.
   (Accepted 2026-09-27: user chose option 2.)

## Settled decisions (continued)

4. **MMC scope**: full command/response set as typed variants — one
   closed enum covering transport, arming, locate/shuttle/varispeed,
   and device-ID-addressed responses, done once.
   (Accepted 2026-09-27: user chose option 1.)
5. **Downstream impact**: the only out-of-crate `match` on `MidiMessage`
   (`sotf-engine/src/timeline/midi_track.rs`) already has a wildcard
   arm, so new variants require no engine changes.

## Settled decisions (continued)

6. **MTC rates**: all four spec rates — 24, 25, 29.97 drop-frame,
   and 30 — in the typed model with conformance tests from the start.
   (Accepted 2026-09-27: user chose option 1.)

## Settled decisions (continued)

7. **Validation**: committed conformance tests are the gate — byte-exact
   round-trip vectors for full-frame and quarter-frame at all four
   rates, quarter-frame assembly sequences (including out-of-order and
   dropped nibbles), drop-frame minute-boundary edges, and full MMC
   command set encode/decode, all under the existing `just test` gate.
   No hardware interop required for sign-off.
   (Accepted 2026-09-27: user chose option 1.)

## Scope contract (stage 1: time/transport — pending acceptance)

- **Artifacts in scope**: `crates/sotf-midi/src/message/` (new typed
  MTC/MMC variants on `MidiMessage` with parse/serialize), new committed
  conformance tests in `crates/sotf-midi`, and this record.
- **Out of scope**: control surfaces, structured SysEx, MIDI 2.0/UMP
  (deferred stages, each returning for its own interview); engine,
  sequencer, and UI integration of the new variants; hardware interop.
- **Done means**: new variants parse and serialize per the conformance
  vectors; `just check`, `just lint`, `just test` green; no new
  downstream build breakage; this record marked Final on explicit
  acceptance.
- **Deferred proposals**: stages 2–4 (surfaces, SysEx, MIDI 2.0) are
  recorded above only as order, not approved designs.

## Stages 3+4 interview (Final — scope contract accepted 2026-09-28)

### Settled decisions

8. **Stage 3 scope**: full structured-SysEx set — identity request/reply,
   GM1 on/off, GM2 on, master volume/balance/fine/coarse tuning, MTS
   single-note change, scale/octave (1- and 2-byte forms), and bulk dump
   request/reply. Sample Dump, File Dump, and Show Control stay out.
   (Accepted 2026-09-28: user chose option 1.)

9. **Stage 4 coverage**: performance core typed (Utility JR clock/
   timestamps, System Common, MIDI 1.0 + 2.0 Channel Voice, SysEx7/8
   with group numbers), Flex Data (`0x8`–`0xF`) and Stream (`0xF`)
   messages as raw 32-bit-word passthrough. MIDI CI stays out.
   (Accepted 2026-09-28: user chose option 1.)

10. **Validation**: committed byte-exact tests under `just test` —
    SysEx request/reply vectors (identity, GM, tuning incl. a
    full-size bulk dump round-trip) and UMP packet vectors for every
    typed message type plus raw passthrough, **plus a cross-check
    against an external UMP/SysEx reference** (independent
    implementation vectors, not just self round-trips). No hardware
    interop. (Amended 2026-09-28 per user: option 2.)

### Scope contract (stages 3+4 — accepted 2026-09-28: "accepted and go")

- **Artifacts in scope**: `crates/sotf-midi/src/message/` (new
  `sysex.rs` + `ump.rs` modules, new typed `MidiMessage` variants with
  parse/serialize), committed conformance tests, and this record.
- **Out of scope**: Sample/File Dump, Show Control, MIDI CI, UMP
  transport over OS APIs, engine/sequencer/UI integration, hardware
  interop.
- **Done means**: new variants parse and serialize per the conformance
  vectors; `just check`, `just lint` (sotf-midi scope), `just test`
  green; no new downstream breakage; this record finalized on explicit
  acceptance.
- **Deferred**: nothing further in the MIDI protocol track after stage 4.

## Stages 3+4 implementation notes (Final — accepted 2026-09-28)

- New modules `crates/sotf-midi/src/message/sysex.rs` (stage 3) and
  `message/ump.rs` (stage 4); new typed `MidiMessage` variants
  (`IdentityRequest/Reply`, `GmSystem`, `MasterControl`,
  `MtsSingleNote`, `MtsScaleOctave`, `MtsScaleOctave14`,
  `MtsBulkRequest`, `MtsBulkRequestBank`, `MtsBulkDump`,
  `MtsScaleOctaveDump`, plus `Ump` passthrough), all serde-capable.
  Malformed structured SysEx falls back to generic `SystemExclusive`.
- Bulk dump: `notes` is a `Vec<(u8, u16)>` (serde has no
  `Serialize` for 128-tuples); `encode_bulk_dump` rejects non-128
  lengths. The wire checksum covers everything after `F0`
  (`7E device 08 01 …`), so `computed_checksum` takes the device id.
- UMP voice1 preserves note-on velocity 0 as received instead of
  folding to note-off. Program-change program number decodes from
  word-1 byte 0 (word-0 byte 2 is reserved); SysEx8 stream id is
  word-0 byte 2 with the payload starting at byte 3 and the count
  nibble covering payload + 1.
- External cross-check: `midi2 v0.11` (crates.io) is a dev-dependency;
  `cross_check_vectors_against_midi2_crate` parses every committed
  vector with both implementations field-for-field, and our chunker
  output re-parses in the reference crate. The check caught two real
  defects before landing: program decoded from the wrong word byte,
  and a hand-transcribed SysEx8 stream id (reference: stream lives in
  word-0 byte 2, so the `payload_full` vector carries stream 0).
- Verification status (2026-09-28): `mbx test -p sotf-midi` fully
  green — 235 lib (incl. the live `midi2` cross-check) + 40
  integration + 9 property + 6 doc tests; `mbx clippy -p sotf-midi
  --all-targets -- -D warnings` clean; `just check` (workspace)
  green, so no downstream breakage. `just test` (workspace) still
  fails on `sotf-host --test true_peak_calibration ::
  host_intervals_match_published_full_fir_convolution`, an unrelated
  true-peak DSP test in the parallel loudness WIP (`sotf-host` does
  not depend on `sotf-midi`, so stages 3+4 cannot cause it).
  Accepted 2026-09-28 ("Accepted, commit and then merge into main");
  this record is Final.

## Stage 2 design (control surfaces)

- **Module shape**: new top-level `surfaces` module (`mackie`, `hui`
  submodules) speaking plain MIDI 1.0 `MidiMessage`s — no new message
  variants. Applied as the recommended default (interview question 1
  went unanswered; redirect on request).
- **Mackie**: full `MackieButton` note map (0-115, 118) with press/LED/
  decode; 14-bit faders ch 0-8; VPot/jog sign-magnitude deltas; ring
  modes; LCD writes; time/assignment displays; channel-pressure meters;
  host SysEx commands (handshake frames, backlight, touchless,
  sensitivity, meter modes, reset). Challenge-response stays raw
  passthrough (proprietary, untestable here).
- **HUI**: ping/reply, 4-char scribbles, zoned main display, LSB-first
  timecode, poly-aftertouch meters, ring values, zone/port LED framing,
  CC-pair faders with touch, VPot/jog inverted deltas, footswitch zone,
  reset detection. Button identities were never published, so switches
  stay numeric (`HuiSwitch` zone/port) paired by `HuiSwitchStream`;
  fader halves pair in `HuiFaderStream`.
- **RME**: keeps its own TotalMix dialect values (notably SELECT base 0
  vs canonical 24); only the press builder delegates to the surfaces
  module, so no behavior changed.
- **Validation**: committed byte-exact tests per direction plus
  stream-assembler and error cases, under `just test`.

## Stage 1 implementation notes (landed 2026-09-27)

- New modules `crates/sotf-midi/src/message/mtc.rs` (`MtcFrameRate`,
  `MtcTime`, `MtcQuarterFrameKind`, `MtcQuarterFrameAssembler`) and
  `message/mmc.rs` (`MmcCommand`, `MmcField`, `MmcShuttleSpeed`,
  `MmcResponse`); new `MidiMessage` variants `MtcFullFrame`,
  `MtcQuarterFrame`, `Mmc`, `MmcResponse`, all serde-capable.
- Malformed universal SysEx falls back to generic `SystemExclusive`
  (historical leniency preserved); malformed quarter-frames are errors,
  matching existing `0xF1` strictness.
- `MmcCommand::Other` preserves byte-exact round-trip for command
  numbers outside the confirmed table (41/42/43 field commands carry
  typed `MmcField` framing; 48/49 use the documented counter/step
  framing; Sub-ID#1 `07` responses keep a raw state byte because
  published response-state tables vary by manufacturer).
- Unrepresentable hand-built values (oversize bitmaps/fields, steps
  outside -64..=63, counters above `0x7F7F`) encode as empty rather
  than panicking; decoded values always re-encode.
- Conformance tests live next to the code (`mtc.rs`, `mmc.rs`,
  `midi_message.rs` test modules): rate round-trips, drop-frame gaps,
  assembly incl. reorder/loss/reset, full MMC set vectors, fallback
  and serde cases.
