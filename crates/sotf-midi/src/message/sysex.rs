//! Structured Universal SysEx: identity, GM, device control, MIDI tuning.
//!
//! Wire formats follow the MIDI 1.0 Detailed Specification and the MIDI
//! Tuning Standard (original 1992 MTS plus the bank/dump extensions):
//! - Identity Request `F0 7E <dev> 06 01 F7` and Reply `F0 7E <dev> 06 02
//!   <id> <family lo hi> <model lo hi> <version x4> F7`, where a leading
//!   `0x00` id byte introduces a 3-byte manufacturer id.
//! - General MIDI: `F0 7E <dev> 09 01|02|03 F7` (GM1 On, GM Off, GM2 On).
//! - Device Control (real-time): `F0 7F <dev> 04 01|02|03|04 <ll> <mm>
//!   F7` (master volume, balance, fine tuning, coarse tuning; 14-bit).
//! - Bulk Tuning Dump Request `F0 7E <dev> 08 00 <tt> F7` and Bank
//!   variant `F0 7E <dev> 08 03 <bb> <tt> F7`.
//! - Bulk Tuning Dump Reply `F0 7E <dev> 08 01 <tt> <16 name>
//!   [xx yy zz]x128 <ck> F7` (408 bytes; checksum = XOR of every byte
//!   after `F0` up to `ck`, masked to 7 bits).
//! - Single Note Tuning Change `F0 7F <dev> 08 02 <tt> <ll>
//!   [kk xx yy zz]* F7` and Bank variants on sub-ID `07`
//!   (`F0 7F|7E <dev> 08 07 <bb> <tt> <ll> [...] F7`).
//! - Scale/Octave 1-byte change `F0 7E|7F <dev> 08 08 <ff gg hh>
//!   [12 bytes] F7` and 2-byte change `... 08 09 ... [24 bytes] F7`,
//!   with the 3-byte channel bitmap (`ff` bits 0-1 = ch 15-16,
//!   `gg` = ch 8-14, `hh` = ch 1-7; `ff` bits 2-6 reserved zero).
//! - Scale/Octave Dump 1-byte (sub `05`) and 2-byte (sub `06`):
//!   `F0 7E <dev> 08 05|06 <bb> <tt> <16 name> [12|24 bytes] <ck> F7`.
//!
//! Frequency data `[xx yy zz]`: `xx` = semitone (MIDI note), `yy`/`zz` =
//! 14-bit fraction above it in units of 100/16384 cents; `7F 7F 7F`
//! means "no change". [`frequency_hz`] converts to Hertz.
//!
//! Malformed structured messages decode as `None` so callers fall back
//! to generic SysEx handling; encoding validates ranges and returns
//! [`MidiError`] for unrepresentable values.

use crate::error::{MidiError, Result};
use serde::{Deserialize, Serialize};

/// Bulk Tuning Dump Reply is always 408 bytes on the wire.
pub const BULK_DUMP_WIRE_LEN: usize = 408;
/// Frequency triples per bulk dump.
pub const BULK_DUMP_NOTES: usize = 128;
/// Scale/octave pitch classes C..B.
pub const SCALE_PITCH_CLASSES: usize = 12;

/// 1- or 3-byte manufacturer id in Identity Replies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ManufacturerId {
    /// Single-byte id.
    One(u8),
    /// Extended id: leading `0x00` plus two bytes.
    Three([u8; 2]),
}

impl ManufacturerId {
    /// Decode from the bytes following sub-ID `02`. Returns the id and
    /// the number of bytes consumed.
    pub fn decode(bytes: &[u8]) -> Option<(Self, usize)> {
        match bytes {
            [] => None,
            [0x00, b, c, ..] => Some((Self::Three([*b, *c]), 3)),
            [id, ..] => Some((Self::One(*id), 1)),
        }
    }

    /// Encode to id bytes.
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Self::One(id) => vec![*id & 0x7F],
            Self::Three([b, c]) => vec![0x00, b & 0x7F, c & 0x7F],
        }
    }
}

/// Identity Reply body (device id travels on the message variant).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityReply {
    /// Manufacturer id.
    pub manufacturer: ManufacturerId,
    /// Device family, little-endian (LSB first).
    pub family: u16,
    /// Device model, little-endian.
    pub model: u16,
    /// Software revision, 4 bytes.
    pub version: [u8; 4],
}

/// General MIDI system mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GmMode {
    /// Sub `01`: General MIDI System On.
    Gm1On,
    /// Sub `02`: GM System Off (back to the power-up state).
    GmOff,
    /// Sub `03`: General MIDI 2 System On.
    Gm2On,
}

impl GmMode {
    /// The sub-ID#2 number.
    pub fn number(self) -> u8 {
        match self {
            Self::Gm1On => 0x01,
            Self::GmOff => 0x02,
            Self::Gm2On => 0x03,
        }
    }

    /// Decode a sub-ID#2 number.
    pub fn decode(number: u8) -> Option<Self> {
        match number {
            0x01 => Some(Self::Gm1On),
            0x02 => Some(Self::GmOff),
            0x03 => Some(Self::Gm2On),
            _ => None,
        }
    }
}

/// Master device-control parameter with its 14-bit value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MasterControl {
    /// Sub `01`: master volume.
    Volume(u16),
    /// Sub `02`: master balance.
    Balance(u16),
    /// Sub `03`: master fine tuning.
    FineTuning(u16),
    /// Sub `04`: master coarse tuning.
    CoarseTuning(u16),
}

impl MasterControl {
    /// The sub-ID#2 number.
    pub fn number(&self) -> u8 {
        match self {
            Self::Volume(_) => 0x01,
            Self::Balance(_) => 0x02,
            Self::FineTuning(_) => 0x03,
            Self::CoarseTuning(_) => 0x04,
        }
    }

    /// The 14-bit value.
    pub fn value(&self) -> u16 {
        match self {
            Self::Volume(value)
            | Self::Balance(value)
            | Self::FineTuning(value)
            | Self::CoarseTuning(value) => *value,
        }
    }

    /// Decode a sub-ID#2 number plus 14-bit value.
    pub fn decode(number: u8, value: u16) -> Option<Self> {
        match number {
            0x01 => Some(Self::Volume(value)),
            0x02 => Some(Self::Balance(value)),
            0x03 => Some(Self::FineTuning(value)),
            0x04 => Some(Self::CoarseTuning(value)),
            _ => None,
        }
    }
}

/// MTS frequency data: semitone plus 14-bit fraction above it in units
/// of 100/16384 cents. `7F 7F 7F` (semitone 127, fraction max) is
/// reserved for "no change".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TuningFrequency {
    /// Semitone (MIDI note number to retune to).
    pub semitone: u8,
    /// Fractional part above the semitone, 14-bit.
    pub fraction: u16,
}

impl TuningFrequency {
    /// Whether this triple means "no change".
    pub fn is_no_change(&self) -> bool {
        self.semitone == 0x7F && self.fraction == 0x3FFF
    }

    /// Convert to Hertz: 440 Hz reference on semitone 69.
    pub fn frequency_hz(&self) -> f64 {
        let cents = f64::from(self.semitone) * 100.0 - 6900.0
            + f64::from(self.fraction) * 100.0 / 16384.0;
        440.0 * 2f64.powf(cents / 1200.0)
    }

    /// Decode one `[xx yy zz]` triple.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        match bytes {
            [xx, yy, zz, ..] => Some(Self {
                semitone: *xx,
                fraction: ((u16::from(*yy) << 7) | u16::from(*zz)) & 0x3FFF,
            }),
            _ => None,
        }
    }

    /// Encode one `[xx yy zz]` triple.
    pub fn encode(&self) -> [u8; 3] {
        [
            self.semitone & 0x7F,
            ((self.fraction >> 7) & 0x7F) as u8,
            (self.fraction & 0x7F) as u8,
        ]
    }
}

/// One retuned key in a Single Note Tuning Change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SingleNoteChange {
    /// MIDI key number to retune.
    pub key: u8,
    /// Frequency data for the key.
    pub frequency: TuningFrequency,
}

/// Whether a message uses the real-time (`F0 7F`) header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Realtime {
    /// Universal Real-Time header `F0 7F` (affects sounding notes).
    Yes,
    /// Universal Non-Real-Time header `F0 7E` (setup only).
    No,
}

impl Realtime {
    /// Decode the SysEx header status byte.
    pub fn decode(status: u8) -> Option<Self> {
        match status {
            0x7F => Some(Self::Yes),
            0x7E => Some(Self::No),
            _ => None,
        }
    }

    /// The header status byte.
    pub fn status(self) -> u8 {
        match self {
            Self::Yes => 0x7F,
            Self::No => 0x7E,
        }
    }
}

/// Check the `F0 <7E|7F> <dev> 08 <sub>` universal-tuning prefix.
/// Returns `(realtime, device, body)` on match.
fn tuning_prefix(bytes: &[u8], sub: u8) -> Option<(Realtime, u8, &[u8])> {
    if bytes.len() < 6
        || bytes[bytes.len() - 1] != 0xF7
        || bytes[1..bytes.len() - 1].iter().any(|b| b & 0x80 != 0)
    {
        return None;
    }
    match bytes {
        [0xF0, status, device, 0x08, got, ..] if *got == sub => Some((
            Realtime::decode(*status)?,
            *device,
            &bytes[5..bytes.len() - 1],
        )),
        _ => None,
    }
}

/// Require 7-bit payload bytes and an `F7` terminator.
fn framed_tail(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.len() >= 2
        && bytes[0] == 0xF0
        && bytes[bytes.len() - 1] == 0xF7
        && bytes[1..bytes.len() - 1].iter().all(|b| b & 0x80 == 0)
    {
        Some(&bytes[1..bytes.len() - 1])
    } else {
        None
    }
}

/// Identity Request body: just the device id (`F0 7E <dev> 06 01 F7`).
pub fn decode_identity_request(bytes: &[u8]) -> Option<u8> {
    match framed_tail(bytes)? {
        [0x7E, device, 0x06, 0x01] => Some(*device),
        _ => None,
    }
}

/// Encode an Identity Request.
pub fn encode_identity_request(device: u8) -> Vec<u8> {
    vec![0xF0, 0x7E, device & 0x7F, 0x06, 0x01, 0xF7]
}

/// Decode an Identity Reply, returning device and body.
pub fn decode_identity_reply(bytes: &[u8]) -> Option<(u8, IdentityReply)> {
    let tail = framed_tail(bytes)?;
    match tail {
        [0x7E, device, 0x06, 0x02, rest @ ..] => {
            let (manufacturer, used) = ManufacturerId::decode(rest)?;
            let rest = &rest[used..];
            match rest {
                [fam_lo, fam_hi, mod_lo, mod_hi, v0, v1, v2, v3] => Some((
                    *device,
                    IdentityReply {
                        manufacturer,
                        family: u16::from(*fam_lo) | (u16::from(*fam_hi) << 8),
                        model: u16::from(*mod_lo) | (u16::from(*mod_hi) << 8),
                        version: [*v0, *v1, *v2, *v3],
                    },
                )),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Encode an Identity Reply.
pub fn encode_identity_reply(device: u8, reply: &IdentityReply) -> Vec<u8> {
    let mut bytes = vec![0xF0, 0x7E, device & 0x7F, 0x06, 0x02];
    bytes.extend_from_slice(&reply.manufacturer.encode());
    bytes.extend_from_slice(&[
        (reply.family & 0xFF) as u8,
        ((reply.family >> 8) & 0xFF) as u8,
        (reply.model & 0xFF) as u8,
        ((reply.model >> 8) & 0xFF) as u8,
    ]);
    bytes.extend_from_slice(&reply.version.map(|byte| byte & 0x7F));
    bytes.push(0xF7);
    bytes
}

/// Decode a GM System message, returning device and mode.
pub fn decode_gm_system(bytes: &[u8]) -> Option<(u8, GmMode)> {
    match framed_tail(bytes)? {
        [0x7E, device, 0x09, sub] => Some((*device, GmMode::decode(*sub)?)),
        _ => None,
    }
}

/// Encode a GM System message.
pub fn encode_gm_system(device: u8, mode: GmMode) -> Vec<u8> {
    vec![0xF0, 0x7E, device & 0x7F, 0x09, mode.number(), 0xF7]
}

/// Decode a master device-control message, returning device and control.
pub fn decode_master_control(bytes: &[u8]) -> Option<(u8, MasterControl)> {
    match framed_tail(bytes)? {
        [0x7F, device, 0x04, sub, ll, mm] => Some((
            *device,
            MasterControl::decode(*sub, (u16::from(*ll) | (u16::from(*mm) << 7)) & 0x3FFF)?,
        )),
        _ => None,
    }
}

/// Encode a master device-control message. Values above 14 bits are an error.
pub fn encode_master_control(device: u8, control: MasterControl) -> Result<Vec<u8>> {
    if control.value() > 0x3FFF {
        return Err(MidiError::InvalidMessage(format!(
            "master control value out of range: {:#X}",
            control.value()
        )));
    }
    Ok(vec![
        0xF0,
        0x7F,
        device & 0x7F,
        0x04,
        control.number(),
        (control.value() & 0x7F) as u8,
        ((control.value() >> 7) & 0x7F) as u8,
        0xF7,
    ])
}

/// A decoded Single Note Tuning Change:
/// `(realtime, device, bank, program, changes)`.
pub type SingleNoteDecoded = (Realtime, u8, Option<u8>, u8, Vec<SingleNoteChange>);

/// Decode a Single Note Tuning Change (sub `02` plain, sub `07` with bank).
pub fn decode_single_note(bytes: &[u8]) -> Option<SingleNoteDecoded> {
    // Plain form first: F0 7F <dev> 08 02 <tt> <ll> [...] F7.
    if let Some((realtime, device, body)) = tuning_prefix(bytes, 0x02) {
        if realtime != Realtime::Yes {
            return None;
        }
        return match body {
            [program, count, rest @ ..] if rest.len() == usize::from(*count) * 4 => {
                Some((realtime, device, None, *program, decode_changes(rest)?))
            }
            _ => None,
        };
    }
    // Bank form: F0 7? <dev> 08 07 <bb> <tt> <ll> [...] F7.
    let (realtime, device, body) = tuning_prefix(bytes, 0x07)?;
    match body {
        [bank, program, count, rest @ ..] if rest.len() == usize::from(*count) * 4 => Some((
            realtime,
            device,
            Some(*bank),
            *program,
            decode_changes(rest)?,
        )),
        _ => None,
    }
}

fn decode_changes(bytes: &[u8]) -> Option<Vec<SingleNoteChange>> {
    // Callers only pass multiples of 4 (count-checked framing).
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| {
            Some(SingleNoteChange {
                key: chunk[0],
                frequency: TuningFrequency::decode(&chunk[1..4])?,
            })
        })
        .collect()
}

/// Encode a Single Note Tuning Change. `bank` selects sub `07`
/// (either header); `None` selects sub `02` (real-time only).
pub fn encode_single_note(
    realtime: Realtime,
    device: u8,
    bank: Option<u8>,
    program: u8,
    changes: &[SingleNoteChange],
) -> Result<Vec<u8>> {
    if changes.len() > 0x7F {
        return Err(MidiError::InvalidMessage(format!(
            "too many single-note changes: {}",
            changes.len()
        )));
    }
    let mut bytes = vec![0xF0, realtime.status(), device & 0x7F, 0x08];
    match bank {
        None => {
            if realtime != Realtime::Yes {
                return Err(MidiError::InvalidMessage(
                    "bank-less single-note change is real-time only".to_string(),
                ));
            }
            bytes.push(0x02);
            bytes.push(program & 0x7F);
        }
        Some(bank) => {
            bytes.push(0x07);
            bytes.push(bank & 0x7F);
            bytes.push(program & 0x7F);
        }
    }
    bytes.push(changes.len() as u8);
    for change in changes {
        bytes.push(change.key & 0x7F);
        bytes.extend_from_slice(&change.frequency.encode());
    }
    bytes.push(0xF7);
    Ok(bytes)
}

/// Scale/octave channel bitmap (`ff` bits 0-1 = ch 15-16, `gg` = ch 8-14,
/// `hh` = ch 1-7; `ff` bits 2-6 reserved zero).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScaleChannels {
    /// Options/channel byte 1.
    pub ff: u8,
    /// Channel byte 2.
    pub gg: u8,
    /// Channel byte 3.
    pub hh: u8,
}

impl ScaleChannels {
    /// Validate the reserved bits.
    pub fn validate(&self) -> Result<()> {
        if self.ff & 0xFC != 0 {
            return Err(MidiError::InvalidMessage(format!(
                "scale/octave reserved channel bits set: {:#04X}",
                self.ff
            )));
        }
        Ok(())
    }

    /// The addressed channels, 1-based, in ascending order.
    pub fn channels(&self) -> Vec<u8> {
        let mut channels = Vec::new();
        for channel in 1..=7u8 {
            if self.hh & (1 << (channel - 1)) != 0 {
                channels.push(channel);
            }
        }
        for channel in 8..=14u8 {
            if self.gg & (1 << (channel - 8)) != 0 {
                channels.push(channel);
            }
        }
        for channel in 15..=16u8 {
            if self.ff & (1 << (channel - 15)) != 0 {
                channels.push(channel);
            }
        }
        channels
    }
}

/// Decode the `ff gg hh [payload]` tail of a scale/octave change message.
fn decode_scale_tail(
    body: &[u8],
    payload: usize,
) -> Option<(ScaleChannels, &[u8])> {
    match body {
        [ff, gg, hh, rest @ ..] if rest.len() == payload => {
            let channels = ScaleChannels {
                ff: *ff,
                gg: *gg,
                hh: *hh,
            };
            channels.validate().ok()?;
            Some((channels, rest))
        }
        _ => None,
    }
}

/// Decode a Scale/Octave 1-byte change (sub `08`), returning
/// `(realtime, device, channels, cent offsets C..B)`.
pub fn decode_scale_octave(bytes: &[u8]) -> Option<(Realtime, u8, ScaleChannels, [i8; 12])> {
    let (realtime, device, body) = tuning_prefix(bytes, 0x08)?;
    let (channels, data) = decode_scale_tail(body, SCALE_PITCH_CLASSES)?;
    let mut offsets = [0i8; SCALE_PITCH_CLASSES];
    for (index, byte) in data.iter().enumerate() {
        offsets[index] = (*byte as i8).wrapping_sub(0x40);
    }
    Some((realtime, device, channels, offsets))
}

/// Encode a Scale/Octave 1-byte change. Offsets span -64..=+63 cents.
pub fn encode_scale_octave(
    realtime: Realtime,
    device: u8,
    channels: ScaleChannels,
    offsets: &[i8; SCALE_PITCH_CLASSES],
) -> Result<Vec<u8>> {
    channels.validate()?;
    let mut bytes = vec![
        0xF0,
        realtime.status(),
        device & 0x7F,
        0x08,
        0x08,
        channels.ff,
        channels.gg,
        channels.hh,
    ];
    for offset in offsets {
        if *offset < -64 || *offset > 63 {
            return Err(MidiError::InvalidMessage(format!(
                "scale/octave offset out of range: {offset}"
            )));
        }
        bytes.push(offset.wrapping_add(0x40) as u8);
    }
    bytes.push(0xF7);
    Ok(bytes)
}

/// Decode a Scale/Octave 2-byte change (sub `09`), returning
/// `(realtime, device, channels, 14-bit values C..B, 8192 = equal)`.
pub fn decode_scale_octave_14(
    bytes: &[u8],
) -> Option<(Realtime, u8, ScaleChannels, [u16; 12])> {
    let (realtime, device, body) = tuning_prefix(bytes, 0x09)?;
    let (channels, data) = decode_scale_tail(body, SCALE_PITCH_CLASSES * 2)?;
    let mut values = [0u16; SCALE_PITCH_CLASSES];
    // `data` is exactly 24 bytes (length-checked tail).
    for (index, pair) in data.as_chunks::<2>().0.iter().enumerate() {
        values[index] = (u16::from(pair[0]) << 7) | u16::from(pair[1]);
    }
    Some((realtime, device, channels, values))
}

/// Encode a Scale/Octave 2-byte change. Values are 14-bit (8192 =
/// equal temperament, ~0.0122 cents per step, ±100 cents range).
pub fn encode_scale_octave_14(
    realtime: Realtime,
    device: u8,
    channels: ScaleChannels,
    values: &[u16; SCALE_PITCH_CLASSES],
) -> Result<Vec<u8>> {
    channels.validate()?;
    let mut bytes = vec![
        0xF0,
        realtime.status(),
        device & 0x7F,
        0x08,
        0x09,
        channels.ff,
        channels.gg,
        channels.hh,
    ];
    for value in values {
        if *value > 0x3FFF {
            return Err(MidiError::InvalidMessage(format!(
                "scale/octave 14-bit value out of range: {value:#X}"
            )));
        }
        bytes.push(((value >> 7) & 0x7F) as u8);
        bytes.push((value & 0x7F) as u8);
    }
    bytes.push(0xF7);
    Ok(bytes)
}

/// Convert a 14-bit scale/octave value to cents offset (8192 = 0).
pub fn scale_octave_14_cents(value: u16) -> f64 {
    (f64::from(value & 0x3FFF) - 8192.0) * 200.0 / 16384.0
}

/// Checksum over a dump message: XOR of every byte after `F0` up to the
/// checksum field, masked to 7 bits.
pub fn dump_checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |acc, byte| acc ^ byte) & 0x7F
}

/// Decode a Bulk Tuning Dump Request (sub `00`), returning
/// `(device, program)`.
pub fn decode_bulk_request(bytes: &[u8]) -> Option<(u8, u8)> {
    let tail = framed_tail(bytes)?;
    match tail {
        [0x7E, device, 0x08, 0x00, program] => Some((*device, *program)),
        _ => None,
    }
}

/// Encode a Bulk Tuning Dump Request.
pub fn encode_bulk_request(device: u8, program: u8) -> Vec<u8> {
    vec![0xF0, 0x7E, device & 0x7F, 0x08, 0x00, program & 0x7F, 0xF7]
}

/// Decode a Bank Bulk Tuning Dump Request (sub `03`), returning
/// `(device, bank, program)`.
pub fn decode_bulk_request_bank(bytes: &[u8]) -> Option<(u8, u8, u8)> {
    let tail = framed_tail(bytes)?;
    match tail {
        [0x7E, device, 0x08, 0x03, bank, program] => Some((*device, *bank, *program)),
        _ => None,
    }
}

/// Encode a Bank Bulk Tuning Dump Request.
pub fn encode_bulk_request_bank(device: u8, bank: u8, program: u8) -> Vec<u8> {
    vec![
        0xF0,
        0x7E,
        device & 0x7F,
        0x08,
        0x03,
        bank & 0x7F,
        program & 0x7F,
        0xF7,
    ]
}

/// A Bulk Tuning Dump Reply: program, 16-character name, and 128
/// `(semitone, 14-bit fraction)` triples with a trailing checksum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BulkTuningDump {
    /// Tuning program number.
    pub program: u8,
    /// Tuning name, 16 ASCII characters.
    pub name: [u8; 16],
    /// Per-note frequency data for keys 0-127 (`(semitone, fraction)`).
    pub notes: Vec<(u8, u16)>,
    /// Checksum byte as received (not verified on decode, per the
    /// spec's receiver guidance for sub `01`).
    pub checksum: u8,
}

impl BulkTuningDump {
    /// Compute the wire checksum: XOR over everything after `F0`
    /// (`7E device 08 01`, program, name, frequency data).
    pub fn computed_checksum(&self, device: u8) -> u8 {
        let mut bytes = vec![0x7E, device & 0x7F, 0x08, 0x01, self.program & 0x7F];
        bytes.extend_from_slice(&self.name.map(|byte| byte & 0x7F));
        for (semitone, fraction) in &self.notes {
            bytes.push(semitone & 0x7F);
            bytes.push(((fraction >> 7) & 0x7F) as u8);
            bytes.push((fraction & 0x7F) as u8);
        }
        dump_checksum(&bytes)
    }
}

/// Decode a Bulk Tuning Dump Reply, returning `(device, dump)`.
/// Framing and length (408 bytes) are enforced; the checksum is stored,
/// not verified, per the spec's receiver guidance for sub `01`.
pub fn decode_bulk_dump(bytes: &[u8]) -> Option<(u8, BulkTuningDump)> {
    if bytes.len() != BULK_DUMP_WIRE_LEN {
        return None;
    }
    let tail = framed_tail(bytes)?;
    match tail {
        [0x7E, device, 0x08, 0x01, program, rest @ ..] => {
            let (name, rest) = rest.split_at_checked(16)?;
            let (freq, rest) = rest.split_at_checked(BULK_DUMP_NOTES * 3)?;
            let [checksum] = rest else { return None };
            let mut notes = Vec::with_capacity(BULK_DUMP_NOTES);
            // `freq` is exactly 384 bytes (length-checked framing).
            for triple in freq.as_chunks::<3>().0 {
                notes.push((
                    triple[0],
                    (u16::from(triple[1]) << 7) | u16::from(triple[2]),
                ));
            }
            let mut name_bytes = [0u8; 16];
            name_bytes.copy_from_slice(name);
            Some((
                *device,
                BulkTuningDump {
                    program: *program,
                    name: name_bytes,
                    notes,
                    checksum: *checksum,
                },
            ))
        }
        _ => None,
    }
}

/// Encode a Bulk Tuning Dump Reply, computing the checksum. Fails when
/// `dump.notes` does not hold exactly 128 entries.
pub fn encode_bulk_dump(device: u8, dump: &BulkTuningDump) -> Result<Vec<u8>> {
    if dump.notes.len() != BULK_DUMP_NOTES {
        return Err(MidiError::InvalidMessage(format!(
            "bulk dump needs 128 notes, got {}",
            dump.notes.len()
        )));
    }
    let mut bytes = vec![0xF0, 0x7E, device & 0x7F, 0x08, 0x01, dump.program & 0x7F];
    bytes.extend_from_slice(&dump.name.map(|byte| byte & 0x7F));
    for (semitone, fraction) in &dump.notes {
        bytes.push(semitone & 0x7F);
        bytes.push(((fraction >> 7) & 0x7F) as u8);
        bytes.push((fraction & 0x7F) as u8);
    }
    let checksum = dump_checksum(&bytes[1..]);
    bytes.push(checksum);
    bytes.push(0xF7);
    Ok(bytes)
}

/// A Scale/Octave Dump (sub `05` 1-byte, sub `06` 2-byte): bank, program,
/// name, 12 or 24 data bytes, and a verified checksum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScaleOctaveDump {
    /// True for the 2-byte form (sub `06`), false for 1-byte (sub `05`).
    pub two_byte: bool,
    /// Bank number.
    pub bank: u8,
    /// Tuning program number.
    pub program: u8,
    /// Tuning name, 16 ASCII characters.
    pub name: [u8; 16],
    /// Raw data bytes (12 or 24).
    pub data: Vec<u8>,
    /// Checksum byte as received.
    pub checksum: u8,
}

/// Decode a Scale/Octave Dump, returning `(device, dump)`. Unlike the
/// original bulk dump, the checksum is verified here per the spec.
pub fn decode_scale_dump(bytes: &[u8]) -> Option<(u8, ScaleOctaveDump)> {
    let tail = framed_tail(bytes)?;
    let (two_byte, rest) = match tail {
        [0x7E, device, 0x08, 0x05, rest @ ..] => (false, (*device, rest)),
        [0x7E, device, 0x08, 0x06, rest @ ..] => (true, (*device, rest)),
        _ => return None,
    };
    let (device, rest) = rest;
    let expected = 1 + 1 + 16 + if two_byte { 24 } else { 12 } + 1;
    if rest.len() != expected {
        return None;
    }
    // Verify the checksum over everything after F0 up to ck.
    let checksum_at = bytes.len() - 2;
    if dump_checksum(&bytes[1..checksum_at]) != bytes[checksum_at] {
        return None;
    }
    let (bank, rest) = rest.split_at_checked(1)?;
    let (program, rest) = rest.split_at_checked(1)?;
    let (name, rest) = rest.split_at_checked(16)?;
    let (data, _) = rest.split_at_checked(if two_byte { 24 } else { 12 })?;
    let mut name_bytes = [0u8; 16];
    name_bytes.copy_from_slice(name);
    Some((
        device,
        ScaleOctaveDump {
            two_byte,
            bank: bank[0],
            program: program[0],
            name: name_bytes,
            data: data.to_vec(),
            checksum: bytes[checksum_at],
        },
    ))
}

/// Encode a Scale/Octave Dump, computing the checksum.
pub fn encode_scale_dump(device: u8, dump: &ScaleOctaveDump) -> Result<Vec<u8>> {
    let want = if dump.two_byte { 24 } else { 12 };
    if dump.data.len() != want {
        return Err(MidiError::InvalidMessage(format!(
            "scale/octave dump needs {want} data bytes, got {}",
            dump.data.len()
        )));
    }
    let mut bytes = vec![
        0xF0,
        0x7E,
        device & 0x7F,
        0x08,
        if dump.two_byte { 0x06 } else { 0x05 },
        dump.bank & 0x7F,
        dump.program & 0x7F,
    ];
    bytes.extend_from_slice(&dump.name.map(|byte| byte & 0x7F));
    for byte in &dump.data {
        bytes.push(byte & 0x7F);
    }
    let checksum = dump_checksum(&bytes[1..]);
    bytes.push(checksum);
    bytes.push(0xF7);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_request_matches_spec_example() {
        // Canonical request frame; device 0x7F addresses all devices.
        let bytes = encode_identity_request(0x7F);
        assert_eq!(bytes, vec![0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7]);
        assert_eq!(decode_identity_request(&bytes), Some(0x7F));
        assert_eq!(decode_identity_request(&[0xF0, 0x7E]), None);
    }

    #[test]
    fn identity_reply_round_trips_both_id_forms() {
        for manufacturer in [
            ManufacturerId::One(0x41),
            ManufacturerId::Three([0x20, 0x29]),
        ] {
            let reply = IdentityReply {
                manufacturer,
                family: 0x1234,
                model: 0x0001,
                version: [1, 0, 0, 0],
            };
            let bytes = encode_identity_reply(0x10, &reply);
            let (device, decoded) = decode_identity_reply(&bytes).unwrap();
            assert_eq!(device, 0x10);
            assert_eq!(decoded, reply);
        }
        // Truncated reply stays undecodable.
        assert!(decode_identity_reply(&[0xF0, 0x7E, 0x10, 0x06, 0x02, 0x41, 0xF7]).is_none());
    }

    #[test]
    fn gm_frames_round_trip() {
        for (mode, sub) in [
            (GmMode::Gm1On, 0x01),
            (GmMode::GmOff, 0x02),
            (GmMode::Gm2On, 0x03),
        ] {
            let bytes = encode_gm_system(0x7F, mode);
            assert_eq!(bytes, vec![0xF0, 0x7E, 0x7F, 0x09, sub, 0xF7]);
            assert_eq!(decode_gm_system(&bytes), Some((0x7F, mode)));
        }
        assert_eq!(GmMode::decode(0x04), None);
    }

    #[test]
    fn master_control_round_trips_14_bit_values() {
        for control in [
            MasterControl::Volume(0x2000),
            MasterControl::Balance(0x3FFF),
            MasterControl::FineTuning(0),
            MasterControl::CoarseTuning(100),
        ] {
            let bytes = encode_master_control(0x7F, control).unwrap();
            let (device, decoded) = decode_master_control(&bytes).unwrap();
            assert_eq!(device, 0x7F);
            assert_eq!(decoded, control);
        }
        // Volume at +6 dB: MSB 0x40, LSB 0x00.
        let bytes = encode_master_control(0x7F, MasterControl::Volume(0x2000)).unwrap();
        assert_eq!(bytes, vec![0xF0, 0x7F, 0x7F, 0x04, 0x01, 0x00, 0x40, 0xF7]);
        assert!(encode_master_control(0x7F, MasterControl::Volume(0x4000)).is_err());
    }

    #[test]
    fn frequency_math_matches_concert_pitch() {
        let a4 = TuningFrequency {
            semitone: 69,
            fraction: 0,
        };
        assert!((a4.frequency_hz() - 440.0).abs() < 1e-9);
        let a3 = TuningFrequency {
            semitone: 57,
            fraction: 0,
        };
        assert!((a3.frequency_hz() - 220.0).abs() < 1e-9);
        // One full 100-cent fraction step up from A4.
        let sharp = TuningFrequency {
            semitone: 69,
            fraction: 16384 - 1,
        };
        assert!(sharp.frequency_hz() > 440.0);
        assert!(TuningFrequency {
            semitone: 0x7F,
            fraction: 0x3FFF
        }
        .is_no_change());
    }

    #[test]
    fn single_note_matches_spec_framing() {
        let changes = vec![
            SingleNoteChange {
                key: 69,
                frequency: TuningFrequency {
                    semitone: 69,
                    fraction: 0,
                },
            },
            SingleNoteChange {
                key: 60,
                frequency: TuningFrequency {
                    semitone: 60,
                    fraction: 8192,
                },
            },
        ];
        // Plain real-time form: F0 7F <dev> 08 02 <tt> <ll> [...] F7.
        let bytes = encode_single_note(Realtime::Yes, 0x7F, None, 0, &changes).unwrap();
        assert_eq!(
            bytes,
            vec![
                0xF0, 0x7F, 0x7F, 0x08, 0x02, 0x00, 0x02, 69, 69, 0x00, 0x00, 60, 60, 0x40,
                0x00, 0xF7
            ]
        );
        let (realtime, device, bank, program, decoded) = decode_single_note(&bytes).unwrap();
        assert_eq!((realtime, device, bank, program), (Realtime::Yes, 0x7F, None, 0));
        assert_eq!(decoded, changes);
        // Bank form on either header.
        for realtime in [Realtime::Yes, Realtime::No] {
            let bytes =
                encode_single_note(realtime, 0x10, Some(2), 5, &changes[..1]).unwrap();
            let decoded = decode_single_note(&bytes).unwrap();
            assert_eq!(decoded.0, realtime);
            assert_eq!(decoded.2, Some(2));
            assert_eq!(decoded.4, changes[..1].to_vec());
        }
        // Bank-less non-real-time is rejected on encode.
        assert!(encode_single_note(Realtime::No, 0x7F, None, 0, &changes).is_err());
        // Declared count must match the payload.
        assert!(decode_single_note(&[0xF0, 0x7F, 0x7F, 0x08, 0x02, 0x00, 0x02, 69, 0xF7]).is_none());
    }

    #[test]
    fn scale_octave_change_round_trips() {
        let channels = ScaleChannels {
            ff: 0x00,
            gg: 0x7F,
            hh: 0x7F,
        };
        assert_eq!(
            channels.channels(),
            (1..=14u8).collect::<Vec<u8>>()
        );
        let mut offsets = [0i8; 12];
        offsets[0] = -64;
        offsets[9] = 63;
        for realtime in [Realtime::Yes, Realtime::No] {
            let bytes = encode_scale_octave(realtime, 0x7F, channels, &offsets).unwrap();
            let (rt, device, ch, decoded) = decode_scale_octave(&bytes).unwrap();
            assert_eq!((rt, device, ch), (realtime, 0x7F, channels));
            assert_eq!(decoded, offsets);
        }
        // Reserved channel bits are rejected.
        let bad = ScaleChannels {
            ff: 0x04,
            gg: 0,
            hh: 0,
        };
        assert!(bad.validate().is_err());
        assert!(encode_scale_octave(Realtime::Yes, 0x7F, bad, &offsets).is_err());
        let mut bad_offsets = [0i8; 12];
        bad_offsets[0] = -65;
        assert!(encode_scale_octave(Realtime::Yes, 0x7F, channels, &bad_offsets).is_err());
    }

    #[test]
    fn scale_octave_14_cent_math() {
        assert!((scale_octave_14_cents(8192) - 0.0).abs() < 1e-9);
        assert!((scale_octave_14_cents(0) + 100.0).abs() < 1e-6);
        assert!((scale_octave_14_cents(16383) - 100.0).abs() < 0.02);
        let values = core::array::from_fn(|index| (8192 + index as u16 * 100) & 0x3FFF);
        let bytes = encode_scale_octave_14(
            Realtime::No,
            0x7F,
            ScaleChannels { ff: 3, gg: 0, hh: 0 },
            &values,
        )
        .unwrap();
        let (_, _, _, decoded) = decode_scale_octave_14(&bytes).unwrap();
        assert_eq!(decoded, values);
    }

    #[test]
    fn bulk_request_matches_spec_example() {
        // Verbatim example from the MTS specification (program 16, device 1).
        let bytes = encode_bulk_request(0x01, 0x10);
        assert_eq!(bytes, vec![0xF0, 0x7E, 0x01, 0x08, 0x00, 0x10, 0xF7]);
        assert_eq!(decode_bulk_request(&bytes), Some((0x01, 0x10)));
        let bytes = encode_bulk_request_bank(0x01, 2, 0x10);
        assert_eq!(bytes, vec![0xF0, 0x7E, 0x01, 0x08, 0x03, 0x02, 0x10, 0xF7]);
        assert_eq!(decode_bulk_request_bank(&bytes), Some((0x01, 2, 0x10)));
    }

    #[test]
    fn bulk_dump_round_trips_full_size() {
        let mut notes = vec![(0u8, 0u16); BULK_DUMP_NOTES];
        for (index, note) in notes.iter_mut().enumerate() {
            *note = (index as u8, (index as u16 * 137) & 0x3FFF);
        }
        let dump = BulkTuningDump {
            program: 5,
            name: *b"Just Intonation ",
            notes: notes.clone(),
            checksum: 0,
        };
        let bytes = encode_bulk_dump(0x10, &dump).unwrap();
        assert_eq!(bytes.len(), BULK_DUMP_WIRE_LEN);
        let (device, decoded) = decode_bulk_dump(&bytes).unwrap();
        assert_eq!(device, 0x10);
        assert_eq!(decoded.program, 5);
        assert_eq!(decoded.notes, notes);
        assert_eq!(decoded.checksum, dump.computed_checksum(0x10));
        // Re-encoding reproduces the exact bytes (checksum included).
        let mut with_checksum = dump;
        with_checksum.checksum = with_checksum.computed_checksum(0x10);
        assert_eq!(encode_bulk_dump(0x10, &with_checksum).unwrap(), bytes);
        // Wrong length never parses as typed.
        assert!(decode_bulk_dump(&bytes[..100]).is_none());
    }

    #[test]
    fn scale_dump_verifies_checksum() {
        let dump = ScaleOctaveDump {
            two_byte: false,
            bank: 0,
            program: 1,
            name: *b"Pythagorean     ",
            data: vec![0x40; 12],
            checksum: 0,
        };
        let bytes = encode_scale_dump(0x7F, &dump).unwrap();
        let (device, decoded) = decode_scale_dump(&bytes).unwrap();
        assert_eq!(device, 0x7F);
        assert_eq!(decoded.bank, 0);
        assert_eq!(decoded.data, vec![0x40; 12]);
        // A corrupted checksum falls back to undecodable.
        let mut corrupt = bytes.clone();
        let last = corrupt.len() - 2;
        corrupt[last] ^= 0x01;
        assert_eq!(decode_scale_dump(&corrupt), None);
        // Two-byte form carries 24 bytes.
        let dump2 = ScaleOctaveDump {
            two_byte: true,
            data: [0x40, 0x00].repeat(12),
            ..dump
        };
        let bytes2 = encode_scale_dump(0x7F, &dump2).unwrap();
        assert_eq!(decode_scale_dump(&bytes2).unwrap().1.data.len(), 24);
        assert!(encode_scale_dump(0x7F, &ScaleOctaveDump { data: vec![0; 13], ..dump }).is_err());
    }
}
