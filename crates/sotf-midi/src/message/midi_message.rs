use super::misc::channel_voice_data_len;
use super::misc::required_data_bytes;
use super::misc::system_data_len;
use super::misc::validate_data_bytes;
use super::mmc::{MmcCommand, MmcResponse};
use super::mtc::{MtcQuarterFrameKind, MtcTime};
use super::sysex::{
    BulkTuningDump, GmMode, ManufacturerId, MasterControl, Realtime, ScaleChannels,
    ScaleOctaveDump, SingleNoteChange,
};
use crate::error::{MidiError, Result};
use serde::{Deserialize, Serialize};

/// A MIDI message
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MidiMessage {
    /// Note Off: channel (0-15), note (0-127), velocity (0-127)
    NoteOff { channel: u8, note: u8, velocity: u8 },

    /// Note On: channel (0-15), note (0-127), velocity (0-127)
    NoteOn { channel: u8, note: u8, velocity: u8 },

    /// Polyphonic Aftertouch: channel (0-15), note (0-127), pressure (0-127)
    PolyphonicAftertouch { channel: u8, note: u8, pressure: u8 },

    /// Control Change: channel (0-15), controller (0-127), value (0-127)
    ControlChange {
        channel: u8,
        controller: u8,
        value: u8,
    },

    /// Program Change: channel (0-15), program (0-127)
    ProgramChange { channel: u8, program: u8 },

    /// Channel Aftertouch: channel (0-15), pressure (0-127)
    ChannelAftertouch { channel: u8, pressure: u8 },

    /// Pitch Bend: channel (0-15), value (0-16383, 8192 is center)
    PitchBend { channel: u8, value: u16 },

    /// System Exclusive message
    SystemExclusive { data: Vec<u8> },

    /// Fixed-size System Common / Real-Time / EOX message.
    ///
    /// `len` is the number of valid bytes in `data` (0-2). Keeping these
    /// messages stack-sized avoids heap allocation for MIDI Clock, Active
    /// Sensing, Song Select, and Song Position Pointer. MTC quarter-frame
    /// has its own typed variant below.
    System { status: u8, data: [u8; 2], len: u8 },

    /// MTC full-frame SysEx (`F0 7F <device> 01 01 <hr> <mn> <sc> <fr> F7`).
    MtcFullFrame { device: u8, time: MtcTime },

    /// MTC quarter-frame message (`F1 <type-nibble value-nibble>`).
    MtcQuarterFrame {
        /// Which nibble of the running time this message carries.
        kind: MtcQuarterFrameKind,
        /// Value nibble, range-checked per kind.
        value: u8,
    },

    /// MMC command (Universal Real Time SysEx Sub-ID#1 `06`).
    ///
    /// Commands whose parameters exceed 7-bit length framing (track
    /// bitmaps over 125 bytes, information fields over 127 bytes, steps
    /// outside -64..=63, counters above `0x7F7F`) cannot be represented on
    /// the wire and encode as empty rather than panicking.
    Mmc { device: u8, command: MmcCommand },

    /// MMC response (Universal Real Time SysEx Sub-ID#1 `07`).
    MmcResponse { device: u8, state: u8, data: Vec<u8> },

    /// Identity Request (`F0 7E <device> 06 01 F7`).
    IdentityRequest { device: u8 },

    /// Identity Reply (`F0 7E <device> 06 02 ... F7`).
    IdentityReply {
        /// Target device id.
        device: u8,
        /// Manufacturer id (1- or 3-byte form).
        manufacturer: ManufacturerId,
        /// Device family, little-endian.
        family: u16,
        /// Device model, little-endian.
        model: u16,
        /// Software revision.
        version: [u8; 4],
    },

    /// General MIDI system mode (`F0 7E <device> 09 01|02|03 F7`).
    GmSystem { device: u8, mode: GmMode },

    /// Master device control (`F0 7F <device> 04 sub <ll> <mm> F7`).
    ///
    /// Like MMC, unrepresentable 14-bit values encode as empty rather
    /// than panicking.
    MasterControl { device: u8, control: MasterControl },

    /// MTS Single Note Tuning Change, plain (sub `02`, real-time) or
    /// bank form (sub `07`, either header).
    MtsSingleNote {
        /// Real-time header (`Yes`) or setup header (`No`).
        realtime: Realtime,
        /// Target device id.
        device: u8,
        /// Bank number (`None` = plain real-time form).
        bank: Option<u8>,
        /// Tuning program number.
        program: u8,
        /// Retuned keys.
        changes: Vec<SingleNoteChange>,
    },

    /// MTS Scale/Octave 1-byte change (sub `08`, ±64 cents per class).
    MtsScaleOctave {
        /// Real-time header (`Yes`) or setup header (`No`).
        realtime: Realtime,
        /// Target device id.
        device: u8,
        /// Channel bitmap bytes.
        channels: ScaleChannels,
        /// Cent offsets C..B.
        offsets: [i8; 12],
    },

    /// MTS Scale/Octave 2-byte change (sub `09`, 14-bit values).
    MtsScaleOctave14 {
        /// Real-time header (`Yes`) or setup header (`No`).
        realtime: Realtime,
        /// Target device id.
        device: u8,
        /// Channel bitmap bytes.
        channels: ScaleChannels,
        /// 14-bit values C..B (8192 = equal temperament).
        values: [u16; 12],
    },

    /// MTS Bulk Tuning Dump Request (sub `00`).
    MtsBulkRequest { device: u8, program: u8 },

    /// MTS Bank Bulk Tuning Dump Request (sub `03`).
    MtsBulkRequestBank { device: u8, bank: u8, program: u8 },

    /// MTS Bulk Tuning Dump Reply (sub `01`, 408 bytes; boxed for size).
    MtsBulkDump { device: u8, dump: Box<BulkTuningDump> },

    /// MTS Scale/Octave Dump, 1-byte (sub `05`) or 2-byte (sub `06`).
    MtsScaleOctaveDump { device: u8, dump: Box<ScaleOctaveDump> },

    /// Raw MIDI bytes (for unsupported messages)
    Raw { data: Vec<u8> },
}

impl MidiMessage {
    /// Parse a MIDI message from raw bytes.
    ///
    /// The first byte MUST be a status byte (top bit set). Data-only / running-status
    /// bytes are rejected — callers that need to apply running status must reconstruct
    /// the full message (status + data) before calling this. See `from_bytes_with_status`.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.is_empty() {
            return Err(MidiError::InvalidMessage("Empty message".to_string()));
        }

        let status = bytes[0];
        // Reject data-only bytes — these would silently be parsed as Raw otherwise.
        if status & 0x80 == 0 {
            return Err(MidiError::InvalidMessage(format!(
                "First byte 0x{:02X} is not a status byte (high bit must be set); running-status data without a status byte cannot be parsed standalone",
                status
            )));
        }
        let message_type = status & 0xF0;
        let channel = status & 0x0F;

        match message_type {
            0x80 => {
                // Note Off
                let data = required_data_bytes(bytes, 2, "Note Off")?;
                Ok(MidiMessage::NoteOff {
                    channel,
                    note: data[0],
                    velocity: data[1],
                })
            }
            0x90 => {
                // Note On
                let data = required_data_bytes(bytes, 2, "Note On")?;
                let velocity = data[1];
                // Per MIDI convention, Note On with velocity 0 is equivalent
                // to Note Off. A real 0x80 Note Off can still carry release
                // velocity; this 0x90 form cannot, so the normalized release
                // velocity is necessarily 0.
                if velocity == 0 {
                    Ok(MidiMessage::NoteOff {
                        channel,
                        note: data[0],
                        velocity: 0,
                    })
                } else {
                    Ok(MidiMessage::NoteOn {
                        channel,
                        note: data[0],
                        velocity,
                    })
                }
            }
            0xA0 => {
                // Polyphonic Aftertouch
                let data = required_data_bytes(bytes, 2, "Polyphonic Aftertouch")?;
                Ok(MidiMessage::PolyphonicAftertouch {
                    channel,
                    note: data[0],
                    pressure: data[1],
                })
            }
            0xB0 => {
                // Control Change
                let data = required_data_bytes(bytes, 2, "Control Change")?;
                Ok(MidiMessage::ControlChange {
                    channel,
                    controller: data[0],
                    value: data[1],
                })
            }
            0xC0 => {
                // Program Change
                let data = required_data_bytes(bytes, 1, "Program Change")?;
                Ok(MidiMessage::ProgramChange {
                    channel,
                    program: data[0],
                })
            }
            0xD0 => {
                // Channel Aftertouch
                let data = required_data_bytes(bytes, 1, "Channel Aftertouch")?;
                Ok(MidiMessage::ChannelAftertouch {
                    channel,
                    pressure: data[0],
                })
            }
            0xE0 => {
                // Pitch Bend
                let data = required_data_bytes(bytes, 2, "Pitch Bend")?;
                let lsb = data[0];
                let msb = data[1];
                let value = ((msb as u16) << 7) | (lsb as u16);
                Ok(MidiMessage::PitchBend { channel, value })
            }
            0xF0 => {
                // System messages
                if status == 0xF0 {
                    // System Exclusive; recognize typed MTC/MMC universal
                    // messages, keep everything else generic.
                    Ok(parse_system_exclusive(bytes))
                } else if status == 0xF1 {
                    parse_quarter_frame(bytes)
                } else {
                    parse_system_message(bytes)
                }
            }
            _ => {
                // Unknown message type - store as raw
                Ok(MidiMessage::Raw {
                    data: bytes.to_vec(),
                })
            }
        }
    }

    /// Parse a MIDI message from raw bytes, applying `running_status` if `bytes[0]`
    /// is a data byte. Returns an error if there's no status byte and no running status.
    pub fn from_bytes_with_status(bytes: &[u8], running_status: Option<u8>) -> Result<Self> {
        if bytes.is_empty() {
            return Err(MidiError::InvalidMessage("Empty message".to_string()));
        }
        if bytes[0] & 0x80 != 0 {
            return Self::from_bytes(bytes);
        }
        let status = running_status.ok_or_else(|| {
            MidiError::InvalidMessage("Data-only bytes without running status context".to_string())
        })?;
        if status & 0x80 == 0 {
            return Err(MidiError::InvalidMessage(format!(
                "running_status 0x{:02X} is not a valid status byte",
                status
            )));
        }
        let data_len = channel_voice_data_len(status).ok_or_else(|| {
            MidiError::InvalidMessage(format!(
                "running_status 0x{:02X} is not a channel-voice status",
                status
            ))
        })?;
        if bytes.len() < data_len {
            return Err(MidiError::InvalidMessage(format!(
                "Running-status message 0x{:02X} too short: need {} data bytes, got {}",
                status,
                data_len,
                bytes.len()
            )));
        }
        let mut buf = [0u8; 3];
        buf[0] = status;
        buf[1..1 + data_len].copy_from_slice(&bytes[..data_len]);
        Self::from_bytes(&buf[..1 + data_len])
    }

    /// Write the MIDI message bytes into `out`, returning the number of bytes written.
    /// For channel-voice messages this writes at most 3 bytes without allocating.
    /// If `out` is too small the function still returns the required length so the
    /// caller can fall back to `to_bytes()`.
    pub fn write_to(&self, out: &mut [u8]) -> usize {
        match self {
            MidiMessage::NoteOff {
                channel,
                note,
                velocity,
            } => {
                if out.len() >= 3 {
                    out[0] = 0x80 | (channel & 0x0F);
                    out[1] = note & 0x7F;
                    out[2] = velocity & 0x7F;
                }
                3
            }
            MidiMessage::NoteOn {
                channel,
                note,
                velocity,
            } => {
                if out.len() >= 3 {
                    out[0] = 0x90 | (channel & 0x0F);
                    out[1] = note & 0x7F;
                    out[2] = velocity & 0x7F;
                }
                3
            }
            MidiMessage::PolyphonicAftertouch {
                channel,
                note,
                pressure,
            } => {
                if out.len() >= 3 {
                    out[0] = 0xA0 | (channel & 0x0F);
                    out[1] = note & 0x7F;
                    out[2] = pressure & 0x7F;
                }
                3
            }
            MidiMessage::ControlChange {
                channel,
                controller,
                value,
            } => {
                if out.len() >= 3 {
                    out[0] = 0xB0 | (channel & 0x0F);
                    out[1] = controller & 0x7F;
                    out[2] = value & 0x7F;
                }
                3
            }
            MidiMessage::ProgramChange { channel, program } => {
                if out.len() >= 2 {
                    out[0] = 0xC0 | (channel & 0x0F);
                    out[1] = program & 0x7F;
                }
                2
            }
            MidiMessage::ChannelAftertouch { channel, pressure } => {
                if out.len() >= 2 {
                    out[0] = 0xD0 | (channel & 0x0F);
                    out[1] = pressure & 0x7F;
                }
                2
            }
            MidiMessage::PitchBend { channel, value } => {
                if out.len() >= 3 {
                    out[0] = 0xE0 | (channel & 0x0F);
                    out[1] = (value & 0x7F) as u8;
                    out[2] = ((value >> 7) & 0x7F) as u8;
                }
                3
            }
            MidiMessage::System { status, data, len } => {
                let len = (*len as usize).min(data.len());
                let required = 1 + len;
                if out.len() >= required {
                    out[0] = *status;
                    out[1..required].copy_from_slice(&data[..len]);
                }
                required
            }
            MidiMessage::MtcFullFrame { device, time } => {
                let bytes = time.to_full_frame_bytes(*device);
                if out.len() >= bytes.len() {
                    out[..bytes.len()].copy_from_slice(&bytes);
                }
                bytes.len()
            }
            MidiMessage::MtcQuarterFrame { kind, value } => {
                if out.len() >= 2 {
                    out[0] = 0xF1;
                    out[1] = kind.to_byte(*value);
                }
                2
            }
            MidiMessage::Mmc { device, command } => {
                // Unrepresentable values (e.g. a >125-byte track bitmap)
                // encode as empty rather than panicking; see the variant docs.
                let bytes = command.to_sysex(*device).unwrap_or_default();
                if out.len() >= bytes.len() {
                    out[..bytes.len()].copy_from_slice(&bytes);
                }
                bytes.len()
            }
            MidiMessage::MmcResponse { device, state, data } => {
                let response = MmcResponse {
                    device: *device,
                    state: *state,
                    data: data.clone(),
                };
                let bytes = response.to_sysex();
                if out.len() >= bytes.len() {
                    out[..bytes.len()].copy_from_slice(&bytes);
                }
                bytes.len()
            }
            MidiMessage::IdentityRequest { .. }
            | MidiMessage::IdentityReply { .. }
            | MidiMessage::GmSystem { .. }
            | MidiMessage::MasterControl { .. }
            | MidiMessage::MtsSingleNote { .. }
            | MidiMessage::MtsScaleOctave { .. }
            | MidiMessage::MtsScaleOctave14 { .. }
            | MidiMessage::MtsBulkRequest { .. }
            | MidiMessage::MtsBulkRequestBank { .. }
            | MidiMessage::MtsBulkDump { .. }
            | MidiMessage::MtsScaleOctaveDump { .. } => {
                let bytes = self.to_bytes();
                if out.len() >= bytes.len() {
                    out[..bytes.len()].copy_from_slice(&bytes);
                }
                bytes.len()
            }
            MidiMessage::SystemExclusive { data } | MidiMessage::Raw { data } => {
                if out.len() >= data.len() {
                    out[..data.len()].copy_from_slice(data);
                }
                data.len()
            }
        }
    }

    /// Convert the MIDI message to raw bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            MidiMessage::NoteOff {
                channel,
                note,
                velocity,
            } => {
                vec![0x80 | (channel & 0x0F), note & 0x7F, velocity & 0x7F]
            }
            MidiMessage::NoteOn {
                channel,
                note,
                velocity,
            } => {
                vec![0x90 | (channel & 0x0F), note & 0x7F, velocity & 0x7F]
            }
            MidiMessage::PolyphonicAftertouch {
                channel,
                note,
                pressure,
            } => {
                vec![0xA0 | (channel & 0x0F), note & 0x7F, pressure & 0x7F]
            }
            MidiMessage::ControlChange {
                channel,
                controller,
                value,
            } => {
                vec![0xB0 | (channel & 0x0F), controller & 0x7F, value & 0x7F]
            }
            MidiMessage::ProgramChange { channel, program } => {
                vec![0xC0 | (channel & 0x0F), program & 0x7F]
            }
            MidiMessage::ChannelAftertouch { channel, pressure } => {
                vec![0xD0 | (channel & 0x0F), pressure & 0x7F]
            }
            MidiMessage::PitchBend { channel, value } => {
                let lsb = (value & 0x7F) as u8;
                let msb = ((value >> 7) & 0x7F) as u8;
                vec![0xE0 | (channel & 0x0F), lsb, msb]
            }
            MidiMessage::System { status, data, len } => {
                let len = (*len as usize).min(data.len());
                let mut bytes = Vec::with_capacity(1 + len);
                bytes.push(*status);
                bytes.extend_from_slice(&data[..len]);
                bytes
            }
            MidiMessage::MtcFullFrame { device, time } => {
                time.to_full_frame_bytes(*device).to_vec()
            }
            MidiMessage::MtcQuarterFrame { kind, value } => {
                vec![0xF1, kind.to_byte(*value)]
            }
            MidiMessage::Mmc { device, command } => {
                command.to_sysex(*device).unwrap_or_default()
            }
            MidiMessage::MmcResponse { device, state, data } => {
                MmcResponse {
                    device: *device,
                    state: *state,
                    data: data.clone(),
                }
                .to_sysex()
            }
            MidiMessage::IdentityRequest { device } => {
                super::sysex::encode_identity_request(*device)
            }
            MidiMessage::IdentityReply {
                device,
                manufacturer,
                family,
                model,
                version,
            } => super::sysex::encode_identity_reply(
                *device,
                &super::sysex::IdentityReply {
                    manufacturer: *manufacturer,
                    family: *family,
                    model: *model,
                    version: *version,
                },
            ),
            MidiMessage::GmSystem { device, mode } => {
                super::sysex::encode_gm_system(*device, *mode)
            }
            MidiMessage::MasterControl { device, control } => super::sysex::encode_master_control(
                *device,
                *control,
            )
            .unwrap_or_default(),
            MidiMessage::MtsSingleNote {
                realtime,
                device,
                bank,
                program,
                changes,
            } => super::sysex::encode_single_note(*realtime, *device, *bank, *program, changes)
                .unwrap_or_default(),
            MidiMessage::MtsScaleOctave {
                realtime,
                device,
                channels,
                offsets,
            } => super::sysex::encode_scale_octave(*realtime, *device, *channels, offsets)
                .unwrap_or_default(),
            MidiMessage::MtsScaleOctave14 {
                realtime,
                device,
                channels,
                values,
            } => super::sysex::encode_scale_octave_14(*realtime, *device, *channels, values)
                .unwrap_or_default(),
            MidiMessage::MtsBulkRequest { device, program } => {
                super::sysex::encode_bulk_request(*device, *program)
            }
            MidiMessage::MtsBulkRequestBank {
                device,
                bank,
                program,
            } => super::sysex::encode_bulk_request_bank(*device, *bank, *program),
            MidiMessage::MtsBulkDump { device, dump } => {
                super::sysex::encode_bulk_dump(*device, dump).unwrap_or_default()
            }
            MidiMessage::MtsScaleOctaveDump { device, dump } => {
                super::sysex::encode_scale_dump(*device, dump).unwrap_or_default()
            }
            MidiMessage::SystemExclusive { data } => data.clone(),
            MidiMessage::Raw { data } => data.clone(),
        }
    }

    /// Get a human-readable description of the message
    pub fn description(&self) -> String {
        match self {
            MidiMessage::NoteOff {
                channel,
                note,
                velocity,
            } => {
                format!("Note Off: ch={}, note={}, vel={}", channel, note, velocity)
            }
            MidiMessage::NoteOn {
                channel,
                note,
                velocity,
            } => {
                format!("Note On: ch={}, note={}, vel={}", channel, note, velocity)
            }
            MidiMessage::PolyphonicAftertouch {
                channel,
                note,
                pressure,
            } => {
                format!(
                    "Poly Aftertouch: ch={}, note={}, pressure={}",
                    channel, note, pressure
                )
            }
            MidiMessage::ControlChange {
                channel,
                controller,
                value,
            } => {
                format!(
                    "Control Change: ch={}, cc={}, val={}",
                    channel, controller, value
                )
            }
            MidiMessage::ProgramChange { channel, program } => {
                format!("Program Change: ch={}, prog={}", channel, program)
            }
            MidiMessage::ChannelAftertouch { channel, pressure } => {
                format!("Channel Aftertouch: ch={}, pressure={}", channel, pressure)
            }
            MidiMessage::PitchBend { channel, value } => {
                format!("Pitch Bend: ch={}, val={}", channel, value)
            }
            MidiMessage::MtcFullFrame { device, time } => {
                format!(
                    "MTC Full Frame: dev={:#04X} {:02}:{:02}:{:02}:{:02} {:?}",
                    device, time.hours, time.minutes, time.seconds, time.frames, time.rate
                )
            }
            MidiMessage::MtcQuarterFrame { kind, value } => {
                format!("MTC Quarter Frame: {:?}={}", kind, value)
            }
            MidiMessage::Mmc { device, command } => {
                format!("MMC: dev={:#04X} {:?}", device, command)
            }
            MidiMessage::MmcResponse { device, state, data } => {
                format!(
                    "MMC Response: dev={:#04X} state={:#04X} {} bytes",
                    device,
                    state,
                    data.len()
                )
            }
            MidiMessage::IdentityRequest { device } => {
                format!("Identity Request: dev={:#04X}", device)
            }
            MidiMessage::IdentityReply {
                device,
                manufacturer,
                family,
                model,
                ..
            } => {
                format!(
                    "Identity Reply: dev={:#04X} {:?} family={:#06X} model={:#06X}",
                    device, manufacturer, family, model
                )
            }
            MidiMessage::GmSystem { device, mode } => {
                format!("GM System: dev={:#04X} {:?}", device, mode)
            }
            MidiMessage::MasterControl { device, control } => {
                format!("Master Control: dev={:#04X} {:?}", device, control)
            }
            MidiMessage::MtsSingleNote {
                device,
                program,
                changes,
                ..
            } => {
                format!(
                    "MTS Single Note: dev={:#04X} program={} {} changes",
                    device,
                    program,
                    changes.len()
                )
            }
            MidiMessage::MtsScaleOctave { device, .. } => {
                format!("MTS Scale/Octave: dev={:#04X}", device)
            }
            MidiMessage::MtsScaleOctave14 { device, .. } => {
                format!("MTS Scale/Octave 14-bit: dev={:#04X}", device)
            }
            MidiMessage::MtsBulkRequest { device, program } => {
                format!("MTS Bulk Request: dev={:#04X} program={}", device, program)
            }
            MidiMessage::MtsBulkRequestBank {
                device,
                bank,
                program,
            } => {
                format!(
                    "MTS Bulk Request: dev={:#04X} bank={} program={}",
                    device, bank, program
                )
            }
            MidiMessage::MtsBulkDump { device, dump } => {
                format!(
                    "MTS Bulk Dump: dev={:#04X} program={}",
                    device, dump.program
                )
            }
            MidiMessage::MtsScaleOctaveDump { device, dump } => {
                format!(
                    "MTS Scale/Octave Dump: dev={:#04X} program={}",
                    device, dump.program
                )
            }
            MidiMessage::SystemExclusive { data } => {
                format!("SysEx: {} bytes", data.len())
            }
            MidiMessage::System { status, len, .. } => {
                format!("System: status=0x{:02X}, data_len={}", status, len)
            }
            MidiMessage::Raw { data } => {
                format!("Raw: {} bytes", data.len())
            }
        }
    }
}

/// Parse a SysEx buffer, recognizing typed MTC full-frame, MMC command,
/// and MMC response universal messages. Anything else — including
/// malformed universal messages — stays a generic `SystemExclusive`,
/// preserving the historical leniency of SysEx parsing.
fn parse_system_exclusive(bytes: &[u8]) -> MidiMessage {
    if let Some((device, time)) = MtcTime::from_full_frame_bytes(bytes) {
        return MidiMessage::MtcFullFrame { device, time };
    }
    if let Some((device, command)) = MmcCommand::from_sysex(bytes) {
        return MidiMessage::Mmc { device, command };
    }
    if let Some(response) = MmcResponse::from_sysex(bytes) {
        return MidiMessage::MmcResponse {
            device: response.device,
            state: response.state,
            data: response.data,
        };
    }
    if let Some(device) = super::sysex::decode_identity_request(bytes) {
        return MidiMessage::IdentityRequest { device };
    }
    if let Some((device, reply)) = super::sysex::decode_identity_reply(bytes) {
        return MidiMessage::IdentityReply {
            device,
            manufacturer: reply.manufacturer,
            family: reply.family,
            model: reply.model,
            version: reply.version,
        };
    }
    if let Some((device, mode)) = super::sysex::decode_gm_system(bytes) {
        return MidiMessage::GmSystem { device, mode };
    }
    if let Some((device, control)) = super::sysex::decode_master_control(bytes) {
        return MidiMessage::MasterControl { device, control };
    }
    if let Some((realtime, device, bank, program, changes)) =
        super::sysex::decode_single_note(bytes)
    {
        return MidiMessage::MtsSingleNote {
            realtime,
            device,
            bank,
            program,
            changes,
        };
    }
    if let Some((realtime, device, channels, offsets)) =
        super::sysex::decode_scale_octave(bytes)
    {
        return MidiMessage::MtsScaleOctave {
            realtime,
            device,
            channels,
            offsets,
        };
    }
    if let Some((realtime, device, channels, values)) =
        super::sysex::decode_scale_octave_14(bytes)
    {
        return MidiMessage::MtsScaleOctave14 {
            realtime,
            device,
            channels,
            values,
        };
    }
    if let Some((device, program)) = super::sysex::decode_bulk_request(bytes) {
        return MidiMessage::MtsBulkRequest { device, program };
    }
    if let Some((device, bank, program)) = super::sysex::decode_bulk_request_bank(bytes) {
        return MidiMessage::MtsBulkRequestBank {
            device,
            bank,
            program,
        };
    }
    if let Some((device, dump)) = super::sysex::decode_bulk_dump(bytes) {
        return MidiMessage::MtsBulkDump {
            device,
            dump: Box::new(dump),
        };
    }
    if let Some((device, dump)) = super::sysex::decode_scale_dump(bytes) {
        return MidiMessage::MtsScaleOctaveDump {
            device,
            dump: Box::new(dump),
        };
    }
    MidiMessage::SystemExclusive {
        data: bytes.to_vec(),
    }
}

/// Parse an MTC quarter-frame message (`F1 <type value>`).
fn parse_quarter_frame(bytes: &[u8]) -> Result<MidiMessage> {
    let data = required_data_bytes(bytes, 1, "MTC quarter frame")?;
    let kind = MtcQuarterFrameKind::from_nibble(data[0] >> 4).ok_or_else(|| {
        MidiError::InvalidMessage(format!(
            "MTC quarter-frame type out of range: 0x{:02X}",
            data[0]
        ))
    })?;
    let value = data[0] & 0x0F;
    kind.validate_value(value)?;
    Ok(MidiMessage::MtcQuarterFrame { kind, value })
}

fn parse_system_message(bytes: &[u8]) -> Result<MidiMessage> {
    let status = bytes[0];
    let Some(data_len) = system_data_len(status) else {
        return Ok(MidiMessage::Raw {
            data: bytes.to_vec(),
        });
    };
    if bytes.len() < 1 + data_len {
        return Err(MidiError::InvalidMessage(format!(
            "System message 0x{:02X} too short",
            status
        )));
    }
    let mut data = [0u8; 2];
    data[..data_len].copy_from_slice(&bytes[1..1 + data_len]);
    validate_data_bytes(&data[..data_len], "System message")?;
    Ok(MidiMessage::System {
        status,
        data,
        len: data_len as u8,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MmcShuttleSpeed, MtcFrameRate};

    #[test]
    fn test_note_on_encoding() {
        let msg = MidiMessage::NoteOn {
            channel: 0,
            note: 60,
            velocity: 100,
        };
        let bytes = msg.to_bytes();
        assert_eq!(bytes, vec![0x90, 60, 100]);

        let decoded = MidiMessage::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn test_note_off_encoding() {
        let msg = MidiMessage::NoteOff {
            channel: 1,
            note: 64,
            velocity: 0,
        };
        let bytes = msg.to_bytes();
        assert_eq!(bytes, vec![0x81, 64, 0]);

        let decoded = MidiMessage::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn test_control_change_encoding() {
        let msg = MidiMessage::ControlChange {
            channel: 2,
            controller: 7,
            value: 127,
        };
        let bytes = msg.to_bytes();
        assert_eq!(bytes, vec![0xB2, 7, 127]);

        let decoded = MidiMessage::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn test_pitch_bend_encoding() {
        let msg = MidiMessage::PitchBend {
            channel: 0,
            value: 8192, // center
        };
        let bytes = msg.to_bytes();
        assert_eq!(bytes, vec![0xE0, 0, 64]);

        let decoded = MidiMessage::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn test_from_bytes_rejects_data_only() {
        // Data-only bytes (no status byte) must be rejected, not silently parsed as Raw.
        let result = MidiMessage::from_bytes(&[0x40, 0x7F]);
        assert!(
            result.is_err(),
            "expected error for data-only bytes, got {:?}",
            result
        );
    }

    #[test]
    fn test_from_bytes_rejects_data_only_single_byte() {
        let result = MidiMessage::from_bytes(&[0x40]);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_bytes_with_status_running() {
        let msg = MidiMessage::from_bytes_with_status(&[7, 100], Some(0xB0)).unwrap();
        assert_eq!(
            msg,
            MidiMessage::ControlChange {
                channel: 0,
                controller: 7,
                value: 100
            }
        );
    }

    #[test]
    fn test_from_bytes_with_status_no_running_errors() {
        let result = MidiMessage::from_bytes_with_status(&[7, 100], None);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_bytes_rejects_status_byte_in_channel_data() {
        let result = MidiMessage::from_bytes(&[0x90, 0x90, 60]);
        assert!(
            result.is_err(),
            "status byte in channel data must not be masked into note 16"
        );
    }

    #[test]
    fn test_from_bytes_with_status_rejects_status_byte_in_data() {
        let result = MidiMessage::from_bytes_with_status(&[0xB0, 100], Some(0x90));
        assert!(result.is_err());
    }

    #[test]
    fn test_system_realtime_is_stack_sized_message() {
        let msg = MidiMessage::from_bytes(&[0xF8]).unwrap();
        assert_eq!(
            msg,
            MidiMessage::System {
                status: 0xF8,
                data: [0, 0],
                len: 0
            }
        );
        let mut out = [0u8; 3];
        let n = msg.write_to(&mut out);
        assert_eq!(n, 1);
        assert_eq!(&out[..n], &[0xF8]);
    }

    #[test]
    fn test_system_common_validates_data_bytes() {
        // 0xF1 now parses as a typed MTC quarter frame (seconds LS nibble).
        let msg = MidiMessage::from_bytes(&[0xF1, 0x2E]).unwrap();
        assert_eq!(
            msg,
            MidiMessage::MtcQuarterFrame {
                kind: MtcQuarterFrameKind::SecondsLsb,
                value: 0x0E
            }
        );
        let mut out = [0u8; 2];
        let n = msg.write_to(&mut out);
        assert_eq!(n, 2);
        assert_eq!(&out[..n], &[0xF1, 0x2E]);

        let result = MidiMessage::from_bytes(&[0xF1, 0x80]);
        assert!(result.is_err());

        // Type 7 with the top value bit set is malformed per the MTC spec.
        let result = MidiMessage::from_bytes(&[0xF1, 0x7F]);
        assert!(result.is_err());
    }

    #[test]
    fn test_write_to_note_on() {
        let msg = MidiMessage::NoteOn {
            channel: 0,
            note: 60,
            velocity: 100,
        };
        let mut buf = [0u8; 3];
        let n = msg.write_to(&mut buf);
        assert_eq!(n, 3);
        assert_eq!(buf, [0x90, 60, 100]);
    }

    #[test]
    fn test_note_on_zero_velocity_becomes_note_off() {
        let bytes = vec![0x90, 60, 0]; // Note On with velocity 0
        let decoded = MidiMessage::from_bytes(&bytes).unwrap();
        assert_eq!(
            decoded,
            MidiMessage::NoteOff {
                channel: 0,
                note: 60,
                velocity: 0
            }
        );
    }

    #[test]
    fn test_note_off_round_trip_max_values() {
        let msg = MidiMessage::NoteOff {
            channel: 15,
            note: 127,
            velocity: 127,
        };
        let bytes = msg.to_bytes();
        assert_eq!(bytes, vec![0x8F, 127, 127]);
        assert_eq!(MidiMessage::from_bytes(&bytes).unwrap(), msg);
    }

    #[test]
    fn test_note_on_round_trip_min_values() {
        let msg = MidiMessage::NoteOn {
            channel: 0,
            note: 0,
            velocity: 1,
        };
        let bytes = msg.to_bytes();
        assert_eq!(bytes, vec![0x90, 0, 1]);
        assert_eq!(MidiMessage::from_bytes(&bytes).unwrap(), msg);
    }

    #[test]
    fn test_polyphonic_aftertouch_round_trip() {
        let msg = MidiMessage::PolyphonicAftertouch {
            channel: 7,
            note: 64,
            pressure: 32,
        };
        let bytes = msg.to_bytes();
        assert_eq!(bytes, vec![0xA7, 64, 32]);
        assert_eq!(MidiMessage::from_bytes(&bytes).unwrap(), msg);
    }

    #[test]
    fn test_program_change_round_trip() {
        let msg = MidiMessage::ProgramChange {
            channel: 3,
            program: 127,
        };
        let bytes = msg.to_bytes();
        assert_eq!(bytes, vec![0xC3, 127]);
        assert_eq!(MidiMessage::from_bytes(&bytes).unwrap(), msg);
    }

    #[test]
    fn test_channel_aftertouch_round_trip() {
        let msg = MidiMessage::ChannelAftertouch {
            channel: 11,
            pressure: 127,
        };
        let bytes = msg.to_bytes();
        assert_eq!(bytes, vec![0xDB, 127]);
        assert_eq!(MidiMessage::from_bytes(&bytes).unwrap(), msg);
    }

    #[test]
    fn test_pitch_bend_round_trip_extremes() {
        for &(value, msb, lsb) in &[
            (0u16, 0u8, 0u8),
            (8192u16, 64u8, 0u8),
            (16383u16, 127u8, 127u8),
        ] {
            let msg = MidiMessage::PitchBend { channel: 0, value };
            let bytes = msg.to_bytes();
            assert_eq!(bytes, vec![0xE0, lsb, msb], "value={}", value);
            assert_eq!(MidiMessage::from_bytes(&bytes).unwrap(), msg);
        }
    }

    #[test]
    fn test_system_exclusive_round_trip() {
        let msg = MidiMessage::SystemExclusive {
            data: vec![0xF0, 0x7D, 0x10, 0xF7],
        };
        let bytes = msg.to_bytes();
        assert_eq!(bytes, vec![0xF0, 0x7D, 0x10, 0xF7]);
        assert_eq!(MidiMessage::from_bytes(&bytes).unwrap(), msg);
    }

    #[test]
    fn test_system_common_round_trip() {
        let msg = MidiMessage::System {
            status: 0xF2,
            data: [0x7F, 0x7F],
            len: 2,
        };
        let bytes = msg.to_bytes();
        assert_eq!(bytes, vec![0xF2, 0x7F, 0x7F]);
        assert_eq!(MidiMessage::from_bytes(&bytes).unwrap(), msg);
    }

    #[test]
    fn test_raw_undefined_system_message_round_trip() {
        let msg = MidiMessage::Raw { data: vec![0xF4] };
        let bytes = msg.to_bytes();
        assert_eq!(bytes, vec![0xF4]);
        let decoded = MidiMessage::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn test_write_to_buffer_size_edge_cases() {
        let msg = MidiMessage::NoteOn {
            channel: 0,
            note: 60,
            velocity: 100,
        };
        let mut buf = [0u8; 2];
        let n = msg.write_to(&mut buf);
        assert_eq!(n, 3);
        // Buffer too small must not be written past its length.
        assert_eq!(buf, [0u8; 2]);

        let sys = MidiMessage::SystemExclusive {
            data: vec![0xF0, 0x01, 0x02],
        };
        let mut buf = [0u8; 2];
        let n = sys.write_to(&mut buf);
        assert_eq!(n, 3);
        assert_eq!(buf, [0u8; 2]);

        let rt = MidiMessage::System {
            status: 0xF8,
            data: [0, 0],
            len: 0,
        };
        let mut buf = [0u8; 0];
        let n = rt.write_to(&mut buf);
        assert_eq!(n, 1);
    }

    #[test]
    fn test_description_covers_all_variants() {
        let msgs: Vec<MidiMessage> = vec![
            MidiMessage::NoteOff {
                channel: 0,
                note: 60,
                velocity: 0,
            },
            MidiMessage::NoteOn {
                channel: 0,
                note: 60,
                velocity: 100,
            },
            MidiMessage::PolyphonicAftertouch {
                channel: 0,
                note: 60,
                pressure: 50,
            },
            MidiMessage::ControlChange {
                channel: 0,
                controller: 7,
                value: 100,
            },
            MidiMessage::ProgramChange {
                channel: 0,
                program: 5,
            },
            MidiMessage::ChannelAftertouch {
                channel: 0,
                pressure: 80,
            },
            MidiMessage::PitchBend {
                channel: 0,
                value: 8192,
            },
            MidiMessage::SystemExclusive { data: vec![0xF0] },
            MidiMessage::System {
                status: 0xF8,
                data: [0, 0],
                len: 0,
            },
            MidiMessage::MtcFullFrame {
                device: 0x7F,
                time: MtcTime::new(MtcFrameRate::Fps30, 1, 2, 3, 4).unwrap(),
            },
            MidiMessage::MtcQuarterFrame {
                kind: MtcQuarterFrameKind::FrameLsb,
                value: 5,
            },
            MidiMessage::Mmc {
                device: 0x7F,
                command: MmcCommand::Stop,
            },
            MidiMessage::MmcResponse {
                device: 0x7F,
                state: 0x01,
                data: vec![],
            },
            MidiMessage::IdentityRequest { device: 0x7F },
            MidiMessage::IdentityReply {
                device: 0x7F,
                manufacturer: ManufacturerId::One(0x41),
                family: 1,
                model: 2,
                version: [3, 0, 0, 0],
            },
            MidiMessage::GmSystem {
                device: 0x7F,
                mode: GmMode::Gm1On,
            },
            MidiMessage::MasterControl {
                device: 0x7F,
                control: MasterControl::Volume(0x2000),
            },
            MidiMessage::MtsSingleNote {
                realtime: Realtime::Yes,
                device: 0x7F,
                bank: None,
                program: 0,
                changes: vec![],
            },
            MidiMessage::MtsScaleOctave {
                realtime: Realtime::No,
                device: 0x7F,
                channels: ScaleChannels { ff: 0, gg: 0, hh: 1 },
                offsets: [0; 12],
            },
            MidiMessage::MtsScaleOctave14 {
                realtime: Realtime::No,
                device: 0x7F,
                channels: ScaleChannels { ff: 0, gg: 0, hh: 1 },
                values: [8192; 12],
            },
            MidiMessage::MtsBulkRequest {
                device: 0x7F,
                program: 0,
            },
            MidiMessage::MtsBulkRequestBank {
                device: 0x7F,
                bank: 0,
                program: 0,
            },
            MidiMessage::MtsBulkDump {
                device: 0x7F,
                dump: Box::new(BulkTuningDump {
                    program: 0,
                    name: [b' '; 16],
                    notes: vec![(69, 0); 128],
                    checksum: 0,
                }),
            },
            MidiMessage::MtsScaleOctaveDump {
                device: 0x7F,
                dump: Box::new(ScaleOctaveDump {
                    two_byte: false,
                    bank: 0,
                    program: 0,
                    name: [b' '; 16],
                    data: vec![0x40; 12],
                    checksum: 0,
                }),
            },
            MidiMessage::Raw { data: vec![0xF4] },
        ];
        for msg in &msgs {
            let d = msg.description();
            assert!(!d.is_empty());
        }
    }

    #[test]
    fn test_from_bytes_empty() {
        let result = MidiMessage::from_bytes(&[]);
        assert!(matches!(result, Err(MidiError::InvalidMessage(_))));
    }

    #[test]
    fn test_from_bytes_too_short_channel_messages() {
        assert!(MidiMessage::from_bytes(&[0x90, 60]).is_err());
        assert!(MidiMessage::from_bytes(&[0x80, 60]).is_err());
        assert!(MidiMessage::from_bytes(&[0xA0, 60]).is_err());
        assert!(MidiMessage::from_bytes(&[0xB0, 7]).is_err());
        assert!(MidiMessage::from_bytes(&[0xE0, 0]).is_err());
    }

    #[test]
    fn test_from_bytes_too_short_program_and_aftertouch() {
        assert!(MidiMessage::from_bytes(&[0xC0]).is_err());
        assert!(MidiMessage::from_bytes(&[0xD0]).is_err());
    }

    #[test]
    fn test_from_bytes_high_bit_in_data_byte() {
        assert!(MidiMessage::from_bytes(&[0x90, 60, 0x80]).is_err());
        assert!(MidiMessage::from_bytes(&[0xC0, 0x80]).is_err());
    }

    #[test]
    fn test_from_bytes_with_status_invalid_running() {
        let result = MidiMessage::from_bytes_with_status(&[60, 100], Some(0x40));
        assert!(result.is_err());
    }

    #[test]
    fn test_from_bytes_with_status_non_channel_voice() {
        let result = MidiMessage::from_bytes_with_status(&[60, 100], Some(0xF8));
        assert!(result.is_err());
    }

    #[test]
    fn test_from_bytes_with_status_too_short() {
        let result = MidiMessage::from_bytes_with_status(&[60], Some(0x90));
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_system_message_directly() {
        let m = parse_system_message(&[0xF8]).unwrap();
        assert_eq!(
            m,
            MidiMessage::System {
                status: 0xF8,
                data: [0, 0],
                len: 0
            }
        );
        let result = parse_system_message(&[0xF2, 0x00]);
        assert!(result.is_err());
    }

    #[test]
    fn test_polyphonic_aftertouch_too_short() {
        assert!(MidiMessage::from_bytes(&[0xA0, 60]).is_err());
    }

    #[test]
    fn test_polyphonic_aftertouch_high_bit_data() {
        assert!(MidiMessage::from_bytes(&[0xA0, 60, 0x80]).is_err());
    }

    #[test]
    fn test_program_change_high_bit_data() {
        assert!(MidiMessage::from_bytes(&[0xC0, 0x80]).is_err());
    }

    #[test]
    fn test_channel_aftertouch_high_bit_data() {
        assert!(MidiMessage::from_bytes(&[0xD0, 0x80]).is_err());
    }

    #[test]
    fn test_note_off_too_short() {
        assert!(MidiMessage::from_bytes(&[0x80, 60]).is_err());
    }

    #[test]
    fn test_note_off_high_bit_data() {
        assert!(MidiMessage::from_bytes(&[0x80, 60, 0x80]).is_err());
    }

    #[test]
    fn test_control_change_high_bit_data() {
        assert!(MidiMessage::from_bytes(&[0xB0, 7, 0x80]).is_err());
    }

    #[test]
    fn test_pitch_bend_high_bit_data() {
        assert!(MidiMessage::from_bytes(&[0xE0, 0, 0x80]).is_err());
    }

    #[test]
    fn test_song_position_pointer_valid() {
        let msg = MidiMessage::from_bytes(&[0xF2, 0x7F, 0x7F]).unwrap();
        assert_eq!(
            msg,
            MidiMessage::System {
                status: 0xF2,
                data: [0x7F, 0x7F],
                len: 2
            }
        );
    }

    #[test]
    fn test_song_position_pointer_too_short() {
        assert!(MidiMessage::from_bytes(&[0xF2, 0x7F]).is_err());
    }

    #[test]
    fn test_song_select_valid() {
        let msg = MidiMessage::from_bytes(&[0xF3, 0x7F]).unwrap();
        assert_eq!(
            msg,
            MidiMessage::System {
                status: 0xF3,
                data: [0x7F, 0],
                len: 1
            }
        );
    }

    #[test]
    fn test_song_select_too_short() {
        assert!(MidiMessage::from_bytes(&[0xF3]).is_err());
    }

    #[test]
    fn test_system_realtime_start() {
        let msg = MidiMessage::from_bytes(&[0xFA]).unwrap();
        assert_eq!(
            msg,
            MidiMessage::System {
                status: 0xFA,
                data: [0, 0],
                len: 0
            }
        );
        let mut out = [0u8; 3];
        let n = msg.write_to(&mut out);
        assert_eq!(n, 1);
        assert_eq!(&out[..n], &[0xFA]);
    }

    #[test]
    fn test_mtc_full_frame_parses_from_sysex() {
        let time = MtcTime::new(MtcFrameRate::Fps25, 8, 15, 45, 20).unwrap();
        let msg = MidiMessage::MtcFullFrame {
            device: 0x10,
            time,
        };
        let bytes = msg.to_bytes();
        // 25 fps -> rate bits 01, hours 8 -> 0x28.
        assert_eq!(bytes, vec![0xF0, 0x7F, 0x10, 0x01, 0x01, 0x28, 15, 45, 20, 0xF7]);
        assert_eq!(MidiMessage::from_bytes(&bytes).unwrap(), msg);
        let mut out = [0u8; 10];
        let n = msg.write_to(&mut out);
        assert_eq!(n, 10);
        assert_eq!(&out[..n], &bytes[..]);
    }

    #[test]
    fn test_malformed_universal_sysex_stays_generic() {
        // Truncated MTC full frame: not typed, still generic SysEx.
        let bytes = vec![0xF0, 0x7F, 0x7F, 0x01, 0x01, 0x20, 0xF7];
        assert_eq!(
            MidiMessage::from_bytes(&bytes).unwrap(),
            MidiMessage::SystemExclusive { data: bytes.clone() }
        );
        // Drop-frame gap time in a full frame: well-shaped but invalid.
        let bytes = vec![0xF0, 0x7F, 0x7F, 0x01, 0x01, 0x41, 0x01, 0x00, 0x00, 0xF7];
        assert_eq!(
            MidiMessage::from_bytes(&bytes).unwrap(),
            MidiMessage::SystemExclusive { data: bytes }
        );
    }

    #[test]
    fn test_mmc_parses_from_sysex() {
        let msg = MidiMessage::Mmc {
            device: 0x7F,
            command: MmcCommand::Play,
        };
        let bytes = msg.to_bytes();
        assert_eq!(bytes, vec![0xF0, 0x7F, 0x7F, 0x06, 0x02, 0xF7]);
        assert_eq!(MidiMessage::from_bytes(&bytes).unwrap(), msg);

        let locate = MidiMessage::Mmc {
            device: 0x01,
            command: MmcCommand::Locate {
                time: MtcTime::new(MtcFrameRate::Fps24, 0, 0, 5, 10).unwrap(),
                subframes: 0,
            },
        };
        let bytes = locate.to_bytes();
        assert_eq!(MidiMessage::from_bytes(&bytes).unwrap(), locate);
    }

    #[test]
    fn test_mmc_response_parses_from_sysex() {
        let msg = MidiMessage::MmcResponse {
            device: 0x01,
            state: 0x03,
            data: vec![0x01, 0x02],
        };
        let bytes = msg.to_bytes();
        assert_eq!(bytes, vec![0xF0, 0x7F, 0x01, 0x07, 0x03, 0x01, 0x02, 0xF7]);
        assert_eq!(MidiMessage::from_bytes(&bytes).unwrap(), msg);
    }

    #[test]
    fn test_typed_messages_serde_round_trip() {
        for msg in [
            MidiMessage::MtcFullFrame {
                device: 0x7F,
                time: MtcTime::new(MtcFrameRate::Fps29_97Drop, 1, 10, 0, 2).unwrap(),
            },
            MidiMessage::MtcQuarterFrame {
                kind: MtcQuarterFrameKind::HoursMsbRate,
                value: 6,
            },
            MidiMessage::Mmc {
                device: 0x7F,
                command: MmcCommand::Shuttle {
                    speed: MmcShuttleSpeed { sh: 0x41, sm: 0, sl: 0 },
                },
            },
        ] {
            let json = serde_json::to_string(&msg).unwrap();
            assert_eq!(serde_json::from_str::<MidiMessage>(&json).unwrap(), msg);
        }
    }

    #[test]
    fn test_from_bytes_with_status_delegates_when_status_present() {
        // When bytes already start with a status byte, from_bytes_with_status
        // should delegate directly to from_bytes regardless of running_status.
        let msg = MidiMessage::from_bytes_with_status(&[0x90, 60, 100], Some(0xB0)).unwrap();
        assert_eq!(
            msg,
            MidiMessage::NoteOn {
                channel: 0,
                note: 60,
                velocity: 100
            }
        );
    }
}
