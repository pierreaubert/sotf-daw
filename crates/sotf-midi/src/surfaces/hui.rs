//! Mackie HUI protocol: first-class surface support.
//!
//! Wire map verified against the reverse-engineered HUI protocol document
//! (device firmware 1.45, cross-checked with an independent HUI/MCU MIDI
//! logger):
//! - Header: `F0 00 00 66 05 00 ... F7` for all HUI SysEx.
//! - Ping: NoteOn ch 0 note 0 velocity 0; reply velocity `0x7F`.
//! - Scribble strips: command `0x10`, channel 0-7 (8 = SELECT ASSIGN),
//!   4 characters each.
//! - Main display: command `0x12`, up to four 10-character zones (0-7).
//! - Timecode display: command `0x11`, 8 digits LSB-first, bit 4 (`0x10`)
//!   adds the decimal point.
//! - VU meters: Channel Pressure ch 0, data `(channel, side << 4 | level)`.
//! - VPot rings: CC `0x10`-`0x1B` (`B0 1y vv`), channel/param 0-11.
//! - LEDs and switches: zone select CC `0x0C` then port CC `0x2C`
//!   (`0x40`-flagged port byte = on); inbound uses zone select `0x0F`
//!   plus port `0x2F` in the same framing.
//! - Faders: CC pairs `B0 0z <hi>` + `B0 2z <lo>` (10-bit effective);
//!   touch/release reuse zone select with zone `0z` and port `0x40`/`0x00`.
//! - VPot rotation: `B0 4p vv` (params 0-12); jog: `B0 0D vv`.
//!   Deltas are inverted relative to MCU: below `0x40` counts down,
//!   above counts up (`delta = vv - 0x40`).
//! - Footswitches: zone `0x1D`, ports 0-1; relays/click/beep share the
//!   zone/port LED mechanism on the same zone.
//!
//! Button identities (which zone/port is PLAY, SELECT, ...) were never
//! published and vary by surface, so switches travel as typed
//! [`HuiSwitch`] zone/port pairs rather than named buttons. A stateful
//! [`HuiSwitchStream`] pairs zone-select and port messages; a
//! [`HuiFaderStream`] pairs fader MSB/LSB messages.

use crate::error::{MidiError, Result};
use crate::message::MidiMessage;
use serde::{Deserialize, Serialize};

/// HUI SysEx header: Mackie id, HUI device id, command-class byte.
pub const SYSEX_HEADER: [u8; 5] = [0x00, 0x00, 0x66, 0x05, 0x00];
/// Scribble-strip SysEx command (4 characters per strip).
pub const CMD_SCRIBBLE: u8 = 0x10;
/// Timecode-display SysEx command (8 digits, LSB first).
pub const CMD_TIMECODE: u8 = 0x11;
/// Main-display SysEx command (up to four 10-character zones).
pub const CMD_MAIN_DISPLAY: u8 = 0x12;
/// Scribble strip index addressing the SELECT ASSIGN readout.
pub const SELECT_ASSIGN_STRIP: u8 = 8;
/// Maximum zone number for LEDs, switches, and display zones.
pub const MAX_ZONE: u8 = 0x1D;
/// Zone carrying footswitches (ports 0-1) and relays/click/beep (ports 0-3).
pub const ZONE_FOOTSWITCH: u8 = 0x1D;
/// First VPot-ring CC (`B0 1y vv`); params 0-11 map to CC 0x10-0x1B.
pub const RING_CC_START: u8 = 0x10;
/// VPot rotation CC base (`B0 4p vv`); params 0-12.
pub const VPOT_CC_START: u8 = 0x40;
/// Jog-wheel CC.
pub const JOG_CC: u8 = 0x0D;
/// LED/switch zone-select CC (host direction) and port CC.
pub const LED_ZONE_CC: u8 = 0x0C;
/// LED/switch port CC (host direction).
pub const LED_PORT_CC: u8 = 0x2C;
/// Inbound switch zone-select CC and port CC.
pub const SWITCH_ZONE_CC: u8 = 0x0F;
/// Inbound switch port CC.
pub const SWITCH_PORT_CC: u8 = 0x2F;

/// Ping request: NoteOn ch 0 note 0 velocity 0. The surface replies with
/// velocity `0x7F` (`ping_reply`); both sides repeat it to stay connected.
pub fn ping() -> MidiMessage {
    MidiMessage::NoteOn {
        channel: 0,
        note: 0,
        velocity: 0,
    }
}

/// Ping reply the host sends back: NoteOn ch 0 note 0 velocity `0x7F`.
pub fn ping_reply() -> MidiMessage {
    MidiMessage::NoteOn {
        channel: 0,
        note: 0,
        velocity: 0x7F,
    }
}

/// Whether a message is a HUI ping in either direction.
pub fn is_ping(message: &MidiMessage) -> bool {
    matches!(
        message,
        MidiMessage::NoteOn {
            channel: 0,
            note: 0,
            ..
        }
    )
}

/// Write a 4-character scribble strip (strips 0-7, or
/// [`SELECT_ASSIGN_STRIP`] for the assign readout). Characters outside the
/// printable 7-bit range become spaces.
pub fn scribble(strip: u8, text: &str) -> Result<MidiMessage> {
    if strip > SELECT_ASSIGN_STRIP {
        return Err(MidiError::InvalidMessage(format!(
            "HUI scribble strip out of range: {strip}"
        )));
    }
    let mut chars: Vec<u8> = text
        .bytes()
        .map(|byte| {
            if (0x20..0x80).contains(&byte) {
                byte
            } else {
                b' '
            }
        })
        .take(4)
        .collect();
    while chars.len() < 4 {
        chars.push(b' ');
    }
    let mut bytes = Vec::with_capacity(12);
    bytes.push(0xF0);
    bytes.extend_from_slice(&SYSEX_HEADER);
    bytes.extend_from_slice(&[CMD_SCRIBBLE, strip]);
    bytes.extend_from_slice(&chars);
    bytes.push(0xF7);
    Ok(MidiMessage::SystemExclusive { data: bytes })
}

/// Write main-display zones: each `(zone, text)` pair contributes 10
/// characters (space-padded, non-printable bytes become spaces). At least
/// one and at most four zones; zone numbers run 0-7.
pub fn main_display(zones: &[(u8, &str)]) -> Result<MidiMessage> {
    if zones.is_empty() || zones.len() > 4 {
        return Err(MidiError::InvalidMessage(format!(
            "HUI main display needs 1-4 zones, got {}",
            zones.len()
        )));
    }
    let mut bytes = Vec::with_capacity(8 + zones.len() * 11);
    bytes.push(0xF0);
    bytes.extend_from_slice(&SYSEX_HEADER);
    bytes.push(CMD_MAIN_DISPLAY);
    for (zone, text) in zones {
        if *zone > 7 {
            return Err(MidiError::InvalidMessage(format!(
                "HUI display zone out of range: {zone}"
            )));
        }
        let mut chars: Vec<u8> = text
            .bytes()
            .map(|byte| {
                if (0x10..0x80).contains(&byte) {
                    byte
                } else {
                    b' '
                }
            })
            .take(10)
            .collect();
        while chars.len() < 10 {
            chars.push(b' ');
        }
        bytes.push(*zone);
        bytes.extend_from_slice(&chars);
    }
    bytes.push(0xF7);
    Ok(MidiMessage::SystemExclusive { data: bytes })
}

/// Write the 8-digit timecode display, digits LSB first. Each digit holds
/// 0-15, or 0x10-0x1F with the decimal point (`0x10` flag). Fewer than 8
/// digits blank-fill from the most significant end.
pub fn timecode(digits: &[u8]) -> Result<MidiMessage> {
    if digits.len() > 8 {
        return Err(MidiError::InvalidMessage(format!(
            "HUI timecode takes at most 8 digits, got {}",
            digits.len()
        )));
    }
    if digits.iter().any(|digit| *digit > 0x1F) {
        return Err(MidiError::InvalidMessage(
            "HUI timecode digits out of range".to_string(),
        ));
    }
    let mut bytes = Vec::with_capacity(15);
    bytes.push(0xF0);
    bytes.extend_from_slice(&SYSEX_HEADER);
    bytes.push(CMD_TIMECODE);
    bytes.extend_from_slice(digits);
    while bytes.len() < 15 {
        bytes.push(0x00);
    }
    bytes.push(0xF7);
    Ok(MidiMessage::SystemExclusive { data: bytes })
}

/// Meter side for VU displays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HuiMeterSide {
    /// Left meter (0x1_ value prefix).
    Left,
    /// Right meter (0x0_ value prefix).
    Right,
}

/// Drive a VU meter: Polyphonic Aftertouch on channel 0 with the note
/// byte carrying the channel index and pressure `(side << 4) | level`
/// (`level` 0-12, where 12 is clip). The two-data-byte shape is why this
/// is polyphonic rather than channel pressure on the wire.
pub fn meter(channel: u8, side: HuiMeterSide, level: u8) -> Result<MidiMessage> {
    if channel > 7 {
        return Err(MidiError::InvalidMessage(format!(
            "HUI meter channel out of range: {channel}"
        )));
    }
    if level > 0x0C {
        return Err(MidiError::InvalidMessage(format!(
            "HUI meter level out of range: {level}"
        )));
    }
    let prefix = match side {
        HuiMeterSide::Left => 0x10,
        HuiMeterSide::Right => 0x00,
    };
    Ok(MidiMessage::PolyphonicAftertouch {
        channel: 0,
        note: channel,
        pressure: prefix | level,
    })
}

/// Decode a VU meter update into `(channel, side, level)`.
pub fn decode_meter(message: &MidiMessage) -> Option<(u8, HuiMeterSide, u8)> {
    match message {
        MidiMessage::PolyphonicAftertouch {
            channel: 0,
            note,
            pressure,
        } if *note <= 7 => {
            let level = pressure & 0x0F;
            if level > 0x0C {
                return None;
            }
            let side = if pressure & 0x10 != 0 {
                HuiMeterSide::Left
            } else {
                HuiMeterSide::Right
            };
            Some((*note, side, level))
        }
        _ => None,
    }
}

/// Drive a VPot ring (`B0 1y vv`), channel/param 0-11. Values follow the
/// documented rows (modes at `0x00`/`0x10`/`0x20`/`0x30` plus position,
/// `0x40` lights the small LED); anything above `0x7B` is rejected.
pub fn vpot_ring(param: u8, value: u8) -> Result<MidiMessage> {
    if param > 0x0B {
        return Err(MidiError::InvalidMessage(format!(
            "HUI VPot ring param out of range: {param:#04X}"
        )));
    }
    if value > 0x7B {
        return Err(MidiError::InvalidMessage(format!(
            "HUI VPot ring value out of range: {value:#04X}"
        )));
    }
    Ok(MidiMessage::ControlChange {
        channel: 0,
        controller: RING_CC_START + param,
        value,
    })
}

/// Select an LED zone, then set a port: returns the two CC messages
/// (`B0 0C zz`, `B0 2C (0x40|port)` or `(0x00|port)`). Zone `0x1D` ports
/// 0-3 drive relays/click/beep.
pub fn led_set(zone: u8, port: u8, on: bool) -> Result<[MidiMessage; 2]> {
    if zone > MAX_ZONE {
        return Err(MidiError::InvalidMessage(format!(
            "HUI LED zone out of range: {zone:#04X}"
        )));
    }
    if port > 7 {
        return Err(MidiError::InvalidMessage(format!(
            "HUI LED port out of range: {port}"
        )));
    }
    let port_byte = if on { 0x40 | port } else { port };
    Ok([
        MidiMessage::ControlChange {
            channel: 0,
            controller: LED_ZONE_CC,
            value: zone,
        },
        MidiMessage::ControlChange {
            channel: 0,
            controller: LED_PORT_CC,
            value: port_byte,
        },
    ])
}

/// Drive a fader: CC pair `B0 0z <hi>` + `B0 2z <lo>` carrying a 14-bit
/// value (10-bit effective resolution on hardware).
pub fn fader_position(zone: u8, value: u16) -> Result<[MidiMessage; 2]> {
    if zone > 7 {
        return Err(MidiError::InvalidMessage(format!(
            "HUI fader zone out of range: {zone}"
        )));
    }
    let value = value & 0x3FFF;
    Ok([
        MidiMessage::ControlChange {
            channel: 0,
            controller: zone,
            value: ((value >> 7) & 0x7F) as u8,
        },
        MidiMessage::ControlChange {
            channel: 0,
            controller: 0x20 | zone,
            value: (value & 0x7F) as u8,
        },
    ])
}

/// Fader touch/release: zone select (`B0 0F 0z`) plus port
/// (`B0 2F 0x40` touched, `B0 2F 0x00` released).
pub fn fader_touch(zone: u8, touched: bool) -> Result<[MidiMessage; 2]> {
    if zone > 7 {
        return Err(MidiError::InvalidMessage(format!(
            "HUI fader zone out of range: {zone}"
        )));
    }
    Ok([
        MidiMessage::ControlChange {
            channel: 0,
            controller: SWITCH_ZONE_CC,
            value: zone,
        },
        MidiMessage::ControlChange {
            channel: 0,
            controller: SWITCH_PORT_CC,
            value: if touched { 0x40 } else { 0x00 },
        },
    ])
}

/// A switch press/release identified by zone and port numbers. Button
/// identities were never published, so switches stay numeric; zone
/// `0x1D` ports 0-1 are the footswitches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HuiSwitch {
    /// Switch zone (0x00-0x1D).
    pub zone: u8,
    /// Switch port within the zone (0-7).
    pub port: u8,
    /// True for press (port byte `0x40`-flagged), false for release.
    pub pressed: bool,
}

/// Pairs inbound zone-select (`B0 0F zz`) and port (`B0 2F pp`) messages
/// into [`HuiSwitch`] values. A port message without a preceding zone
/// select is dropped; each port message consumes the stored zone.
#[derive(Debug, Clone, Default)]
pub struct HuiSwitchStream {
    pending_zone: Option<u8>,
}

impl HuiSwitchStream {
    /// Create an empty stream.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one message; returns a completed switch when a port message
    /// arrives with a stored zone.
    pub fn push(&mut self, message: &MidiMessage) -> Option<HuiSwitch> {
        match message {
            MidiMessage::ControlChange {
                channel: 0,
                controller: SWITCH_ZONE_CC,
                value,
            } if *value <= MAX_ZONE => {
                self.pending_zone = Some(*value);
                None
            }
            MidiMessage::ControlChange {
                channel: 0,
                controller: SWITCH_PORT_CC,
                value,
            } => {
                let zone = self.pending_zone.take()?;
                if value & 0xB8 != 0 {
                    return None;
                }
                Some(HuiSwitch {
                    zone,
                    port: value & 0x07,
                    pressed: value & 0x40 != 0,
                })
            }
            _ => None,
        }
    }
}

/// Pairs inbound fader MSB (`B0 0z`) / LSB (`B0 2z`) messages into
/// `(zone, value)` pairs. Either half may arrive first; each completion
/// consumes the stored half.
#[derive(Debug, Clone, Default)]
pub struct HuiFaderStream {
    pending_hi: [Option<u8>; 8],
    pending_lo: [Option<u8>; 8],
}

impl HuiFaderStream {
    /// Create an empty stream.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one message; returns `(zone, 14-bit value)` when both halves
    /// of a fader position are known.
    pub fn push(&mut self, message: &MidiMessage) -> Option<(u8, u16)> {
        let (zone, half) = match message {
            MidiMessage::ControlChange {
                channel: 0,
                controller,
                value,
            } if *controller < 8 => (*controller, Half::Hi(*value)),
            MidiMessage::ControlChange {
                channel: 0,
                controller,
                value,
            } if (0x20..0x28).contains(controller) => (controller - 0x20, Half::Lo(*value)),
            _ => return None,
        };
        let zone = zone as usize;
        match half {
            Half::Hi(hi) => {
                self.pending_hi[zone] = Some(hi);
                self.pending_lo[zone]
                    .take()
                    .map(|lo| (zone as u8, (((hi as u16) << 7) | lo as u16) & 0x3FFF))
            }
            Half::Lo(lo) => {
                self.pending_lo[zone] = Some(lo);
                self.pending_hi[zone]
                    .take()
                    .map(|hi| (zone as u8, (((hi as u16) << 7) | lo as u16) & 0x3FFF))
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Half {
    Hi(u8),
    Lo(u8),
}

/// Decode VPot rotation (`B0 4p vv`, params 0-12) into `(param, delta)`.
/// Encoding is inverted against MCU: values below `0x40` count down
/// (`-vv`), values above count up (`vv - 0x40`); `0x40` never appears.
pub fn decode_vpot(message: &MidiMessage) -> Option<(u8, i8)> {
    match message {
        MidiMessage::ControlChange {
            channel: 0,
            controller,
            value,
        } if (VPOT_CC_START..VPOT_CC_START + 13).contains(controller) => {
            let delta = if *value > 0x40 {
                (value - 0x40) as i8
            } else if *value < 0x40 {
                -(*value as i8)
            } else {
                return None;
            };
            Some((controller - VPOT_CC_START, delta))
        }
        _ => None,
    }
}

/// Decode jog-wheel motion (`B0 0D vv`) with the same inverted delta
/// encoding as VPot rotation.
pub fn decode_jog(message: &MidiMessage) -> Option<i8> {
    match message {
        MidiMessage::ControlChange {
            channel: 0,
            controller: JOG_CC,
            value,
        } => {
            if *value > 0x40 {
                Some((value - 0x40) as i8)
            } else if *value < 0x40 {
                Some(-(*value as i8))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Whether a message is the HUI reset command (MIDI status `0xFF`).
pub fn is_system_reset(message: &MidiMessage) -> bool {
    matches!(message, MidiMessage::System { status: 0xFF, .. })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_round_trip() {
        assert_eq!(ping().to_bytes(), vec![0x90, 0x00, 0x00]);
        assert_eq!(ping_reply().to_bytes(), vec![0x90, 0x00, 0x7F]);
        assert!(is_ping(&ping()));
        assert!(is_ping(&ping_reply()));
        assert!(!is_ping(&MidiMessage::NoteOn {
            channel: 1,
            note: 0,
            velocity: 0
        }));
    }

    #[test]
    fn scribble_frames_four_chars() {
        let message = scribble(3, "Pan").unwrap();
        assert_eq!(
            message.to_bytes(),
            vec![
                0xF0, 0x00, 0x00, 0x66, 0x05, 0x00, 0x10, 0x03, b'P', b'a', b'n', b' ', 0xF7
            ]
        );
        let assign = scribble(SELECT_ASSIGN_STRIP, "A").unwrap();
        assert_eq!(assign.to_bytes()[7], SELECT_ASSIGN_STRIP);
        assert!(scribble(9, "x").is_err());
    }

    #[test]
    fn main_display_frames_zones() {
        let message = main_display(&[(0, "Track 01"), (7, "12:34")]).unwrap();
        let bytes = message.to_bytes();
        assert_eq!(
            &bytes[..8],
            &[0xF0, 0x00, 0x00, 0x66, 0x05, 0x00, 0x12, 0x00]
        );
        assert_eq!(&bytes[8..18], b"Track 01  ");
        assert_eq!(bytes[18], 0x07);
        assert_eq!(&bytes[19..29], b"12:34     ");
        assert_eq!(bytes[bytes.len() - 1], 0xF7);
        assert!(main_display(&[]).is_err());
        assert!(main_display(&[(8, "x")]).is_err());
    }

    #[test]
    fn timecode_is_lsb_first_with_point_flag() {
        let message = timecode(&[0x04, 0x13, 0x02]).unwrap();
        assert_eq!(
            message.to_bytes(),
            vec![
                0xF0, 0x00, 0x00, 0x66, 0x05, 0x00, 0x11, 0x04, 0x13, 0x02, 0x00, 0x00, 0x00, 0x00,
                0x00, 0xF7
            ]
        );
        assert!(timecode(&[0x20]).is_err());
        assert!(timecode(&[0; 9]).is_err());
    }

    #[test]
    fn meters_round_trip() {
        let message = meter(5, HuiMeterSide::Left, 0x0C).unwrap();
        assert_eq!(message.to_bytes(), vec![0xA0, 0x05, 0x1C]);
        assert_eq!(decode_meter(&message), Some((5, HuiMeterSide::Left, 0x0C)));
        let right = meter(0, HuiMeterSide::Right, 3).unwrap();
        assert_eq!(decode_meter(&right), Some((0, HuiMeterSide::Right, 3)));
        assert!(meter(8, HuiMeterSide::Left, 0).is_err());
        assert!(meter(0, HuiMeterSide::Left, 0x0D).is_err());
    }

    #[test]
    fn rings_and_leds_frame_correctly() {
        let ring = vpot_ring(2, 0x06).unwrap();
        assert_eq!(
            ring,
            MidiMessage::ControlChange {
                channel: 0,
                controller: 0x12,
                value: 0x06
            }
        );
        assert!(vpot_ring(0x0C, 0).is_err());
        assert!(vpot_ring(0, 0x7C).is_err());
        let [select, on] = led_set(0x04, 3, true).unwrap();
        assert_eq!(
            select,
            MidiMessage::ControlChange {
                channel: 0,
                controller: 0x0C,
                value: 0x04
            }
        );
        assert_eq!(
            on,
            MidiMessage::ControlChange {
                channel: 0,
                controller: 0x2C,
                value: 0x43
            }
        );
        assert!(led_set(0x1E, 0, true).is_err());
        assert!(led_set(0, 8, true).is_err());
    }

    #[test]
    fn fader_pairs_encode_and_assemble() {
        let [hi, lo] = fader_position(3, 1000).unwrap();
        assert_eq!(
            hi,
            MidiMessage::ControlChange {
                channel: 0,
                controller: 0x03,
                value: 7,
            }
        );
        assert_eq!(
            lo,
            MidiMessage::ControlChange {
                channel: 0,
                controller: 0x23,
                value: 104,
            }
        );
        // Either half may arrive first.
        let mut stream = HuiFaderStream::new();
        assert_eq!(stream.push(&hi), None);
        assert_eq!(stream.push(&lo), Some((3, 1000)));
        let mut stream = HuiFaderStream::new();
        assert_eq!(stream.push(&lo), None);
        assert_eq!(stream.push(&hi), Some((3, 1000)));
        assert!(fader_position(8, 0).is_err());
    }

    #[test]
    fn switch_stream_pairs_zone_and_port() {
        let mut stream = HuiSwitchStream::new();
        // Port without zone is dropped.
        let port = MidiMessage::ControlChange {
            channel: 0,
            controller: SWITCH_PORT_CC,
            value: 0x43,
        };
        assert_eq!(stream.push(&port), None);
        let zone = MidiMessage::ControlChange {
            channel: 0,
            controller: SWITCH_ZONE_CC,
            value: 0x04,
        };
        assert_eq!(stream.push(&zone), None);
        assert_eq!(
            stream.push(&port),
            Some(HuiSwitch {
                zone: 0x04,
                port: 3,
                pressed: true,
            })
        );
        // Zone is consumed: a second port needs a new select.
        assert_eq!(stream.push(&port), None);
        // Fader touch arrives through the same framing.
        let [select, touch] = fader_touch(5, true).unwrap();
        assert_eq!(stream.push(&select), None);
        assert_eq!(
            stream.push(&touch),
            Some(HuiSwitch {
                zone: 5,
                port: 0,
                pressed: true,
            })
        );
        let [_, release] = fader_touch(5, false).unwrap();
        let zone_again = MidiMessage::ControlChange {
            channel: 0,
            controller: SWITCH_ZONE_CC,
            value: 5,
        };
        assert_eq!(stream.push(&zone_again), None);
        assert_eq!(
            stream.push(&release),
            Some(HuiSwitch {
                zone: 5,
                port: 0,
                pressed: false,
            })
        );
    }

    #[test]
    fn vpot_and_jog_deltas_are_inverted_against_mcu() {
        let vpot = |value| {
            decode_vpot(&MidiMessage::ControlChange {
                channel: 0,
                controller: 0x42,
                value,
            })
        };
        assert_eq!(vpot(0x01), Some((2, -1)));
        assert_eq!(vpot(0x41), Some((2, 1)));
        assert_eq!(vpot(0x40), None);
        assert_eq!(
            decode_jog(&MidiMessage::ControlChange {
                channel: 0,
                controller: JOG_CC,
                value: 0x05,
            }),
            Some(-5)
        );
        assert_eq!(
            decode_jog(&MidiMessage::ControlChange {
                channel: 0,
                controller: JOG_CC,
                value: 0x45,
            }),
            Some(5)
        );
        assert!(is_system_reset(&MidiMessage::from_bytes(&[0xFF]).unwrap()));
    }
}
