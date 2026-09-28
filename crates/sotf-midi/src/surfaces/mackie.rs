//! Mackie Control Universal (MCU) protocol: first-class surface support.
//!
//! Wire map verified against the TouchMCU protocol writeup (reverse
//! engineered against MCU Pro firmware, cross-checked with the Logic
//! Control lineage) and a hardware-mapped MCU MIDI table:
//! - Buttons and button LEDs: Note On (velocity 127)/Note Off on channel 0.
//! - Faders (8 strips + master): Pitch Bend on channels 0-8, full 14-bit.
//! - VPot rotation: CC 16-23, sign-magnitude relative (+1 CW .. -1 CCW,
//!   wider magnitudes likely encode acceleration but are untested).
//! - VPot LED rings: CC 48-55, `0LMMVVVV` (center LED, mode, position).
//! - Jog wheel: CC 60, same relative encoding as VPots.
//! - Time display: CC 64-73 (10 digits, right-to-left) plus assignment
//!   CC 74-75, using the 6-bit special-ASCII table with bit 6 as the
//!   decimal-point flag.
//! - Meters: Channel Pressure on channel 0, value `(strip << 4) | level`.
//! - LCD: SysEx `F0 00 00 66 14 12 <offset> <text> F7`, 2 rows x 56 chars.
//! - Host commands: handshake (0x00-0x03), backlight (0x0B), touchless
//!   faders (0x0C), touch sensitivity (0x0E), meter modes (0x20-0x21),
//!   all-LEDs-off (0x62), reset (0x63), faders-to-minimum (0x61).
//!
//! The challenge-response handshake (0x01/0x02) is intentionally
//! pass-through only: the response algorithm is proprietary and cannot be
//! conformance-tested here.

use crate::error::{MidiError, Result};
use crate::message::MidiMessage;
use serde::{Deserialize, Serialize};

/// MIDI channel (0-based) carrying all MCU traffic.
pub const CHANNEL: u8 = 0;
/// First VPot rotation CC (strips 0-7 map to CC 16-23).
pub const VPOT_CC_START: u8 = 16;
/// First VPot LED-ring CC (strips 0-7 map to CC 48-55).
pub const RING_CC_START: u8 = 48;
/// Jog/scrub wheel CC.
pub const JOG_CC: u8 = 60;
/// First time-display digit CC (10 digits on CC 64-73, right-to-left).
pub const TIME_CC_START: u8 = 64;
/// First assignment-display digit CC (2 digits on CC 74-75).
pub const ASSIGN_CC_START: u8 = 74;
/// Mackie SysEx manufacturer id + MCU device id header.
pub const SYSEX_HEADER: [u8; 4] = [0x00, 0x00, 0x66, 0x14];
/// LCD text rows of 56 characters; offsets 0x00-0x37 (top) / 0x38-0x6F.
pub const LCD_ROW_CHARS: usize = 56;
/// Maximum LCD payload bytes per message.
pub const LCD_MAX_BYTES: usize = 112;

/// VPot assign modes (the ASSIGN button block, notes 40-45).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MackieAssign {
    /// Note 40.
    Track,
    /// Note 41.
    Send,
    /// Note 42.
    PanSurround,
    /// Note 43.
    Plugin,
    /// Note 44.
    Eq,
    /// Note 45.
    Instrument,
}

/// Mixer view keys (notes 62-69).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MackieView {
    /// Note 62.
    MidiTracks,
    /// Note 63.
    Inputs,
    /// Note 64.
    AudioTracks,
    /// Note 65.
    AudioInstruments,
    /// Note 66.
    Aux,
    /// Note 67.
    Busses,
    /// Note 68.
    Outputs,
    /// Note 69.
    User,
}

/// Transport buttons (notes 91-95).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MackieTransport {
    /// Note 91.
    Rewind,
    /// Note 92.
    FastForward,
    /// Note 93.
    Stop,
    /// Note 94.
    Play,
    /// Note 95.
    Record,
}

/// Cursor keys (notes 96-99).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MackieCursor {
    /// Note 96.
    Up,
    /// Note 97.
    Down,
    /// Note 98.
    Left,
    /// Note 99.
    Right,
}

/// VPot LED-ring display modes (value bits 5-4 of CC 48-55).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MackieRingMode {
    /// 00: single dot.
    SingleDot = 0,
    /// 01: boost/cut from centre.
    BoostCut = 1,
    /// 10: wrap (fill from the left).
    Wrap = 2,
    /// 11: spread (symmetrical about centre).
    Spread = 3,
}

impl MackieRingMode {
    /// Decode the mode bits.
    pub fn from_bits(bits: u8) -> Self {
        match bits & 0x03 {
            0 => Self::SingleDot,
            1 => Self::BoostCut,
            2 => Self::Wrap,
            _ => Self::Spread,
        }
    }
}

/// Meter levels for channel pressure (low nibble); strip id rides the high
/// nibble. Levels follow the documented dB mapping; overload states only
/// render in horizontal LCD meter mode on hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MackieMeterLevel {
    /// 0x0: below -60 dB, all LEDs off.
    Off = 0,
    /// 0x1: >= -60 dB.
    Minus60Db = 1,
    /// 0x2: >= -50 dB.
    Minus50Db = 2,
    /// 0x3: >= -40 dB.
    Minus40Db = 3,
    /// 0x4: >= -30 dB.
    Minus30Db = 4,
    /// 0x5: >= -20 dB.
    Minus20Db = 5,
    /// 0x6: >= -14 dB.
    Minus14Db = 6,
    /// 0x7: >= -10 dB.
    Minus10Db = 7,
    /// 0x8: >= -8 dB.
    Minus8Db = 8,
    /// 0x9: >= -6 dB.
    Minus6Db = 9,
    /// 0xA: >= -4 dB.
    Minus4Db = 10,
    /// 0xB: >= -2 dB.
    Minus2Db = 11,
    /// 0xC: 0 dB (clip).
    Clip = 12,
    /// 0xD: above 0 dB.
    Over = 13,
    /// 0xE: set overload latch.
    SetOverload = 14,
    /// 0xF: clear overload latch.
    ClearOverload = 15,
}

impl MackieMeterLevel {
    /// Decode the level nibble.
    pub fn from_nibble(nibble: u8) -> Option<Self> {
        match nibble & 0x0F {
            0 => Some(Self::Off),
            1 => Some(Self::Minus60Db),
            2 => Some(Self::Minus50Db),
            3 => Some(Self::Minus40Db),
            4 => Some(Self::Minus30Db),
            5 => Some(Self::Minus20Db),
            6 => Some(Self::Minus14Db),
            7 => Some(Self::Minus10Db),
            8 => Some(Self::Minus8Db),
            9 => Some(Self::Minus6Db),
            10 => Some(Self::Minus4Db),
            11 => Some(Self::Minus2Db),
            12 => Some(Self::Clip),
            13 => Some(Self::Over),
            14 => Some(Self::SetOverload),
            _ => Some(Self::ClearOverload),
        }
    }
}

/// Every MCU button/LED by note number (all on channel 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MackieButton {
    /// Notes 0-7: record-arm per strip.
    Rec(u8),
    /// Notes 8-15: solo per strip.
    Solo(u8),
    /// Notes 16-23: mute per strip.
    Mute(u8),
    /// Notes 24-31: select per strip.
    Select(u8),
    /// Notes 32-39: VPot push per strip.
    VPotSwitch(u8),
    /// Notes 40-45: VPot assign block.
    Assign(MackieAssign),
    /// Note 46.
    BankLeft,
    /// Note 47.
    BankRight,
    /// Note 48.
    ChannelLeft,
    /// Note 49.
    ChannelRight,
    /// Note 50.
    Flip,
    /// Note 51.
    GlobalView,
    /// Note 52: display name/value toggle.
    DisplayNameValue,
    /// Note 53: display SMPTE/beats toggle.
    DisplaySmpteBeats,
    /// Notes 54-61: function keys F1-F8 (parameter 1-8).
    Function(u8),
    /// Notes 62-69: mixer view keys.
    View(MackieView),
    /// Note 70.
    Shift,
    /// Note 71.
    Option,
    /// Note 72.
    Control,
    /// Note 73.
    Alt,
    /// Note 74.
    ReadOff,
    /// Note 75.
    Write,
    /// Note 76.
    Trim,
    /// Note 77.
    Touch,
    /// Note 78.
    Latch,
    /// Note 79.
    Group,
    /// Note 80.
    Save,
    /// Note 81.
    Undo,
    /// Note 82.
    Cancel,
    /// Note 83.
    Enter,
    /// Note 84.
    Markers,
    /// Note 85.
    Nudge,
    /// Note 86.
    Cycle,
    /// Note 87.
    Drop,
    /// Note 88.
    Replace,
    /// Note 89.
    Click,
    /// Note 90: global solo indicator.
    GlobalSolo,
    /// Notes 91-95: transport block.
    Transport(MackieTransport),
    /// Notes 96-99: cursor keys.
    Cursor(MackieCursor),
    /// Note 100.
    Zoom,
    /// Note 101.
    Scrub,
    /// Notes 102-103: user switches (parameter 1-2).
    UserSwitch(u8),
    /// Notes 104-111: fader touch per strip.
    FaderTouch(u8),
    /// Note 112: master fader touch.
    MasterTouch,
    /// Note 113: SMPTE LED.
    SmpteLed,
    /// Note 114: beats LED.
    BeatsLed,
    /// Note 115: rude-solo LED.
    RudeSoloLed,
    /// Note 118: relay click.
    RelayClick,
}

impl MackieButton {
    /// The MIDI note number for this button.
    pub fn note(&self) -> u8 {
        match self {
            Self::Rec(strip) => strip & 0x07,
            Self::Solo(strip) => 8 + (strip & 0x07),
            Self::Mute(strip) => 16 + (strip & 0x07),
            Self::Select(strip) => 24 + (strip & 0x07),
            Self::VPotSwitch(strip) => 32 + (strip & 0x07),
            Self::Assign(assign) => match assign {
                MackieAssign::Track => 40,
                MackieAssign::Send => 41,
                MackieAssign::PanSurround => 42,
                MackieAssign::Plugin => 43,
                MackieAssign::Eq => 44,
                MackieAssign::Instrument => 45,
            },
            Self::BankLeft => 46,
            Self::BankRight => 47,
            Self::ChannelLeft => 48,
            Self::ChannelRight => 49,
            Self::Flip => 50,
            Self::GlobalView => 51,
            Self::DisplayNameValue => 52,
            Self::DisplaySmpteBeats => 53,
            Self::Function(index) => 53 + (*index).clamp(1, 8),
            Self::View(view) => match view {
                MackieView::MidiTracks => 62,
                MackieView::Inputs => 63,
                MackieView::AudioTracks => 64,
                MackieView::AudioInstruments => 65,
                MackieView::Aux => 66,
                MackieView::Busses => 67,
                MackieView::Outputs => 68,
                MackieView::User => 69,
            },
            Self::Shift => 70,
            Self::Option => 71,
            Self::Control => 72,
            Self::Alt => 73,
            Self::ReadOff => 74,
            Self::Write => 75,
            Self::Trim => 76,
            Self::Touch => 77,
            Self::Latch => 78,
            Self::Group => 79,
            Self::Save => 80,
            Self::Undo => 81,
            Self::Cancel => 82,
            Self::Enter => 83,
            Self::Markers => 84,
            Self::Nudge => 85,
            Self::Cycle => 86,
            Self::Drop => 87,
            Self::Replace => 88,
            Self::Click => 89,
            Self::GlobalSolo => 90,
            Self::Transport(transport) => match transport {
                MackieTransport::Rewind => 91,
                MackieTransport::FastForward => 92,
                MackieTransport::Stop => 93,
                MackieTransport::Play => 94,
                MackieTransport::Record => 95,
            },
            Self::Cursor(cursor) => match cursor {
                MackieCursor::Up => 96,
                MackieCursor::Down => 97,
                MackieCursor::Left => 98,
                MackieCursor::Right => 99,
            },
            Self::Zoom => 100,
            Self::Scrub => 101,
            Self::UserSwitch(index) => 101 + (*index).clamp(1, 2),
            Self::FaderTouch(strip) => 104 + (strip & 0x07),
            Self::MasterTouch => 112,
            Self::SmpteLed => 113,
            Self::BeatsLed => 114,
            Self::RudeSoloLed => 115,
            Self::RelayClick => 118,
        }
    }

    /// Decode a note number into a button. Notes 116, 117, 119+ are
    /// unassigned and return `None`.
    pub fn from_note(note: u8) -> Option<Self> {
        match note {
            0..=7 => Some(Self::Rec(note)),
            8..=15 => Some(Self::Solo(note - 8)),
            16..=23 => Some(Self::Mute(note - 16)),
            24..=31 => Some(Self::Select(note - 24)),
            32..=39 => Some(Self::VPotSwitch(note - 32)),
            40 => Some(Self::Assign(MackieAssign::Track)),
            41 => Some(Self::Assign(MackieAssign::Send)),
            42 => Some(Self::Assign(MackieAssign::PanSurround)),
            43 => Some(Self::Assign(MackieAssign::Plugin)),
            44 => Some(Self::Assign(MackieAssign::Eq)),
            45 => Some(Self::Assign(MackieAssign::Instrument)),
            46 => Some(Self::BankLeft),
            47 => Some(Self::BankRight),
            48 => Some(Self::ChannelLeft),
            49 => Some(Self::ChannelRight),
            50 => Some(Self::Flip),
            51 => Some(Self::GlobalView),
            52 => Some(Self::DisplayNameValue),
            53 => Some(Self::DisplaySmpteBeats),
            54..=61 => Some(Self::Function(note - 53)),
            62 => Some(Self::View(MackieView::MidiTracks)),
            63 => Some(Self::View(MackieView::Inputs)),
            64 => Some(Self::View(MackieView::AudioTracks)),
            65 => Some(Self::View(MackieView::AudioInstruments)),
            66 => Some(Self::View(MackieView::Aux)),
            67 => Some(Self::View(MackieView::Busses)),
            68 => Some(Self::View(MackieView::Outputs)),
            69 => Some(Self::View(MackieView::User)),
            70 => Some(Self::Shift),
            71 => Some(Self::Option),
            72 => Some(Self::Control),
            73 => Some(Self::Alt),
            74 => Some(Self::ReadOff),
            75 => Some(Self::Write),
            76 => Some(Self::Trim),
            77 => Some(Self::Touch),
            78 => Some(Self::Latch),
            79 => Some(Self::Group),
            80 => Some(Self::Save),
            81 => Some(Self::Undo),
            82 => Some(Self::Cancel),
            83 => Some(Self::Enter),
            84 => Some(Self::Markers),
            85 => Some(Self::Nudge),
            86 => Some(Self::Cycle),
            87 => Some(Self::Drop),
            88 => Some(Self::Replace),
            89 => Some(Self::Click),
            90 => Some(Self::GlobalSolo),
            91 => Some(Self::Transport(MackieTransport::Rewind)),
            92 => Some(Self::Transport(MackieTransport::FastForward)),
            93 => Some(Self::Transport(MackieTransport::Stop)),
            94 => Some(Self::Transport(MackieTransport::Play)),
            95 => Some(Self::Transport(MackieTransport::Record)),
            96 => Some(Self::Cursor(MackieCursor::Up)),
            97 => Some(Self::Cursor(MackieCursor::Down)),
            98 => Some(Self::Cursor(MackieCursor::Left)),
            99 => Some(Self::Cursor(MackieCursor::Right)),
            100 => Some(Self::Zoom),
            101 => Some(Self::Scrub),
            102..=103 => Some(Self::UserSwitch(note - 101)),
            104..=111 => Some(Self::FaderTouch(note - 104)),
            112 => Some(Self::MasterTouch),
            113 => Some(Self::SmpteLed),
            114 => Some(Self::BeatsLed),
            115 => Some(Self::RudeSoloLed),
            118 => Some(Self::RelayClick),
            _ => None,
        }
    }

    /// Build a momentary press (NoteOn 127 then NoteOff) on the MCU channel.
    pub fn press(&self) -> [MidiMessage; 2] {
        button_press(CHANNEL, self.note())
    }

    /// Drive the button LED: NoteOn lights it, NoteOff clears it.
    pub fn set_led(&self, on: bool) -> MidiMessage {
        if on {
            MidiMessage::NoteOn {
                channel: CHANNEL,
                note: self.note(),
                velocity: 127,
            }
        } else {
            MidiMessage::NoteOff {
                channel: CHANNEL,
                note: self.note(),
                velocity: 0,
            }
        }
    }

    /// Decode a channel-0 note message into `(button, pressed)`.
    /// NoteOn velocity 0 counts as released, matching the MIDI convention.
    pub fn decode(message: &MidiMessage) -> Option<(Self, bool)> {
        match message {
            MidiMessage::NoteOn {
                channel,
                note,
                velocity,
            } if *channel == CHANNEL => Some((Self::from_note(*note)?, *velocity != 0)),
            MidiMessage::NoteOff { channel, note, .. } if *channel == CHANNEL => {
                Some((Self::from_note(*note)?, false))
            }
            _ => None,
        }
    }
}

/// Build a momentary button press on any channel (NoteOn 127 + NoteOff).
/// Hardware profiles with Mackie-flavored notes on other channels route
/// through here; pure MCU traffic uses channel 0.
pub fn button_press(channel: u8, note: u8) -> [MidiMessage; 2] {
    [
        MidiMessage::NoteOn {
            channel,
            note,
            velocity: 127,
        },
        MidiMessage::NoteOff {
            channel,
            note,
            velocity: 0,
        },
    ]
}

/// Drive a fader: Pitch Bend on the strip channel (0-7 strips, 8 master),
/// full 14-bit range, 0 down to 16383 fully up.
pub fn fader_position(strip: u8, value: u16) -> Result<MidiMessage> {
    if strip > 8 {
        return Err(MidiError::InvalidMessage(format!(
            "MCU fader strip out of range: {strip}"
        )));
    }
    Ok(MidiMessage::PitchBend {
        channel: strip,
        value: value & 0x3FFF,
    })
}

/// Decode a fader position update into `(strip, value)`.
pub fn decode_fader(message: &MidiMessage) -> Option<(u8, u16)> {
    match message {
        MidiMessage::PitchBend { channel, value } if *channel <= 8 => {
            Some((*channel, *value & 0x3FFF))
        }
        _ => None,
    }
}

/// Decode a VPot rotation (CC 16-23) into `(strip, delta)`.
/// Encoding is sign-magnitude: bit 6 is the CCW sign, the low 6 bits the
/// click count (+1 CW .. -1 CCW nominal; wider magnitudes likely encode
/// acceleration but are untested on hardware).
pub fn decode_vpot(message: &MidiMessage) -> Option<(u8, i8)> {
    match message {
        MidiMessage::ControlChange {
            channel,
            controller,
            value,
        } if *channel == CHANNEL && (VPOT_CC_START..VPOT_CC_START + 8).contains(controller) => {
            let magnitude = (value & 0x3F) as i8;
            let delta = if value & 0x40 != 0 { -magnitude } else { magnitude };
            Some((controller - VPOT_CC_START, delta))
        }
        _ => None,
    }
}

/// Decode jog/scrub wheel motion (CC 60) into a signed delta, using the
/// same sign-magnitude encoding as VPot rotation.
pub fn decode_jog(message: &MidiMessage) -> Option<i8> {
    match message {
        MidiMessage::ControlChange {
            channel,
            controller,
            value,
        } if *channel == CHANNEL && *controller == JOG_CC => {
            let magnitude = (value & 0x3F) as i8;
            Some(if value & 0x40 != 0 { -magnitude } else { magnitude })
        }
        _ => None,
    }
}

/// Drive a VPot LED ring (CC 48-55): `0LMMVVVV` — center LED flag, mode,
/// and position 0-11 (positions above 11 saturate, matching hardware).
pub fn vpot_ring(strip: u8, mode: MackieRingMode, position: u8, center_led: bool) -> Result<MidiMessage> {
    if strip > 7 {
        return Err(MidiError::InvalidMessage(format!(
            "MCU VPot strip out of range: {strip}"
        )));
    }
    let value = ((center_led as u8) << 6) | ((mode as u8) << 4) | position.min(11);
    Ok(MidiMessage::ControlChange {
        channel: CHANNEL,
        controller: RING_CC_START + strip,
        value,
    })
}

/// Decode a VPot LED-ring update into `(strip, mode, position, center_led)`.
pub fn decode_ring(message: &MidiMessage) -> Option<(u8, MackieRingMode, u8, bool)> {
    match message {
        MidiMessage::ControlChange {
            channel,
            controller,
            value,
        } if *channel == CHANNEL && (RING_CC_START..RING_CC_START + 8).contains(controller) => {
            Some((
                controller - RING_CC_START,
                MackieRingMode::from_bits(value >> 4),
                value & 0x0F,
                value & 0x40 != 0,
            ))
        }
        _ => None,
    }
}

/// Write LCD text at `offset` (0x00-0x37 top row, 0x38-0x6F bottom row).
/// Non-7-bit characters become spaces; payloads over 112 bytes continue in
/// follow-up messages with advancing offsets.
pub fn lcd_write(offset: u8, text: &str) -> Result<Vec<MidiMessage>> {
    if offset > 0x6F {
        return Err(MidiError::InvalidMessage(format!(
            "MCU LCD offset out of range: {offset:#04X}"
        )));
    }
    let clean: Vec<u8> = text
        .bytes()
        .map(|byte| {
            if (0x20..0x80).contains(&byte) {
                byte
            } else {
                b' '
            }
        })
        .collect();
    let mut messages = Vec::new();
    let chunks: Vec<&[u8]> = if clean.is_empty() {
        vec![&[]]
    } else {
        clean.chunks(LCD_MAX_BYTES).collect()
    };
    for (index, chunk) in chunks.iter().enumerate() {
        let chunk_offset = offset.saturating_add((index * LCD_MAX_BYTES) as u8);
        let mut bytes = Vec::with_capacity(7 + chunk.len());
        bytes.extend_from_slice(&[0xF0]);
        bytes.extend_from_slice(&SYSEX_HEADER);
        bytes.extend_from_slice(&[0x12, chunk_offset]);
        bytes.extend_from_slice(chunk);
        bytes.push(0xF7);
        messages.push(MidiMessage::SystemExclusive { data: bytes });
    }
    Ok(messages)
}

/// Blank both LCD rows (two 56-space writes).
pub fn lcd_clear() -> Vec<MidiMessage> {
    let mut messages = lcd_write(0x00, &" ".repeat(LCD_ROW_CHARS)).unwrap_or_default();
    messages.extend(lcd_write(0x38, &" ".repeat(LCD_ROW_CHARS)).unwrap_or_default());
    messages
}

/// Encode one time/assignment display digit per the 6-bit special-ASCII
/// table (letters map to 1-26, space to 0, other 7-bit codes pass through
/// below 64); bit 6 adds the decimal point. Returns `None` for characters
/// with no mapping.
pub fn time_digit_code(point: bool, digit: char) -> Option<u8> {
    let code = match digit {
        'A'..='Z' => digit as u8 - b'A' + 1,
        ' ' => 0,
        c if (0x20..0x40).contains(&(c as u8)) && c.is_ascii() => c as u8,
        _ => {
            let upper = digit.to_ascii_uppercase();
            match upper {
                'A'..='Z' => upper as u8 - b'A' + 1,
                _ => return None,
            }
        }
    };
    Some(code | if point { 0x40 } else { 0 })
}

/// Drive the 10-digit time display (CC 64-73, right-to-left): the text is
/// right-aligned into 10 cells; each cell is one `(code, dot)` pair.
/// Dots attach with a `.` suffix per cell, e.g. `"12.34"`.
pub fn time_display(text: &str) -> Vec<MidiMessage> {
    const CELLS: usize = 10;
    let mut cells: Vec<(char, bool)> = Vec::new();
    for char in text.chars() {
        if char == '.' {
            if let Some(last) = cells.last_mut() {
                last.1 = true;
            }
            continue;
        }
        cells.push((char, false));
    }
    while cells.len() < CELLS {
        cells.insert(0, (' ', false));
    }
    cells[cells.len() - CELLS..]
        .iter()
        .enumerate()
        .filter_map(|(index, (char, point))| {
            time_digit_code(*point, *char).map(|code| MidiMessage::ControlChange {
                channel: CHANNEL,
                controller: TIME_CC_START + index as u8,
                value: code,
            })
        })
        .collect()
}

/// Drive the 2-digit assignment display (CC 74-75, right-to-left).
pub fn assignment_display(text: &str) -> Vec<MidiMessage> {
    const CELLS: usize = 2;
    let mut cells: Vec<char> = text.chars().collect();
    while cells.len() < CELLS {
        cells.insert(0, ' ');
    }
    cells[cells.len() - CELLS..]
        .iter()
        .enumerate()
        .filter_map(|(index, char)| {
            time_digit_code(false, *char).map(|code| MidiMessage::ControlChange {
                channel: CHANNEL,
                controller: ASSIGN_CC_START + index as u8,
                value: code,
            })
        })
        .collect()
}

/// Drive a channel meter: Channel Pressure on channel 0 with value
/// `(strip << 4) | level`.
pub fn meter(strip: u8, level: MackieMeterLevel) -> Result<MidiMessage> {
    if strip > 7 {
        return Err(MidiError::InvalidMessage(format!(
            "MCU meter strip out of range: {strip}"
        )));
    }
    Ok(MidiMessage::ChannelAftertouch {
        channel: CHANNEL,
        pressure: (strip << 4) | level as u8,
    })
}

/// Decode a channel meter update into `(strip, level)`.
pub fn decode_meter(message: &MidiMessage) -> Option<(u8, MackieMeterLevel)> {
    match message {
        MidiMessage::ChannelAftertouch { channel, pressure } if *channel == CHANNEL => {
            Some((pressure >> 4, MackieMeterLevel::from_nibble(*pressure)?))
        }
        _ => None,
    }
}

/// Build a host-to-device SysEx command frame (`F0 00 00 66 14 <cmd> ... F7`).
fn host_command(command: u8, params: &[u8]) -> MidiMessage {
    let mut bytes = Vec::with_capacity(6 + params.len());
    bytes.push(0xF0);
    bytes.extend_from_slice(&SYSEX_HEADER);
    bytes.push(command);
    bytes.extend_from_slice(params);
    bytes.push(0xF7);
    MidiMessage::SystemExclusive { data: bytes }
}

/// Device query handshake starter (command 0x00).
pub fn device_query() -> MidiMessage {
    host_command(0x00, &[])
}

/// Connection confirmation with the 7-byte ASCII serial (command 0x03).
pub fn host_connection_confirm(serial: &[u8; 7]) -> MidiMessage {
    host_command(0x03, serial)
}

/// Raw connection reply carrying a precomputed 4-byte response code
/// (command 0x02). The challenge-response algorithm is proprietary, so the
/// caller supplies the response bytes.
pub fn host_connection_reply(serial: &[u8; 7], response: &[u8; 4]) -> MidiMessage {
    let mut params = Vec::with_capacity(11);
    params.extend_from_slice(serial);
    params.extend_from_slice(response);
    host_command(0x02, &params)
}

/// Firmware version request (command 0x13).
pub fn version_request() -> MidiMessage {
    host_command(0x13, &[0x00])
}

/// Transport button click feedback (command 0x0A).
pub fn transport_click(on: bool) -> MidiMessage {
    host_command(0x0A, &[u8::from(on)])
}

/// LCD backlight saver (command 0x0B): `0` switches off, otherwise the
/// timeout in minutes (default 15).
pub fn backlight(minutes: u8) -> Result<MidiMessage> {
    if minutes > 127 {
        return Err(MidiError::InvalidMessage(format!(
            "MCU backlight timeout out of range: {minutes}"
        )));
    }
    Ok(host_command(0x0B, &[minutes]))
}

/// Touchless fader mode (command 0x0C): when true, fader motion transmits
/// even without a recognized touch.
pub fn touchless_faders(on: bool) -> MidiMessage {
    host_command(0x0C, &[u8::from(on)])
}

/// Fader touch sensitivity per strip 0-8 (command 0x0E), levels 0-5.
pub fn touch_sensitivity(strip: u8, level: u8) -> Result<MidiMessage> {
    if strip > 8 {
        return Err(MidiError::InvalidMessage(format!(
            "MCU touch strip out of range: {strip}"
        )));
    }
    if level > 5 {
        return Err(MidiError::InvalidMessage(format!(
            "MCU touch sensitivity out of range: {level}"
        )));
    }
    Ok(host_command(0x0E, &[strip, level]))
}

/// Per-channel LCD meter mode (command 0x20): level meter, peak hold, and
/// signal LED flags for strip 0-7.
pub fn channel_meter_mode(
    strip: u8,
    level_meter: bool,
    peak_hold: bool,
    signal_led: bool,
) -> Result<MidiMessage> {
    if strip > 7 {
        return Err(MidiError::InvalidMessage(format!(
            "MCU meter strip out of range: {strip}"
        )));
    }
    let mode = (u8::from(level_meter) << 2) | (u8::from(peak_hold) << 1) | u8::from(signal_led);
    Ok(host_command(0x20, &[strip, mode]))
}

/// Global LCD meter orientation (command 0x21).
pub fn global_meter_mode(vertical: bool) -> MidiMessage {
    host_command(0x21, &[u8::from(vertical)])
}

/// Switch all button LEDs off (command 0x62).
pub fn all_leds_off() -> MidiMessage {
    host_command(0x62, &[])
}

/// Reset the surface (command 0x63).
pub fn reset_surface() -> MidiMessage {
    host_command(0x63, &[])
}

/// Drive all faders to minimum (command 0x61).
pub fn faders_to_minimum() -> MidiMessage {
    host_command(0x61, &[])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_notes_cover_the_hardware_map() {
        for strip in 0..8u8 {
            assert_eq!(MackieButton::Rec(strip).note(), strip);
            assert_eq!(MackieButton::Solo(strip).note(), 8 + strip);
            assert_eq!(MackieButton::Mute(strip).note(), 16 + strip);
            assert_eq!(MackieButton::Select(strip).note(), 24 + strip);
            assert_eq!(MackieButton::VPotSwitch(strip).note(), 32 + strip);
            assert_eq!(MackieButton::FaderTouch(strip).note(), 104 + strip);
            // Round-trip through the decoder.
            for button in [
                MackieButton::Rec(strip),
                MackieButton::Solo(strip),
                MackieButton::Mute(strip),
                MackieButton::Select(strip),
                MackieButton::VPotSwitch(strip),
                MackieButton::FaderTouch(strip),
            ] {
                assert_eq!(MackieButton::from_note(button.note()), Some(button));
            }
        }
        assert_eq!(
            MackieButton::from_note(54),
            Some(MackieButton::Function(1))
        );
        assert_eq!(
            MackieButton::from_note(61),
            Some(MackieButton::Function(8))
        );
        assert_eq!(
            MackieButton::from_note(94),
            Some(MackieButton::Transport(MackieTransport::Play))
        );
        assert_eq!(MackieButton::from_note(70), Some(MackieButton::Shift));
        assert_eq!(MackieButton::from_note(112), Some(MackieButton::MasterTouch));
        assert_eq!(MackieButton::from_note(115), Some(MackieButton::RudeSoloLed));
        assert_eq!(MackieButton::from_note(118), Some(MackieButton::RelayClick));
        assert_eq!(MackieButton::from_note(116), None);
        assert_eq!(MackieButton::from_note(119), None);
        assert_eq!(MackieButton::from_note(200), None);
    }

    #[test]
    fn button_press_and_led_round_trip() {
        let button = MackieButton::Mute(3);
        let [press, release] = button.press();
        assert_eq!(
            press,
            MidiMessage::NoteOn {
                channel: 0,
                note: 19,
                velocity: 127
            }
        );
        assert_eq!(MackieButton::decode(&press), Some((button, true)));
        assert_eq!(MackieButton::decode(&release), Some((button, false)));
        assert_eq!(
            button.set_led(true),
            MidiMessage::NoteOn {
                channel: 0,
                note: 19,
                velocity: 127
            }
        );
        // Velocity-0 NoteOn counts as release; other channels are ignored.
        assert_eq!(
            MackieButton::decode(&MidiMessage::NoteOn {
                channel: 0,
                note: 19,
                velocity: 0
            }),
            Some((button, false))
        );
        assert_eq!(
            MackieButton::decode(&MidiMessage::NoteOn {
                channel: 5,
                note: 19,
                velocity: 127
            }),
            None
        );
    }

    #[test]
    fn fader_position_round_trips_full_range() {
        for (strip, value) in [(0u8, 0u16), (3, 8192), (8, 16383)] {
            let message = fader_position(strip, value).unwrap();
            assert_eq!(decode_fader(&message), Some((strip, value)));
            assert_eq!(
                message.to_bytes(),
                vec![0xE0 | strip, (value & 0x7F) as u8, ((value >> 7) & 0x7F) as u8]
            );
        }
        assert!(fader_position(9, 0).is_err());
    }

    #[test]
    fn vpot_and_jog_deltas_decode() {
        let vpot = |value| {
            decode_vpot(&MidiMessage::ControlChange {
                channel: 0,
                controller: 19,
                value,
            })
        };
        assert_eq!(vpot(0x01), Some((3, 1)));
        assert_eq!(vpot(0x41), Some((3, -1)));
        assert_eq!(vpot(0x05), Some((3, 5)));
        assert_eq!(vpot(0x45), Some((3, -5)));
        assert_eq!(
            decode_jog(&MidiMessage::ControlChange {
                channel: 0,
                controller: 60,
                value: 0x01,
            }),
            Some(1)
        );
        assert_eq!(
            decode_jog(&MidiMessage::ControlChange {
                channel: 0,
                controller: 60,
                value: 0x41,
            }),
            Some(-1)
        );
        assert_eq!(
            decode_jog(&MidiMessage::ControlChange {
                channel: 0,
                controller: 61,
                value: 0x01,
            }),
            None
        );
    }

    #[test]
    fn vpot_ring_encodes_mode_position_and_center() {
        let message = vpot_ring(2, MackieRingMode::Spread, 6, true).unwrap();
        assert_eq!(
            message,
            MidiMessage::ControlChange {
                channel: 0,
                controller: 50,
                value: 0x40 | 0x30 | 0x06,
            }
        );
        assert_eq!(
            decode_ring(&message),
            Some((2, MackieRingMode::Spread, 6, true))
        );
        assert!(vpot_ring(8, MackieRingMode::Wrap, 0, false).is_err());
    }

    #[test]
    fn lcd_write_frames_offsets_and_chunks() {
        let messages = lcd_write(0x00, "hello").unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0].to_bytes(),
            vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x12, 0x00, b'h', b'e', b'l', b'l', b'o', 0xF7]
        );
        let long = "x".repeat(120);
        let messages = lcd_write(0x00, &long).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].to_bytes().len(), 8 + 112);
        assert_eq!(messages[1].to_bytes()[6], 112);
        assert!(lcd_write(0x70, "x").is_err());
        assert_eq!(lcd_clear().len(), 2);
    }

    #[test]
    fn time_and_assignment_displays_encode() {
        let messages = time_display("12.34");
        assert_eq!(messages.len(), 10);
        // Right-aligned: CC 73 carries '4', CC 72 carries '3' with the point.
        assert_eq!(
            messages[9],
            MidiMessage::ControlChange {
                channel: 0,
                controller: 73,
                value: b'4',
            }
        );
        assert_eq!(
            messages[7],
            MidiMessage::ControlChange {
                channel: 0,
                controller: 71,
                value: 0x40 | b'2',
            }
        );
        let assign = assignment_display("Pn");
        assert_eq!(assign.len(), 2);
        assert_eq!(
            assign[1],
            MidiMessage::ControlChange {
                channel: 0,
                controller: 75,
                value: b'N' - b'A' + 1,
            }
        );
    }

    #[test]
    fn meters_round_trip() {
        let message = meter(7, MackieMeterLevel::Clip).unwrap();
        assert_eq!(
            message.to_bytes(),
            vec![0xD0, (7 << 4) | 0x0C]
        );
        assert_eq!(
            decode_meter(&message),
            Some((7, MackieMeterLevel::Clip))
        );
        assert!(meter(8, MackieMeterLevel::Off).is_err());
        assert_eq!(MackieMeterLevel::from_nibble(0xF), Some(MackieMeterLevel::ClearOverload));
    }

    #[test]
    fn host_commands_frame_correctly() {
        assert_eq!(
            device_query().to_bytes(),
            vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x00, 0xF7]
        );
        assert_eq!(
            host_connection_confirm(b"ABC1234").to_bytes(),
            vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x03, b'A', b'B', b'C', b'1', b'2', b'3', b'4', 0xF7]
        );
        assert_eq!(
            backlight(15).unwrap().to_bytes(),
            vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x0B, 0x0F, 0xF7]
        );
        assert!(backlight(128).is_err());
        assert_eq!(
            touch_sensitivity(8, 3).unwrap().to_bytes(),
            vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x0E, 0x08, 0x03, 0xF7]
        );
        assert_eq!(
            channel_meter_mode(0, true, false, true).unwrap().to_bytes(),
            vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x20, 0x00, 0x05, 0xF7]
        );
    }
}
