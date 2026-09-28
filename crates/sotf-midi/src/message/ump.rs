//! Universal MIDI Packet (UMP): MIDI 2.0 word-oriented messages.
//!
//! Layouts follow the MIDI 2.0 UMP specification, cross-checked against
//! the `midi2` reference crate (midi2-dev): all words are big-endian
//! `u32`s; the first word carries type (high nibble) and group (low
//! nibble of byte 0) unless noted.
//! - Utility (`0x0`, 1 word): `0x0G 0S TT TT` — status 0 = Noop,
//!   1 = JR Clock, 2 = JR Timestamp, each with a 16-bit time.
//! - System Common (`0x1`, 1 word): `0x1G SS D1 D2` for statuses F1, F2,
//!   F3, F6, F8, FA, FB, FC, FE, FF with 1/2/1/0/0/0/0/0/0/0 data bytes;
//!   unused bytes must be zero.
//! - MIDI 1.0 Channel Voice (`0x2`, 1 word): status/channel plus the
//!   usual 1-2 data bytes; unused bytes must be zero.
//! - SysEx7 Data64 (`0x3`, 2 words): status (0-3) and count (0-6) share
//!   byte 1; payload bytes 2-7, all 7-bit.
//! - MIDI 2.0 Channel Voice (`0x4`, 2 words): statuses 0x0-0x6 and
//!   0x8-0xF (0x7 reserved) with 32-bit payloads; note attributes ride
//!   the low byte of word 0 with 16-bit data in word 1.
//! - SysEx8 Data128 (`0x5`, 4 words): stream id at byte 2, status and
//!   count (1-14, counting the stream id) at byte 1, up to 13 payload
//!   bytes from byte 3.
//! - Flex Data (`0x8`-`0xE`) and Stream (`0xF`) travel as validated raw
//!   4-word packets; types `0x6`-`0x7` are reserved and rejected.
//!
//! [`UmpMessage::decode`] returns the message plus words consumed so a
//! packet stream parses with one call per packet.

use crate::error::{MidiError, Result};
use crate::message::MidiMessage;
use serde::{Deserialize, Serialize};

/// Highest valid UMP group number.
pub const GROUP_MAX: u8 = 15;
/// SysEx7 payload capacity per packet.
pub const SYSEX7_CAPACITY: usize = 6;
/// SysEx8 payload capacity per packet (excluding the stream id).
pub const SYSEX8_CAPACITY: usize = 13;

/// Complete/Start/Continue/End framing shared by SysEx7 and SysEx8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SysexStatus {
    /// Single packet carries the whole payload.
    Complete = 0,
    /// First packet of a multi-packet payload.
    Start = 1,
    /// Middle packet(s).
    Continue = 2,
    /// Final packet.
    End = 3,
}

impl SysexStatus {
    /// Decode the status nibble.
    pub fn decode(nibble: u8) -> Option<Self> {
        match nibble & 0x03 {
            0 => Some(Self::Complete),
            1 => Some(Self::Start),
            2 => Some(Self::Continue),
            _ => Some(Self::End),
        }
    }
}

/// Utility message operation (type `0x0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UmpUtility {
    /// Status 0: no operation.
    Noop,
    /// Status 1: JR Clock with 16-bit sender timestamp.
    JrClock(u16),
    /// Status 2: JR Timestamp with 16-bit sender timestamp.
    JrTimestamp(u16),
}

/// Note-on/off attribute (low byte of word 0, data in word 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoteAttribute {
    /// Type 0: no attribute (data must be zero).
    None,
    /// Type 1: manufacturer-specific 16-bit value.
    Manufacturer(u16),
    /// Type 2: profile-specific 16-bit value.
    Profile(u16),
    /// Type 3: pitch 7.9 16-bit value.
    Pitch79(u16),
}

impl NoteAttribute {
    /// Decode the attribute type byte plus 16-bit data word.
    pub fn decode(kind: u8, data: u16) -> Option<Self> {
        match kind {
            0x00 => (data == 0).then_some(Self::None),
            0x01 => Some(Self::Manufacturer(data)),
            0x02 => Some(Self::Profile(data)),
            0x03 => Some(Self::Pitch79(data)),
            _ => None,
        }
    }

    /// Encode to `(type byte, data word)`.
    pub fn encode(&self) -> (u8, u16) {
        match self {
            Self::None => (0x00, 0),
            Self::Manufacturer(value) => (0x01, *value),
            Self::Profile(value) => (0x02, *value),
            Self::Pitch79(value) => (0x03, *value),
        }
    }
}

/// MIDI 2.0 Channel Voice message (type `0x4`), group carried alongside.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Midi2Voice {
    /// Status `0x8`: note off with 16-bit velocity and attribute.
    NoteOff {
        /// Channel 0-15.
        channel: u8,
        /// Note number.
        note: u8,
        /// Release velocity.
        velocity: u16,
        /// Note attribute.
        attribute: NoteAttribute,
    },
    /// Status `0x9`: note on with 16-bit velocity and attribute.
    NoteOn {
        /// Channel 0-15.
        channel: u8,
        /// Note number.
        note: u8,
        /// Strike velocity.
        velocity: u16,
        /// Note attribute.
        attribute: NoteAttribute,
    },
    /// Status `0xA`: poly pressure with 32-bit value.
    PolyPressure {
        /// Channel 0-15.
        channel: u8,
        /// Note number.
        note: u8,
        /// Pressure value.
        pressure: u32,
    },
    /// Status `0xB`: control change with 32-bit value.
    ControlChange {
        /// Channel 0-15.
        channel: u8,
        /// Controller index.
        index: u8,
        /// Control value.
        value: u32,
    },
    /// Status `0x0`: registered per-note controller.
    RegisteredPerNote {
        /// Channel 0-15.
        channel: u8,
        /// Note number.
        note: u8,
        /// Controller index.
        index: u8,
        /// Control value.
        value: u32,
    },
    /// Status `0x1`: assignable per-note controller.
    AssignablePerNote {
        /// Channel 0-15.
        channel: u8,
        /// Note number.
        note: u8,
        /// Controller index.
        index: u8,
        /// Control value.
        value: u32,
    },
    /// Status `0x2`: registered (banked) controller.
    Registered {
        /// Channel 0-15.
        channel: u8,
        /// Bank number.
        bank: u8,
        /// Controller index.
        index: u8,
        /// Control value.
        value: u32,
    },
    /// Status `0x3`: assignable (banked) controller.
    Assignable {
        /// Channel 0-15.
        channel: u8,
        /// Bank number.
        bank: u8,
        /// Controller index.
        index: u8,
        /// Control value.
        value: u32,
    },
    /// Status `0x4`: relative registered controller.
    RelativeRegistered {
        /// Channel 0-15.
        channel: u8,
        /// Bank number.
        bank: u8,
        /// Controller index.
        index: u8,
        /// Control value.
        value: u32,
    },
    /// Status `0x5`: relative assignable controller.
    RelativeAssignable {
        /// Channel 0-15.
        channel: u8,
        /// Bank number.
        bank: u8,
        /// Controller index.
        index: u8,
        /// Control value.
        value: u32,
    },
    /// Status `0x6`: per-note pitch bend with 32-bit value.
    PerNotePitchBend {
        /// Channel 0-15.
        channel: u8,
        /// Note number.
        note: u8,
        /// Bend value.
        value: u32,
    },
    /// Status `0xC`: program change with optional 14-bit bank.
    ProgramChange {
        /// Channel 0-15.
        channel: u8,
        /// Program number.
        program: u8,
        /// Bank number (`None` = no bank; word-0 flag clear, bank bytes zero).
        bank: Option<u16>,
    },
    /// Status `0xD`: channel pressure with 32-bit value.
    ChannelPressure {
        /// Channel 0-15.
        channel: u8,
        /// Pressure value.
        value: u32,
    },
    /// Status `0xE`: channel pitch bend with 32-bit value.
    PitchBend {
        /// Channel 0-15.
        channel: u8,
        /// Bend value.
        value: u32,
    },
    /// Status `0xF`: per-note management (reset/detach flags).
    PerNoteManagement {
        /// Channel 0-15.
        channel: u8,
        /// Note number.
        note: u8,
        /// Reset the note.
        reset: bool,
        /// Detach the note.
        detach: bool,
    },
}

/// One Universal MIDI Packet message with its group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UmpMessage {
    /// Type `0x0`: utility message.
    Utility {
        /// Group 0-15.
        group: u8,
        /// Operation.
        op: UmpUtility,
    },
    /// Type `0x1`: system common message.
    SystemCommon {
        /// Group 0-15.
        group: u8,
        /// Status byte (`F1 F2 F3 F6 F8 FA FB FC FE FF`).
        status: u8,
        /// First data byte (zero when the status takes none).
        d1: u8,
        /// Second data byte (zero unless song position).
        d2: u8,
    },
    /// Type `0x2`: MIDI 1.0 channel voice message.
    Midi1Voice {
        /// Group 0-15.
        group: u8,
        /// The channel voice message (note/CC/program/pressure/bend).
        message: Box<MidiMessage>,
    },
    /// Type `0x3`: SysEx7 payload chunk (at most 6 bytes).
    Sysex7 {
        /// Group 0-15.
        group: u8,
        /// Chunk framing.
        status: SysexStatus,
        /// Payload bytes (7-bit each).
        bytes: Vec<u8>,
    },
    /// Type `0x4`: MIDI 2.0 channel voice message.
    Midi2Voice {
        /// Group 0-15.
        group: u8,
        /// The voice message.
        message: Midi2Voice,
    },
    /// Type `0x5`: SysEx8 payload chunk (at most 13 bytes).
    Sysex8 {
        /// Group 0-15.
        group: u8,
        /// Stream id byte.
        stream_id: u8,
        /// Chunk framing.
        status: SysexStatus,
        /// Payload bytes.
        bytes: Vec<u8>,
    },
    /// Types `0x8`-`0xE`: Flex Data as raw validated words.
    FlexData {
        /// Raw packet words.
        words: [u32; 4],
    },
    /// Type `0xF`: UMP Stream message as raw validated words.
    Stream {
        /// Raw packet words.
        words: [u32; 4],
    },
}

/// Words per packet by message type; `None` for reserved types.
pub fn packet_words(message_type: u8) -> Option<usize> {
    match message_type & 0x0F {
        0x0..=0x2 => Some(1),
        0x3 | 0x4 => Some(2),
        0x5 | 0x8 | 0x9 | 0xA | 0xB | 0xC | 0xD | 0xE | 0xF => Some(4),
        _ => None,
    }
}

fn word_bytes(word: u32) -> [u8; 4] {
    word.to_be_bytes()
}

fn group_of(word: u32) -> u8 {
    (word >> 24) as u8 & 0x0F
}

fn check_group(group: u8) -> Result<()> {
    if group > GROUP_MAX {
        return Err(MidiError::InvalidMessage(format!(
            "UMP group out of range: {group}"
        )));
    }
    Ok(())
}

impl UmpMessage {
    /// Encode to packet words (big-endian).
    pub fn encode(&self) -> Result<Vec<u32>> {
        match self {
            Self::Utility { group, op } => {
                check_group(*group)?;
                let word = (u32::from(*group & 0x0F) << 24)
                    | match op {
                        UmpUtility::Noop => 0,
                        UmpUtility::JrClock(time) => 0x0010_0000 | u32::from(*time),
                        UmpUtility::JrTimestamp(time) => 0x0020_0000 | u32::from(*time),
                    };
                Ok(vec![word])
            }
            Self::SystemCommon {
                group,
                status,
                d1,
                d2,
            } => {
                check_group(*group)?;
                check_system_status(*status, *d1, *d2)?;
                Ok(vec![
                    (0x10 | u32::from(*group & 0x0F)) << 24
                        | (u32::from(*status) << 16)
                        | (u32::from(*d1) << 8)
                        | u32::from(*d2),
                ])
            }
            Self::Midi1Voice { group, message } => {
                check_group(*group)?;
                let bytes = message.to_bytes();
                let (status, payload) = match bytes.as_slice() {
                    [status @ (0x80..=0xEF), rest @ ..] => (*status, rest),
                    _ => {
                        return Err(MidiError::InvalidMessage(
                            "UMP MIDI 1.0 voice needs a channel voice message".to_string(),
                        ));
                    }
                };
                check_voice_bytes(status, payload)?;
                let mut word = (0x20 | u32::from(*group & 0x0F)) << 24 | (u32::from(status) << 16);
                if !payload.is_empty() {
                    word |= u32::from(payload[0]) << 8;
                }
                if payload.len() > 1 {
                    word |= u32::from(payload[1]);
                }
                Ok(vec![word])
            }
            Self::Sysex7 {
                group,
                status,
                bytes,
            } => {
                check_group(*group)?;
                if bytes.len() > SYSEX7_CAPACITY
                    || bytes.iter().any(|byte| byte & 0x80 != 0)
                {
                    return Err(MidiError::InvalidMessage(format!(
                        "SysEx7 chunk takes at most {SYSEX7_CAPACITY} 7-bit bytes, got {}",
                        bytes.len()
                    )));
                }
                let mut padded = [0u8; 8];
                padded[2..2 + bytes.len()].copy_from_slice(bytes);
                let word0 = (0x30 | u32::from(*group & 0x0F)) << 24
                    | ((*status as u32) << 20)
                    | ((bytes.len() as u32) << 16)
                    | (u32::from(padded[2]) << 8)
                    | u32::from(padded[3]);
                let word1 = (u32::from(padded[4]) << 24)
                    | (u32::from(padded[5]) << 16)
                    | (u32::from(padded[6]) << 8)
                    | u32::from(padded[7]);
                Ok(vec![word0, word1])
            }
            Self::Midi2Voice { group, message } => {
                check_group(*group)?;
                Ok(encode_voice2(*group, message)?)
            }
            Self::Sysex8 {
                group,
                stream_id,
                status,
                bytes,
            } => {
                check_group(*group)?;
                if bytes.len() > SYSEX8_CAPACITY {
                    return Err(MidiError::InvalidMessage(format!(
                        "SysEx8 chunk takes at most {SYSEX8_CAPACITY} bytes, got {}",
                        bytes.len()
                    )));
                }
                let mut words = [0u32; 4];
                words[0] = (0x50 | u32::from(*group & 0x0F)) << 24
                    | ((*status as u32) << 20)
                    | (((bytes.len() + 1) as u32) << 16)
                    | (u32::from(*stream_id) << 8);
                for (index, byte) in bytes.iter().enumerate() {
                    let at = index + 3;
                    words[at / 4] |= u32::from(*byte) << (8 * (3 - at % 4));
                }
                Ok(words.to_vec())
            }
            Self::FlexData { words } => match (words[0] >> 28) as u8 {
                0x8..=0xE => Ok(words.to_vec()),
                other => Err(MidiError::InvalidMessage(format!(
                    "Flex Data needs type 0x8-0xE, got {other:#X}"
                ))),
            },
            Self::Stream { words } => {
                check_raw_packet(words, 0xF)?;
                Ok(words.to_vec())
            }
        }
    }

    /// Decode one packet from the front of `words`, returning the message
    /// plus words consumed. Reserved types and malformed packets are `None`.
    pub fn decode(words: &[u32]) -> Option<(Self, usize)> {
        let first = *words.first()?;
        let message_type = (first >> 28) as u8;
        let size = packet_words(message_type)?;
        if words.len() < size {
            return None;
        }
        let group = group_of(first);
        if group > GROUP_MAX {
            return None;
        }
        let message = match message_type {
            0x0 => decode_utility(group, first)?,
            0x1 => decode_system(group, first)?,
            0x2 => decode_voice1(group, first)?,
            0x3 => decode_sysex7(group, &words[..2])?,
            0x4 => decode_voice2(group, &words[..2])?,
            0x5 => decode_sysex8(group, &words[..4])?,
            0x8..=0xE => {
                check_raw_packet(
                    &[words[0], words[1], words[2], words[3]],
                    message_type,
                )
                .ok()?;
                Self::FlexData {
                    words: [words[0], words[1], words[2], words[3]],
                }
            }
            0xF => {
                check_raw_packet(&[words[0], words[1], words[2], words[3]], 0xF).ok()?;
                Self::Stream {
                    words: [words[0], words[1], words[2], words[3]],
                }
            }
            _ => return None,
        };
        Some((message, size))
    }
}

/// Validate a raw Flex/Stream packet: exact type match on word 0.
fn check_raw_packet(words: &[u32], message_type: u8) -> Result<()> {
    if ((words[0] >> 28) as u8) != message_type & 0x0F {
        return Err(MidiError::InvalidMessage(format!(
            "raw UMP packet type mismatch: expected {message_type:#X}"
        )));
    }
    Ok(())
}

/// Expected data length by 1.0 status; `None` rejects the status.
fn voice1_len(status: u8) -> Option<usize> {
    match status & 0xF0 {
        0x80 | 0x90 | 0xA0 | 0xB0 | 0xE0 => Some(2),
        0xC0 | 0xD0 => Some(1),
        _ => None,
    }
}

fn check_voice_bytes(status: u8, payload: &[u8]) -> Result<()> {
    let want = voice1_len(status).ok_or_else(|| {
        MidiError::InvalidMessage(format!("bad MIDI 1.0 voice status: {status:#04X}"))
    })?;
    if payload.len() != want || payload.iter().any(|byte| byte & 0x80 != 0) {
        return Err(MidiError::InvalidMessage(format!(
            "bad MIDI 1.0 voice payload for status {status:#04X}"
        )));
    }
    Ok(())
}

/// Expected data length by system status; `None` rejects the status.
fn system_len(status: u8) -> Option<usize> {
    match status {
        0xF1 | 0xF3 => Some(1),
        0xF2 => Some(2),
        0xF6 | 0xF8 | 0xFA | 0xFB | 0xFC | 0xFE | 0xFF => Some(0),
        _ => None,
    }
}

fn check_system_status(status: u8, d1: u8, d2: u8) -> Result<()> {
    let want = system_len(status).ok_or_else(|| {
        MidiError::InvalidMessage(format!("bad UMP system status: {status:#04X}"))
    })?;
    let bytes = [d1, d2];
    if bytes.iter().any(|byte| byte & 0x80 != 0)
        || (want < 2 && d2 != 0)
        || (want < 1 && d1 != 0)
    {
        return Err(MidiError::InvalidMessage(format!(
            "bad UMP system payload for status {status:#04X}"
        )));
    }
    Ok(())
}

fn decode_utility(group: u8, word: u32) -> Option<UmpMessage> {
    let status = ((word >> 16) & 0xF0) as u8;
    let time = (word & 0xFFFF) as u16;
    match status {
        0x00 if word & 0x00FF_FFFF == 0 => Some(UmpMessage::Utility {
            group,
            op: UmpUtility::Noop,
        }),
        0x10 => Some(UmpMessage::Utility {
            group,
            op: UmpUtility::JrClock(time),
        }),
        0x20 => Some(UmpMessage::Utility {
            group,
            op: UmpUtility::JrTimestamp(time),
        }),
        _ => None,
    }
}

fn decode_system(group: u8, word: u32) -> Option<UmpMessage> {
    let bytes = word_bytes(word);
    let (status, d1, d2) = (bytes[1], bytes[2], bytes[3]);
    check_system_status(status, d1, d2).ok()?;
    Some(UmpMessage::SystemCommon {
        group,
        status,
        d1,
        d2,
    })
}

fn decode_voice1(group: u8, word: u32) -> Option<UmpMessage> {
    let bytes = word_bytes(word);
    let (status, d1, d2) = (bytes[1], bytes[2], bytes[3]);
    let want = voice1_len(status)?;
    let payload = match want {
        1 => vec![d1],
        _ => vec![d1, d2],
    };
    check_voice_bytes(status, &payload).ok()?;
    if want == 1 && d2 != 0 {
        return None;
    }
    // A note-on with velocity 0 is preserved as received, not folded to
    // note-off (`MidiMessage::from_bytes` would normalize it).
    if status == 0x90 && d2 == 0 {
        return Some(UmpMessage::Midi1Voice {
            group,
            message: Box::new(MidiMessage::NoteOn {
                channel: status & 0x0F,
                note: d1,
                velocity: 0,
            }),
        });
    }
    let mut bytes = vec![status];
    bytes.extend_from_slice(&payload);
    let message = MidiMessage::from_bytes(&bytes).ok()?;
    match message {
        MidiMessage::SystemExclusive { .. }
        | MidiMessage::System { .. }
        | MidiMessage::MtcFullFrame { .. }
        | MidiMessage::MtcQuarterFrame { .. }
        | MidiMessage::Mmc { .. }
        | MidiMessage::MmcResponse { .. }
        | MidiMessage::IdentityRequest { .. }
        | MidiMessage::IdentityReply { .. }
        | MidiMessage::GmSystem { .. }
        | MidiMessage::MasterControl { .. }
        | MidiMessage::MtsSingleNote { .. }
        | MidiMessage::MtsScaleOctave { .. }
        | MidiMessage::MtsScaleOctave14 { .. }
        | MidiMessage::MtsBulkRequest { .. }
        | MidiMessage::MtsBulkRequestBank { .. }
        | MidiMessage::MtsBulkDump { .. }
        | MidiMessage::MtsScaleOctaveDump { .. }
        | MidiMessage::Raw { .. } => None,
        voice => Some(UmpMessage::Midi1Voice {
            group,
            message: Box::new(voice),
        }),
    }
}

fn decode_sysex7(group: u8, words: &[u32]) -> Option<UmpMessage> {
    let bytes = words_to_bytes(words);
    let (status, count) = (bytes[1] >> 4, bytes[1] & 0x0F);
    if count as usize > SYSEX7_CAPACITY || bytes[2..].iter().any(|b| b & 0x80 != 0) {
        return None;
    }
    Some(UmpMessage::Sysex7 {
        group,
        status: SysexStatus::decode(status)?,
        bytes: bytes[2..2 + count as usize].to_vec(),
    })
}

fn words_to_bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_be_bytes()).collect()
}

fn decode_sysex8(group: u8, words: &[u32]) -> Option<UmpMessage> {
    let bytes = words_to_bytes(words);
    let (status, count) = (bytes[1] >> 4, bytes[1] & 0x0F);
    // The count includes the stream id byte; the payload starts at byte 3.
    if count == 0 || count as usize > SYSEX8_CAPACITY + 1 {
        return None;
    }
    Some(UmpMessage::Sysex8 {
        group,
        stream_id: bytes[2],
        status: SysexStatus::decode(status)?,
        bytes: bytes[3..3 + count as usize - 1].to_vec(),
    })
}

#[allow(clippy::too_many_lines)]
fn encode_voice2(group: u8, message: &Midi2Voice) -> Result<Vec<u32>> {
    let header = |status: u8, channel: u8| -> Result<u32> {
        if channel > 15 {
            return Err(MidiError::InvalidMessage(format!(
                "MIDI 2.0 channel out of range: {channel}"
            )));
        }
        Ok((0x40 | u32::from(group & 0x0F)) << 24 | (u32::from(status) << 20) | (u32::from(channel) << 16))
    };
    let check_note = |note: u8| -> Result<()> {
        if note > 0x7F {
            return Err(MidiError::InvalidMessage(format!(
                "MIDI 2.0 note out of range: {note}"
            )));
        }
        Ok(())
    };
    let check_index = |index: u8, name: &str| -> Result<()> {
        if index > 0x7F {
            return Err(MidiError::InvalidMessage(format!(
                "MIDI 2.0 {name} out of range: {index}"
            )));
        }
        Ok(())
    };
    let mut words = [0u32; 2];
    match message {
        Midi2Voice::NoteOff { .. } | Midi2Voice::NoteOn { .. } => {
            let (status, channel, note, velocity, attribute) = match message {
                Midi2Voice::NoteOff {
                    channel,
                    note,
                    velocity,
                    attribute,
                } => (0x8u8, channel, note, velocity, attribute),
                Midi2Voice::NoteOn {
                    channel,
                    note,
                    velocity,
                    attribute,
                } => (0x9u8, channel, note, velocity, attribute),
                _ => unreachable!(),
            };
            check_note(*note)?;
            let (kind, data) = attribute.encode();
            words[0] = header(status, *channel)?
                | (u32::from(*note) << 8)
                | u32::from(kind);
            words[1] = (u32::from(*velocity) << 16) | u32::from(data);
        }
        Midi2Voice::PolyPressure {
            channel,
            note,
            pressure,
        } => {
            check_note(*note)?;
            words[0] = header(0xA, *channel)? | (u32::from(*note) << 8);
            words[1] = *pressure;
        }
        Midi2Voice::ControlChange {
            channel,
            index,
            value,
        } => {
            check_index(*index, "controller")?;
            words[0] = header(0xB, *channel)? | (u32::from(*index) << 8);
            words[1] = *value;
        }
        Midi2Voice::RegisteredPerNote { .. } | Midi2Voice::AssignablePerNote { .. } => {
            let (status, channel, note, index, value) = match message {
                Midi2Voice::RegisteredPerNote {
                    channel,
                    note,
                    index,
                    value,
                } => (0x0u8, channel, note, index, value),
                Midi2Voice::AssignablePerNote {
                    channel,
                    note,
                    index,
                    value,
                } => (0x1u8, channel, note, index, value),
                _ => unreachable!(),
            };
            check_note(*note)?;
            check_index(*index, "controller")?;
            words[0] = header(status, *channel)?
                | (u32::from(*note) << 8)
                | u32::from(*index);
            words[1] = *value;
        }
        Midi2Voice::Registered { .. }
        | Midi2Voice::Assignable { .. }
        | Midi2Voice::RelativeRegistered { .. }
        | Midi2Voice::RelativeAssignable { .. } => {
            let (status, channel, bank, index, value) = match message {
                Midi2Voice::Registered {
                    channel,
                    bank,
                    index,
                    value,
                } => (0x2u8, channel, bank, index, value),
                Midi2Voice::Assignable {
                    channel,
                    bank,
                    index,
                    value,
                } => (0x3u8, channel, bank, index, value),
                Midi2Voice::RelativeRegistered {
                    channel,
                    bank,
                    index,
                    value,
                } => (0x4u8, channel, bank, index, value),
                Midi2Voice::RelativeAssignable {
                    channel,
                    bank,
                    index,
                    value,
                } => (0x5u8, channel, bank, index, value),
                _ => unreachable!(),
            };
            check_index(*bank, "bank")?;
            check_index(*index, "controller")?;
            words[0] = header(status, *channel)?
                | (u32::from(*bank) << 8)
                | u32::from(*index);
            words[1] = *value;
        }
        Midi2Voice::PerNotePitchBend {
            channel,
            note,
            value,
        } => {
            check_note(*note)?;
            words[0] = header(0x6, *channel)? | (u32::from(*note) << 8);
            words[1] = *value;
        }
        Midi2Voice::ProgramChange {
            channel,
            program,
            bank,
        } => {
            check_index(*program, "program")?;
            let mut word0 = header(0xC, *channel)?;
            let word1 = match bank {
                None => u32::from(*program) << 24,
                Some(bank) => {
                    if *bank > 0x3FFF {
                        return Err(MidiError::InvalidMessage(format!(
                            "MIDI 2.0 program bank out of range: {bank:#X}"
                        )));
                    }
                    word0 |= 0x01;
                    (u32::from(*program) << 24)
                        | (u32::from(*bank & 0x7F) << 8)
                        | u32::from((*bank >> 7) & 0x7F)
                }
            };
            words = [word0, word1];
        }
        Midi2Voice::ChannelPressure { channel, value } => {
            words[0] = header(0xD, *channel)?;
            words[1] = *value;
        }
        Midi2Voice::PitchBend { channel, value } => {
            words[0] = header(0xE, *channel)?;
            words[1] = *value;
        }
        Midi2Voice::PerNoteManagement {
            channel,
            note,
            reset,
            detach,
        } => {
            check_note(*note)?;
            words[0] = header(0xF, *channel)?
                | (u32::from(*note) << 8)
                | (u32::from(u8::from(*reset)) | (u32::from(u8::from(*detach)) << 1));
        }
    }
    Ok(words.to_vec())
}

fn decode_voice2(group: u8, words: &[u32]) -> Option<UmpMessage> {
    let bytes = words_to_bytes(words);
    let (status, channel, b2, b3) = (bytes[1] >> 4, bytes[1] & 0x0F, bytes[2], bytes[3]);
    if channel > 15 {
        return None;
    }
    let word1 = words[1];
    let note = |note: u8| (note <= 0x7F).then_some(note);
    let index = |index: u8| (index <= 0x7F).then_some(index);
    let message = match status {
        0x8 | 0x9 => {
            let attribute = NoteAttribute::decode(b3, (word1 & 0xFFFF) as u16)?;
            let velocity = (word1 >> 16) as u16;
            let note = note(b2)?;
            if status == 0x8 {
                Some(Midi2Voice::NoteOff {
                    channel,
                    note,
                    velocity,
                    attribute,
                })
            } else {
                Some(Midi2Voice::NoteOn {
                    channel,
                    note,
                    velocity,
                    attribute,
                })
            }
        }
        0xA => Some(Midi2Voice::PolyPressure {
            channel,
            note: note(b2)?,
            pressure: word1,
        }),
        0xB => Some(Midi2Voice::ControlChange {
            channel,
            index: index(b2)?,
            value: word1,
        }),
        0x0 | 0x1 => {
            let (note, index) = (note(b2)?, index(b3)?);
            if status == 0x0 {
                Some(Midi2Voice::RegisteredPerNote {
                    channel,
                    note,
                    index,
                    value: word1,
                })
            } else {
                Some(Midi2Voice::AssignablePerNote {
                    channel,
                    note,
                    index,
                    value: word1,
                })
            }
        }
        0x2..=0x5 => {
            let (bank, index) = (index(b2)?, index(b3)?);
            let fields = (channel, bank, index, word1);
            match status {
                0x2 => Some(Midi2Voice::Registered {
                    channel: fields.0,
                    bank: fields.1,
                    index: fields.2,
                    value: fields.3,
                }),
                0x3 => Some(Midi2Voice::Assignable {
                    channel: fields.0,
                    bank: fields.1,
                    index: fields.2,
                    value: fields.3,
                }),
                0x4 => Some(Midi2Voice::RelativeRegistered {
                    channel: fields.0,
                    bank: fields.1,
                    index: fields.2,
                    value: fields.3,
                }),
                _ => Some(Midi2Voice::RelativeAssignable {
                    channel: fields.0,
                    bank: fields.1,
                    index: fields.2,
                    value: fields.3,
                }),
            }
        }
        0x6 => Some(Midi2Voice::PerNotePitchBend {
            channel,
            note: note(b2)?,
            value: word1,
        }),
        0xC => {
            if b2 != 0 || b3 & 0xFE != 0 {
                return None;
            }
            // Word 1 holds program (byte 0), reserved zero (byte 1), and
            // bank LSB/MSB (bytes 2-3).
            let word = words_to_bytes(&words[1..2]);
            let program = index(word[0])?;
            if word[1] != 0 || word[2] > 0x7F || word[3] > 0x7F {
                return None;
            }
            let bank = if b3 & 0x01 != 0 {
                Some((u16::from(word[3]) << 7) | u16::from(word[2]))
            } else {
                if word1 != (u32::from(program) << 24) {
                    return None;
                }
                None
            };
            Some(Midi2Voice::ProgramChange {
                channel,
                program,
                bank,
            })
        }
        0xD => {
            if b2 != 0 || b3 != 0 {
                return None;
            }
            Some(Midi2Voice::ChannelPressure {
                channel,
                value: word1,
            })
        }
        0xE => {
            if b2 != 0 || b3 != 0 {
                return None;
            }
            Some(Midi2Voice::PitchBend {
                channel,
                value: word1,
            })
        }
        0xF => {
            let note = note(b2)?;
            if b3 & 0xFC != 0 || word1 != 0 {
                return None;
            }
            Some(Midi2Voice::PerNoteManagement {
                channel,
                note,
                reset: b3 & 0x01 != 0,
                detach: b3 & 0x02 != 0,
            })
        }
        _ => return None,
    };
    Some(UmpMessage::Midi2Voice { group, message: message? })
}

/// Split a SysEx7 payload into framed chunks (Complete for one,
/// Start/Continue/End otherwise). Payload bytes must be 7-bit.
pub fn sysex7_packets(group: u8, bytes: &[u8]) -> Result<Vec<UmpMessage>> {
    chunk_packets(group, bytes, SYSEX7_CAPACITY, true, |group, status, chunk| {
        UmpMessage::Sysex7 {
            group,
            status,
            bytes: chunk.to_vec(),
        }
    })
}

/// Split a SysEx8 payload into framed chunks with a stream id.
pub fn sysex8_packets(
    group: u8,
    stream_id: u8,
    bytes: &[u8],
) -> Result<Vec<UmpMessage>> {
    chunk_packets(
        group,
        bytes,
        SYSEX8_CAPACITY,
        false,
        |group, status, chunk| UmpMessage::Sysex8 {
            group,
            stream_id,
            status,
            bytes: chunk.to_vec(),
        },
    )
}

fn chunk_packets(
    group: u8,
    bytes: &[u8],
    capacity: usize,
    seven_bit: bool,
    make: impl Fn(u8, SysexStatus, &[u8]) -> UmpMessage,
) -> Result<Vec<UmpMessage>> {
    check_group(group)?;
    if seven_bit && bytes.iter().any(|byte| byte & 0x80 != 0) {
        return Err(MidiError::InvalidMessage(
            "SysEx7 payload must be 7-bit".to_string(),
        ));
    }
    let mut packets = Vec::new();
    let chunks: Vec<&[u8]> = if bytes.is_empty() {
        vec![&[]]
    } else {
        bytes.chunks(capacity).collect()
    };
    for (index, chunk) in chunks.iter().enumerate() {
        let status = if chunks.len() == 1 {
            SysexStatus::Complete
        } else if index == 0 {
            SysexStatus::Start
        } else if index + 1 == chunks.len() {
            SysexStatus::End
        } else {
            SysexStatus::Continue
        };
        packets.push(make(group, status, chunk));
    }
    Ok(packets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_sizes_follow_message_type() {
        assert_eq!(packet_words(0x0), Some(1));
        assert_eq!(packet_words(0x2), Some(1));
        assert_eq!(packet_words(0x3), Some(2));
        assert_eq!(packet_words(0x4), Some(2));
        assert_eq!(packet_words(0x5), Some(4));
        assert_eq!(packet_words(0x8), Some(4));
        assert_eq!(packet_words(0xE), Some(4));
        assert_eq!(packet_words(0xF), Some(4));
        assert_eq!(packet_words(0x6), None);
        assert_eq!(packet_words(0x7), None);
    }

    #[test]
    fn external_v1_note_on_vector() {
        // midi2 reference crate, channel_voice1/note_on.rs setters test:
        // group 0xD, channel 0xE, note 0x75, velocity 0x3D.
        let (message, used) = UmpMessage::decode(&[0x2D9E_753D]).unwrap();
        assert_eq!(used, 1);
        match &message {
            UmpMessage::Midi1Voice { group, message } => {
                assert_eq!((*group, message.to_bytes()), (0x0D, vec![0x9E, 0x75, 0x3D]));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(message.encode().unwrap(), vec![0x2D9E_753D]);
    }

    #[test]
    fn external_v2_note_on_vector() {
        // midi2 reference crate, channel_voice2/note_on.rs builder test:
        // group 8, channel 8, note 0x5E, velocity 0x6A14, pitch attribute.
        let (message, used) = UmpMessage::decode(&[0x4898_5E03, 0x6A14_E98A]).unwrap();
        assert_eq!(used, 2);
        assert_eq!(
            message,
            UmpMessage::Midi2Voice {
                group: 8,
                message: Midi2Voice::NoteOn {
                    channel: 8,
                    note: 0x5E,
                    velocity: 0x6A14,
                    attribute: NoteAttribute::Pitch79(0xE98A),
                },
            }
        );
        assert_eq!(message.encode().unwrap(), vec![0x4898_5E03, 0x6A14_E98A]);
    }

    #[test]
    fn external_v2_program_vector() {
        // midi2 reference crate, channel_voice2/program_change.rs builder
        // test: group 0xF, channel 0xE, program 0x75, bank 0x1F5E.
        let (message, used) = UmpMessage::decode(&[0x4FCE_0001, 0x7500_5E3E]).unwrap();
        assert_eq!(used, 2);
        assert_eq!(
            message,
            UmpMessage::Midi2Voice {
                group: 0x0F,
                message: Midi2Voice::ProgramChange {
                    channel: 0x0E,
                    program: 0x75,
                    bank: Some(0x1F5E),
                },
            }
        );
        assert_eq!(message.encode().unwrap(), vec![0x4FCE_0001, 0x7500_5E3E]);
    }

    #[test]
    fn external_sysex8_vector() {
        // midi2 reference crate, sysex8/packet.rs payload_full test:
        // stream 0 (word0 byte 2), complete, 13 payload bytes 0x01-0x0D.
        let words = [0x500E_0001, 0x0203_0405, 0x0607_0809, 0x0A0B_0C0D];
        let (message, used) = UmpMessage::decode(&words).unwrap();
        assert_eq!(used, 4);
        assert_eq!(
            message,
            UmpMessage::Sysex8 {
                group: 0,
                stream_id: 0,
                status: SysexStatus::Complete,
                bytes: (0x01..=0x0D).collect(),
            }
        );
        assert_eq!(message.encode().unwrap(), words.to_vec());
        // midi2 reference crate, sysex8/packet.rs stream_id test:
        // stream 1 lives in word0 byte 2, payload is empty.
        let (message, used) = UmpMessage::decode(&[0x5001_0100, 0, 0, 0]).unwrap();
        assert_eq!(used, 4);
        assert_eq!(
            message,
            UmpMessage::Sysex8 {
                group: 0,
                stream_id: 1,
                status: SysexStatus::Complete,
                bytes: vec![],
            }
        );
        assert_eq!(message.encode().unwrap(), vec![0x5001_0100, 0, 0, 0]);
    }

    /// Every committed vector parsed by the external `midi2` reference
    /// crate agrees with our decoder field-for-field.
    #[test]
    fn cross_check_vectors_against_midi2_crate() {
        use midi2::{Channeled, Grouped};

        // MIDI 1.0 note-on.
        let reference =
            midi2::channel_voice1::NoteOn::try_from(&[0x2D9E_753D_u32][..]).unwrap();
        let UmpMessage::Midi1Voice { group, message } =
            &UmpMessage::decode(&[0x2D9E_753D]).unwrap().0
        else {
            panic!("expected Midi1Voice");
        };
        let MidiMessage::NoteOn {
            channel,
            note,
            velocity,
        } = message.as_ref()
        else {
            panic!("expected NoteOn");
        };
        assert_eq!(
            (*group, *channel, *note, *velocity),
            (
                u8::from(reference.group()),
                u8::from(reference.channel()),
                u8::from(reference.note_number()),
                u8::from(reference.velocity()),
            )
        );

        // MIDI 2.0 note-on with pitch attribute.
        let reference =
            midi2::channel_voice2::NoteOn::try_from(&[0x4898_5E03_u32, 0x6A14_E98A][..])
                .unwrap();
        let UmpMessage::Midi2Voice { group, message } =
            &UmpMessage::decode(&[0x4898_5E03, 0x6A14_E98A]).unwrap().0
        else {
            panic!("expected Midi2Voice");
        };
        let Midi2Voice::NoteOn {
            channel,
            note,
            velocity,
            attribute,
        } = message
        else {
            panic!("expected NoteOn");
        };
        assert_eq!(
            (*group, *channel, *note, *velocity),
            (
                u8::from(reference.group()),
                u8::from(reference.channel()),
                u8::from(reference.note_number()),
                reference.velocity(),
            )
        );
        let midi2::channel_voice2::NoteAttribute::Pitch7_9(fixed) =
            reference.attribute().unwrap()
        else {
            panic!("expected pitch attribute");
        };
        assert_eq!(*attribute, NoteAttribute::Pitch79(fixed.to_bits()));

        // MIDI 2.0 program change with bank.
        let reference =
            midi2::channel_voice2::ProgramChange::try_from(&[0x4FCE_0001_u32, 0x7500_5E3E][..])
                .unwrap();
        let UmpMessage::Midi2Voice { group, message } =
            &UmpMessage::decode(&[0x4FCE_0001, 0x7500_5E3E]).unwrap().0
        else {
            panic!("expected Midi2Voice");
        };
        let Midi2Voice::ProgramChange {
            channel,
            program,
            bank,
        } = message
        else {
            panic!("expected ProgramChange");
        };
        assert_eq!(
            (*group, *channel, *program, *bank),
            (
                u8::from(reference.group()),
                u8::from(reference.channel()),
                u8::from(reference.program()),
                reference.bank().map(u16::from),
            )
        );

        // SysEx8 full packet.
        let words = [0x500E_0001_u32, 0x0203_0405, 0x0607_0809, 0x0A0B_0C0D];
        let reference = midi2::sysex8::Packet::try_from(&words[..]).unwrap();
        let UmpMessage::Sysex8 {
            group,
            stream_id,
            status,
            bytes,
        } = &UmpMessage::decode(&words).unwrap().0
        else {
            panic!("expected Sysex8");
        };
        assert_eq!(*group, u8::from(reference.group()));
        assert_eq!(*stream_id, reference.stream_id());
        // `Status` lives in midi2's private `packet` module, so compare
        // its debug form (dev-dependency is version-pinned).
        assert_eq!(format!("{:?}", reference.status()), "Complete");
        assert_eq!(*status, SysexStatus::Complete);
        assert_eq!(*bytes, reference.payload().collect::<Vec<u8>>());

        // A packet built by our SysEx7 chunker parses in the reference crate.
        let packets = sysex7_packets(3, &[0x41, 0x10, 0x42]).unwrap();
        assert_eq!(packets.len(), 1);
        let words = packets[0].encode().unwrap();
        let reference = midi2::sysex7::Packet::try_from(&words[..]).unwrap();
        assert_eq!(format!("{:?}", reference.status()), "Complete");
        assert_eq!(
            reference.payload().map(u8::from).collect::<Vec<u8>>(),
            vec![0x41, 0x10, 0x42]
        );

        // System common time code.
        let reference =
            midi2::system_common::TimeCode::try_from(&[0x15F1_5F00_u32][..]).unwrap();
        let UmpMessage::SystemCommon {
            group,
            status,
            d1,
            d2,
        } = &UmpMessage::decode(&[0x15F1_5F00]).unwrap().0
        else {
            panic!("expected SystemCommon");
        };
        assert_eq!(
            (*group, *status, *d1, *d2),
            (
                u8::from(reference.group()),
                0xF1,
                u8::from(reference.time_code()),
                0
            )
        );
    }

    #[test]
    fn external_system_common_vector() {
        // midi2 reference crate, system_common/time_code.rs: group 5,
        // MTC quarter-frame 0x5F.
        let (message, used) = UmpMessage::decode(&[0x15F1_5F00]).unwrap();
        assert_eq!(used, 1);
        assert_eq!(
            message,
            UmpMessage::SystemCommon {
                group: 5,
                status: 0xF1,
                d1: 0x5F,
                d2: 0,
            }
        );
        assert_eq!(message.encode().unwrap(), vec![0x15F1_5F00]);
    }

    #[test]
    fn utility_round_trips() {
        for (op, word) in [
            (UmpUtility::Noop, 0x0000_0000u32),
            (UmpUtility::JrClock(0x1234), 0x0010_1234),
            (UmpUtility::JrTimestamp(0xABCD), 0x0020_ABCD),
        ] {
            let message = UmpMessage::Utility { group: 0, op };
            assert_eq!(message.encode().unwrap(), vec![word]);
            assert_eq!(UmpMessage::decode(&[word]).unwrap().0, message);
        }
        assert_eq!(UmpMessage::decode(&[0x0030_0000]), None);
    }

    #[test]
    fn voice1_rejects_non_voice_and_zero_pads() {
        // Tune request is system, not voice.
        assert_eq!(UmpMessage::decode(&[0x20F6_0000]), None);
        // Zero velocity decodes as a velocity-0 note-on.
        assert_eq!(UmpMessage::decode(&[0x2090_4000]), Some((
            UmpMessage::Midi1Voice {
                group: 0,
                message: Box::new(MidiMessage::NoteOn {
                    channel: 0,
                    note: 64,
                    velocity: 0,
                }),
            },
            1
        )));
        // Program change carries one data byte with a zero pad.
        assert_eq!(
            UmpMessage::decode(&[0x20C0_0500]).unwrap().0,
            UmpMessage::Midi1Voice {
                group: 0,
                message: Box::new(MidiMessage::ProgramChange {
                    channel: 0,
                    program: 5,
                }),
            }
        );
        // A nonzero pad byte is rejected.
        assert_eq!(UmpMessage::decode(&[0x20C0_0507]), None);
    }

    #[test]
    fn sysex7_chunks_and_rejects_high_bits() {
        let packets = sysex7_packets(2, &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07]).unwrap();
        assert_eq!(packets.len(), 2);
        assert_eq!(
            packets[0].encode().unwrap(),
            vec![0x3216_0102, 0x0304_0506]
        );
        assert_eq!(
            packets[1].encode().unwrap(),
            vec![0x3231_0700, 0x0000_0000]
        );
        for packet in &packets {
            let words = packet.encode().unwrap();
            let (decoded, used) = UmpMessage::decode(&words).unwrap();
            assert_eq!(used, 2);
            assert_eq!(&decoded, packet);
        }
        assert!(sysex7_packets(0, &[0x80]).is_err());
        // numBytes above capacity is rejected.
        assert_eq!(UmpMessage::decode(&[0x3307_0000, 0x0000_0000]), None);
    }

    #[test]
    fn voice2_full_set_round_trips() {
        let messages = [
            Midi2Voice::NoteOff {
                channel: 1,
                note: 60,
                velocity: 1000,
                attribute: NoteAttribute::None,
            },
            Midi2Voice::PolyPressure {
                channel: 2,
                note: 61,
                pressure: 0x1234_5678,
            },
            Midi2Voice::ControlChange {
                channel: 3,
                index: 10,
                value: 0xDEAD_BEEF,
            },
            Midi2Voice::RegisteredPerNote {
                channel: 4,
                note: 62,
                index: 3,
                value: 42,
            },
            Midi2Voice::Registered {
                channel: 5,
                bank: 1,
                index: 2,
                value: 0xABCDEF,
            },
            Midi2Voice::RelativeAssignable {
                channel: 6,
                bank: 0,
                index: 0,
                value: 0x8000_0001,
            },
            Midi2Voice::PerNotePitchBend {
                channel: 7,
                note: 63,
                value: 0x8000_0000,
            },
            Midi2Voice::ProgramChange {
                channel: 8,
                program: 10,
                bank: None,
            },
            Midi2Voice::ChannelPressure {
                channel: 9,
                value: 7,
            },
            Midi2Voice::PitchBend {
                channel: 10,
                value: 0x8000_0000,
            },
            Midi2Voice::PerNoteManagement {
                channel: 11,
                note: 64,
                reset: true,
                detach: false,
            },
        ];
        for message in messages {
            let packet = UmpMessage::Midi2Voice { group: 0, message };
            let words = packet.encode().unwrap();
            assert_eq!(words.len(), 2);
            let (decoded, used) = UmpMessage::decode(&words).unwrap();
            assert_eq!(used, 2);
            assert_eq!(decoded, packet);
        }
    }

    #[test]
    fn flex_stream_passthrough_validates_type() {
        // Flex Data text packet from the midi2 packets.rs docs.
        let words = [0xD050_0101, 0x5368_6164, 0x6F77_7320, 0x6F66_2074];
        let (message, used) = UmpMessage::decode(&words).unwrap();
        assert_eq!(used, 4);
        assert_eq!(message.encode().unwrap(), words.to_vec());
        // Wrong wrapper for the type is rejected.
        assert_eq!(
            UmpMessage::Stream { words }.encode().ok(),
            None
        );
        // Reserved types are rejected.
        assert_eq!(UmpMessage::decode(&[0x6000_0000]), None);
        // Short streams are rejected.
        assert_eq!(UmpMessage::decode(&[0xF000_0000]), None);
    }
}
