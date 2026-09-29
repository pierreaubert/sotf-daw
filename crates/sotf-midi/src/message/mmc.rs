//! MIDI Machine Control: transport and locate commands.
//!
//! Wire formats follow the MIDI 1.0 Detailed Specification (MMC): Universal
//! Real Time SysEx `F0 7F <device> 06 <command> [...] F7`, where device
//! `0x7F` addresses all devices. Simple transport commands carry no
//! parameters; Write/Locate/Shuttle-family commands carry length-prefixed
//! parameter blocks. Command numbers 01-0D, 40-44, and 47 are per the
//! published command tables; 45/46/48/49 share the RP-013 speed or counter
//! framing noted below. Anything else decodes losslessly as
//! [`MmcCommand::Other`], and Sub-ID#1 `07` responses decode as
//! [`MmcResponse`] with a raw state byte, since published response-state
//! tables vary by manufacturer.

use super::mtc::MtcTime;
use crate::error::{MidiError, Result};
use serde::{Deserialize, Serialize};

/// One information field of a field-carrying MMC command
/// (`MaskedWrite`, `Read`, `Procedure`): field id, then length-prefixed data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MmcField {
    /// Field id byte.
    pub id: u8,
    /// Field payload bytes (7-bit each).
    pub data: Vec<u8>,
}

/// Shuttle-family speed triple per RP-013 Standard Speed: `sh` holds the
/// direction sign and integer part, `sm`/`sl` the fractional parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MmcShuttleSpeed {
    /// Speed high byte (sign + integer).
    pub sh: u8,
    /// Speed middle byte.
    pub sm: u8,
    /// Speed low byte.
    pub sl: u8,
}

/// An MMC command (Universal Real Time SysEx Sub-ID#1 `06`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MmcCommand {
    /// 01: stop playback/recording.
    Stop,
    /// 02: start playback.
    Play,
    /// 03: start playback once no longer busy.
    DeferredPlay,
    /// 04: fast forward.
    FastForward,
    /// 05: rewind.
    Rewind,
    /// 06: enter record on armed tracks (punch in).
    RecordStrobe,
    /// 07: exit record (punch out).
    RecordExit,
    /// 08: pause recording.
    RecordPause,
    /// 09: pause playback.
    Pause,
    /// 0A: disengage media.
    Eject,
    /// 0B: chase incoming timecode.
    Chase,
    /// 0C: clear the error-halt flag and resume command processing.
    CommandErrorReset,
    /// 0D: reset to default/startup state.
    MmcReset,
    /// 40: arm tracks. The bitmap assigns one bit per track, least
    /// significant bit first.
    Write {
        /// Track-arm bitmap bytes.
        track_bitmap: Vec<u8>,
    },
    /// 41: masked write of information fields.
    MaskedWrite {
        /// Information fields to write.
        fields: Vec<MmcField>,
    },
    /// 42: read of information fields.
    Read {
        /// Information fields to read.
        fields: Vec<MmcField>,
    },
    /// 43: procedure call with information fields.
    Procedure {
        /// Procedure fields.
        fields: Vec<MmcField>,
    },
    /// 44: cue to a time (`06 01 <hr> <mn> <sc> <fr> <ff>`); the `hr`
    /// byte encodes the rate exactly like an MTC full frame.
    Locate {
        /// Target time.
        time: MtcTime,
        /// Subframe code (device-specific scale, passed through).
        subframes: u8,
    },
    /// 45: play at a shuttle speed.
    VariablePlay {
        /// Playback speed.
        speed: MmcShuttleSpeed,
    },
    /// 46: search at a shuttle speed.
    Search {
        /// Search speed.
        speed: MmcShuttleSpeed,
    },
    /// 47: shuttle at a signed speed.
    Shuttle {
        /// Shuttle speed.
        speed: MmcShuttleSpeed,
    },
    /// 48: step a signed frame count.
    Step {
        /// Signed step count.
        steps: i8,
    },
    /// 49: assign the track counter.
    AssignTrackCounter {
        /// Counter value, transmitted most significant byte first.
        counter: u16,
    },
    /// Any other command number, with its raw parameter bytes. This keeps
    /// parsing total: unknown or manufacturer-specific commands still
    /// round-trip byte-exactly.
    Other {
        /// Command number.
        command: u8,
        /// Raw parameter bytes.
        data: Vec<u8>,
    },
}

impl MmcCommand {
    /// The Sub-ID#2 command number.
    pub fn number(&self) -> u8 {
        match self {
            Self::Stop => 0x01,
            Self::Play => 0x02,
            Self::DeferredPlay => 0x03,
            Self::FastForward => 0x04,
            Self::Rewind => 0x05,
            Self::RecordStrobe => 0x06,
            Self::RecordExit => 0x07,
            Self::RecordPause => 0x08,
            Self::Pause => 0x09,
            Self::Eject => 0x0A,
            Self::Chase => 0x0B,
            Self::CommandErrorReset => 0x0C,
            Self::MmcReset => 0x0D,
            Self::Write { .. } => 0x40,
            Self::MaskedWrite { .. } => 0x41,
            Self::Read { .. } => 0x42,
            Self::Procedure { .. } => 0x43,
            Self::Locate { .. } => 0x44,
            Self::VariablePlay { .. } => 0x45,
            Self::Search { .. } => 0x46,
            Self::Shuttle { .. } => 0x47,
            Self::Step { .. } => 0x48,
            Self::AssignTrackCounter { .. } => 0x49,
            Self::Other { command, .. } => *command,
        }
    }

    /// Encode the parameter bytes following the command number.
    /// Length bytes are 7-bit by construction, so payloads that do not fit
    /// (track bitmaps over 125 bytes, fields over 127 bytes) cannot be
    /// represented and produce an error.
    pub fn encode_params(&self) -> Result<Vec<u8>> {
        match self {
            Self::Stop
            | Self::Play
            | Self::DeferredPlay
            | Self::FastForward
            | Self::Rewind
            | Self::RecordStrobe
            | Self::RecordExit
            | Self::RecordPause
            | Self::Pause
            | Self::Eject
            | Self::Chase
            | Self::CommandErrorReset
            | Self::MmcReset => Ok(Vec::new()),
            Self::Write { track_bitmap } => {
                if track_bitmap.len() > 125 {
                    return Err(MidiError::InvalidMessage(format!(
                        "MMC Write track bitmap too long: {} bytes",
                        track_bitmap.len()
                    )));
                }
                let mut params = Vec::with_capacity(3 + track_bitmap.len());
                params.push((2 + track_bitmap.len()) as u8);
                params.push(0x4F);
                params.push(track_bitmap.len() as u8);
                params.extend_from_slice(track_bitmap);
                Ok(params)
            }
            Self::MaskedWrite { fields } | Self::Read { fields } | Self::Procedure { fields } => {
                encode_fields(fields)
            }
            Self::Locate { time, subframes } => Ok(vec![
                0x06,
                0x01,
                (time.rate.rate_bits() << 5) | (time.hours & 0x1F),
                time.minutes,
                time.seconds,
                time.frames,
                subframes & 0x7F,
            ]),
            Self::VariablePlay { speed } | Self::Search { speed } | Self::Shuttle { speed } => {
                Ok(vec![0x03, speed.sh, speed.sm, speed.sl])
            }
            Self::Step { steps } => {
                // 7-bit two's complement spans -64..=63.
                if *steps < -64 || *steps > 63 {
                    return Err(MidiError::InvalidMessage(format!(
                        "MMC Step count out of range: {steps}"
                    )));
                }
                Ok(vec![0x01, *steps as u8 & 0x7F])
            }
            Self::AssignTrackCounter { counter } => {
                if *counter > 0x7F7F {
                    return Err(MidiError::InvalidMessage(format!(
                        "MMC track counter out of range: {counter:#X}"
                    )));
                }
                Ok(vec![
                    0x02,
                    ((counter >> 8) & 0x7F) as u8,
                    (counter & 0x7F) as u8,
                ])
            }
            Self::Other { data, .. } => Ok(data.clone()),
        }
    }

    /// Decode the parameter bytes following a command number.
    /// Returns `None` when the framing is malformed; callers fall back to
    /// generic SysEx handling.
    pub fn decode(command: u8, params: &[u8]) -> Option<Self> {
        if params.iter().any(|b| b & 0x80 != 0) {
            return None;
        }
        let simple = |command: MmcCommand| params.is_empty().then_some(command);
        match command {
            0x01 => simple(Self::Stop),
            0x02 => simple(Self::Play),
            0x03 => simple(Self::DeferredPlay),
            0x04 => simple(Self::FastForward),
            0x05 => simple(Self::Rewind),
            0x06 => simple(Self::RecordStrobe),
            0x07 => simple(Self::RecordExit),
            0x08 => simple(Self::RecordPause),
            0x09 => simple(Self::Pause),
            0x0A => simple(Self::Eject),
            0x0B => simple(Self::Chase),
            0x0C => simple(Self::CommandErrorReset),
            0x0D => simple(Self::MmcReset),
            0x40 => {
                // <length1> 4F <length2> <track-bitmap>.
                if params.len() < 3 || params[1] != 0x4F {
                    return None;
                }
                let bitmap_len = params[2] as usize;
                if params[0] as usize != 2 + bitmap_len || params.len() != 3 + bitmap_len {
                    return None;
                }
                Some(Self::Write {
                    track_bitmap: params[3..].to_vec(),
                })
            }
            0x41..=0x43 => {
                let fields = decode_fields(params)?;
                match command {
                    0x41 => Some(Self::MaskedWrite { fields }),
                    0x42 => Some(Self::Read { fields }),
                    _ => Some(Self::Procedure { fields }),
                }
            }
            0x44 => {
                // 06 01 <hr> <mn> <sc> <fr> <ff>.
                if params.len() != 7 || params[0] != 0x06 || params[1] != 0x01 {
                    return None;
                }
                let hr = params[2];
                if hr & 0x80 != 0 {
                    return None;
                }
                let time = MtcTime::new(
                    super::mtc::MtcFrameRate::from_rate_bits(hr >> 5)?,
                    hr & 0x1F,
                    params[3],
                    params[4],
                    params[5],
                )
                .ok()?;
                Some(Self::Locate {
                    time,
                    subframes: params[6],
                })
            }
            0x45..=0x47 => {
                // 03 <sh> <sm> <sl> shuttle-family speed triple.
                if params.len() != 4 || params[0] != 0x03 {
                    return None;
                }
                let speed = MmcShuttleSpeed {
                    sh: params[1],
                    sm: params[2],
                    sl: params[3],
                };
                match command {
                    0x45 => Some(Self::VariablePlay { speed }),
                    0x46 => Some(Self::Search { speed }),
                    _ => Some(Self::Shuttle { speed }),
                }
            }
            0x48 => {
                // 01 <steps>, transmitted 7-bit two's complement.
                if params.len() != 2 || params[0] != 0x01 {
                    return None;
                }
                let raw = params[1];
                let steps = if raw & 0x40 != 0 {
                    (raw | 0x80) as i8
                } else {
                    raw as i8
                };
                Some(Self::Step { steps })
            }
            0x49 => {
                // 02 <counter msb> <counter lsb>.
                if params.len() != 3 || params[0] != 0x02 {
                    return None;
                }
                Some(Self::AssignTrackCounter {
                    counter: ((params[1] as u16) << 8) | params[2] as u16,
                })
            }
            _ => Some(Self::Other {
                command,
                data: params.to_vec(),
            }),
        }
    }

    /// Encode a complete command message for `device`.
    pub fn to_sysex(&self, device: u8) -> Result<Vec<u8>> {
        let params = self.encode_params()?;
        let mut bytes = Vec::with_capacity(6 + params.len());
        bytes.extend_from_slice(&[0xF0, 0x7F, device & 0x7F, 0x06, self.number()]);
        bytes.extend_from_slice(&params);
        bytes.push(0xF7);
        Ok(bytes)
    }

    /// Decode a complete command message, returning device and command.
    /// Returns `None` for anything that is not a well-formed MMC command;
    /// callers fall back to generic SysEx handling.
    pub fn from_sysex(bytes: &[u8]) -> Option<(u8, Self)> {
        if bytes.len() < 6
            || bytes[0] != 0xF0
            || bytes[1] != 0x7F
            || bytes[3] != 0x06
            || bytes[bytes.len() - 1] != 0xF7
        {
            return None;
        }
        if bytes[2] & 0x80 != 0 || bytes[4] & 0x80 != 0 {
            return None;
        }
        let command = Self::decode(bytes[4], &bytes[5..bytes.len() - 1])?;
        Some((bytes[2], command))
    }
}

/// An MMC response (Universal Real Time SysEx Sub-ID#1 `07`).
/// Response-state codes vary by manufacturer, so the state byte and any
/// trailing payload travel untyped while still round-tripping exactly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MmcResponse {
    /// Target device id (`0x7F` addresses all devices).
    pub device: u8,
    /// Response-state byte.
    pub state: u8,
    /// Trailing response payload bytes.
    pub data: Vec<u8>,
}

impl MmcResponse {
    /// Encode a complete response message.
    pub fn to_sysex(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(6 + self.data.len());
        bytes.extend_from_slice(&[0xF0, 0x7F, self.device & 0x7F, 0x07, self.state & 0x7F]);
        for byte in &self.data {
            bytes.push(byte & 0x7F);
        }
        bytes.push(0xF7);
        bytes
    }

    /// Decode a complete response message. Returns `None` for anything
    /// that is not a well-formed MMC response.
    pub fn from_sysex(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 6
            || bytes[0] != 0xF0
            || bytes[1] != 0x7F
            || bytes[3] != 0x07
            || bytes[bytes.len() - 1] != 0xF7
        {
            return None;
        }
        if bytes[2..bytes.len() - 1].iter().any(|b| b & 0x80 != 0) {
            return None;
        }
        Some(Self {
            device: bytes[2],
            state: bytes[4],
            data: bytes[5..bytes.len() - 1].to_vec(),
        })
    }
}

/// Encode information fields as `<total-length> <id> <len> <data> ...`.
fn encode_fields(fields: &[MmcField]) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    for field in fields {
        if field.data.len() > 127 {
            return Err(MidiError::InvalidMessage(format!(
                "MMC field 0x{:02X} too long: {} bytes",
                field.id,
                field.data.len()
            )));
        }
        if field.data.iter().any(|b| b & 0x80 != 0) {
            return Err(MidiError::InvalidMessage(format!(
                "MMC field 0x{:02X} has high bit set",
                field.id
            )));
        }
        body.push(field.id & 0x7F);
        body.push(field.data.len() as u8);
        body.extend_from_slice(&field.data);
    }
    if body.len() > 127 {
        return Err(MidiError::InvalidMessage(format!(
            "MMC information fields too long: {} bytes",
            body.len()
        )));
    }
    let mut params = Vec::with_capacity(1 + body.len());
    params.push(body.len() as u8);
    params.extend_from_slice(&body);
    Ok(params)
}

/// Decode `<total-length> <id> <len> <data> ...` information fields.
fn decode_fields(params: &[u8]) -> Option<Vec<MmcField>> {
    if params.is_empty() || params[0] as usize != params.len() - 1 {
        return None;
    }
    let mut fields = Vec::new();
    let mut cursor = 1;
    while cursor < params.len() {
        if cursor + 1 >= params.len() {
            return None;
        }
        let id = params[cursor];
        let len = params[cursor + 1] as usize;
        if params.len() < cursor + 2 + len {
            return None;
        }
        fields.push(MmcField {
            id,
            data: params[cursor + 2..cursor + 2 + len].to_vec(),
        });
        cursor += 2 + len;
    }
    Some(fields)
}

#[cfg(test)]
mod tests {
    use super::super::mtc::{MtcFrameRate, MtcTime};
    use super::*;

    fn time() -> MtcTime {
        MtcTime::new(MtcFrameRate::Fps30, 10, 20, 30, 12).unwrap()
    }

    #[test]
    fn simple_commands_round_trip() {
        for command in [
            MmcCommand::Stop,
            MmcCommand::Play,
            MmcCommand::DeferredPlay,
            MmcCommand::FastForward,
            MmcCommand::Rewind,
            MmcCommand::RecordStrobe,
            MmcCommand::RecordExit,
            MmcCommand::RecordPause,
            MmcCommand::Pause,
            MmcCommand::Eject,
            MmcCommand::Chase,
            MmcCommand::CommandErrorReset,
            MmcCommand::MmcReset,
        ] {
            let bytes = command.to_sysex(0x7F).unwrap();
            let number = command.number();
            assert_eq!(&bytes[..5], &[0xF0, 0x7F, 0x7F, 0x06, number]);
            assert_eq!(bytes[bytes.len() - 1], 0xF7);
            let (device, decoded) = MmcCommand::from_sysex(&bytes).unwrap();
            assert_eq!(device, 0x7F);
            assert_eq!(decoded, command);
        }
    }

    #[test]
    fn simple_commands_reject_trailing_params() {
        assert!(MmcCommand::from_sysex(&[0xF0, 0x7F, 0x7F, 0x06, 0x01, 0x00, 0xF7]).is_none());
    }

    #[test]
    fn write_encodes_track_bitmap_framing() {
        let command = MmcCommand::Write {
            track_bitmap: vec![0b0000_0101, 0b0000_0001],
        };
        let bytes = command.to_sysex(0x01).unwrap();
        assert_eq!(
            bytes,
            vec![
                0xF0, 0x7F, 0x01, 0x06, 0x40, 0x04, 0x4F, 0x02, 0x05, 0x01, 0xF7
            ]
        );
        let (device, decoded) = MmcCommand::from_sysex(&bytes).unwrap();
        assert_eq!(device, 0x01);
        assert_eq!(decoded, command);
    }

    #[test]
    fn locate_encodes_target_time() {
        let command = MmcCommand::Locate {
            time: time(),
            subframes: 50,
        };
        let bytes = command.to_sysex(0x7F).unwrap();
        // 30 fps -> rate bits 11, hours 10 -> 0x6A.
        assert_eq!(
            bytes,
            vec![
                0xF0, 0x7F, 0x7F, 0x06, 0x44, 0x06, 0x01, 0x6A, 20, 30, 12, 50, 0xF7
            ]
        );
        let (_, decoded) = MmcCommand::from_sysex(&bytes).unwrap();
        assert_eq!(decoded, command);
    }

    #[test]
    fn shuttle_family_round_trips_speed_triple() {
        let speed = MmcShuttleSpeed {
            sh: 0x41,
            sm: 0x00,
            sl: 0x00,
        };
        for command in [
            MmcCommand::VariablePlay { speed },
            MmcCommand::Search { speed },
            MmcCommand::Shuttle { speed },
        ] {
            let bytes = command.to_sysex(0x7F).unwrap();
            let (_, decoded) = MmcCommand::from_sysex(&bytes).unwrap();
            assert_eq!(decoded, command);
        }
    }

    #[test]
    fn step_and_counter_round_trip() {
        for steps in [-64i8, -1, 0, 1, 63] {
            let command = MmcCommand::Step { steps };
            let bytes = command.to_sysex(0x7F).unwrap();
            let (_, decoded) = MmcCommand::from_sysex(&bytes).unwrap();
            assert_eq!(decoded, command);
        }
        let command = MmcCommand::AssignTrackCounter { counter: 0x1234 };
        let bytes = command.to_sysex(0x7F).unwrap();
        assert_eq!(&bytes[5..], &[0x02, 0x12, 0x34, 0xF7]);
        let (_, decoded) = MmcCommand::from_sysex(&bytes).unwrap();
        assert_eq!(decoded, command);
    }

    #[test]
    fn step_rejects_bad_framing_and_out_of_range_values() {
        // Wrong length byte.
        assert!(
            MmcCommand::from_sysex(&[0xF0, 0x7F, 0x7F, 0x06, 0x48, 0x02, 0x40, 0x00, 0xF7])
                .is_none()
        );
        // Encode refuses values outside 7-bit two's complement.
        assert!(MmcCommand::Step { steps: 64 }.to_sysex(0x7F).is_err());
        assert!(MmcCommand::Step { steps: -65 }.to_sysex(0x7F).is_err());
        assert!(
            MmcCommand::AssignTrackCounter { counter: 0x8000 }
                .to_sysex(0x7F)
                .is_err()
        );
    }

    #[test]
    fn field_commands_round_trip() {
        let fields = vec![
            MmcField {
                id: 0x4F,
                data: vec![0x01, 0x02],
            },
            MmcField {
                id: 0x00,
                data: vec![],
            },
        ];
        for command in [
            MmcCommand::MaskedWrite {
                fields: fields.clone(),
            },
            MmcCommand::Read {
                fields: fields.clone(),
            },
            MmcCommand::Procedure {
                fields: fields.clone(),
            },
        ] {
            let bytes = command.to_sysex(0x7F).unwrap();
            let (_, decoded) = MmcCommand::from_sysex(&bytes).unwrap();
            assert_eq!(decoded, command);
        }
    }

    #[test]
    fn unknown_commands_round_trip_losslessly() {
        let command = MmcCommand::Other {
            command: 0x55,
            data: vec![0x01, 0x02, 0x03],
        };
        let bytes = command.to_sysex(0x10).unwrap();
        let (device, decoded) = MmcCommand::from_sysex(&bytes).unwrap();
        assert_eq!(device, 0x10);
        assert_eq!(decoded, command);
    }

    #[test]
    fn response_round_trips() {
        let response = MmcResponse {
            device: 0x01,
            state: 0x02,
            data: vec![0x06, 0x01, 0x00],
        };
        let bytes = response.to_sysex();
        assert_eq!(&bytes[..5], &[0xF0, 0x7F, 0x01, 0x07, 0x02]);
        assert_eq!(MmcResponse::from_sysex(&bytes).unwrap(), response);
        assert!(MmcResponse::from_sysex(&[0xF0, 0x7F]).is_none());
    }

    #[test]
    fn rejects_non_mmc_sysex() {
        // MTC full frame is not an MMC command.
        assert!(
            MmcCommand::from_sysex(&[0xF0, 0x7F, 0x7F, 0x01, 0x01, 0x6A, 20, 30, 12, 0xF7])
                .is_none()
        );
        // High bit in device id.
        assert!(MmcCommand::from_sysex(&[0xF0, 0x7F, 0x80, 0x06, 0x01, 0xF7]).is_none());
    }
}
