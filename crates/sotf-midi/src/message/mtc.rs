//! MIDI Time Code: frame rates, full-frame messages, and quarter-frame handling.
//!
//! Wire formats follow the MIDI 1.0 Detailed Specification (MTC, RP-004):
//! - Full frame: `F0 7F <device> 01 01 <hr> <mn> <sc> <fr> F7`, where the
//!   `hr` byte carries the frame rate in bits 6-5 (00 = 24 fps, 01 = 25,
//!   10 = 29.97 drop-frame, 11 = 30) and the hours in bits 4-0.
//! - Quarter frame: `F1 <ttt vvvv>`, one nibble of the running time per
//!   message; eight messages (types 0-7) assemble a complete time.

use crate::error::{MidiError, Result};
use serde::{Deserialize, Serialize};

/// MTC frame rates. The 29.97 drop-frame rate skips frame numbers 00 and 01
/// at the start of every minute except every tenth minute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MtcFrameRate {
    /// 24 frames per second (film).
    Fps24,
    /// 25 frames per second (PAL/SECAM video).
    Fps25,
    /// 29.97 frames per second, drop-frame (NTSC video).
    Fps29_97Drop,
    /// 30 frames per second (non-drop).
    Fps30,
}

impl MtcFrameRate {
    /// Rate bits as stored in bits 6-5 of the full-frame `hr` byte.
    pub fn rate_bits(self) -> u8 {
        match self {
            Self::Fps24 => 0,
            Self::Fps25 => 1,
            Self::Fps29_97Drop => 2,
            Self::Fps30 => 3,
        }
    }

    /// Decode rate bits from a full-frame `hr` byte.
    pub fn from_rate_bits(bits: u8) -> Option<Self> {
        match bits & 0x03 {
            0 => Some(Self::Fps24),
            1 => Some(Self::Fps25),
            2 => Some(Self::Fps29_97Drop),
            3 => Some(Self::Fps30),
            _ => None,
        }
    }

    /// Highest valid frame number for this rate.
    pub fn max_frames(self) -> u8 {
        match self {
            Self::Fps24 => 23,
            Self::Fps25 => 24,
            Self::Fps29_97Drop | Self::Fps30 => 29,
        }
    }

    /// Whether this rate drops frame numbers 00/01 outside tenth minutes.
    pub fn is_drop_frame(self) -> bool {
        matches!(self, Self::Fps29_97Drop)
    }
}

/// A validated MTC time value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MtcTime {
    /// Frame rate governing frame ranges and drop-frame rules.
    pub rate: MtcFrameRate,
    /// Hours, 0-23.
    pub hours: u8,
    /// Minutes, 0-59.
    pub minutes: u8,
    /// Seconds, 0-59.
    pub seconds: u8,
    /// Frame number within the second, up to the rate maximum.
    pub frames: u8,
}

impl MtcTime {
    /// Build a time value, enforcing ranges and drop-frame rules.
    pub fn new(
        rate: MtcFrameRate,
        hours: u8,
        minutes: u8,
        seconds: u8,
        frames: u8,
    ) -> Result<Self> {
        let time = Self {
            rate,
            hours,
            minutes,
            seconds,
            frames,
        };
        time.validate()?;
        Ok(time)
    }

    /// Check ranges and drop-frame rules for the current field values.
    pub fn validate(&self) -> Result<()> {
        if self.hours > 23 {
            return Err(MidiError::InvalidMessage(format!(
                "MTC hours out of range: {}",
                self.hours
            )));
        }
        if self.minutes > 59 {
            return Err(MidiError::InvalidMessage(format!(
                "MTC minutes out of range: {}",
                self.minutes
            )));
        }
        if self.seconds > 59 {
            return Err(MidiError::InvalidMessage(format!(
                "MTC seconds out of range: {}",
                self.seconds
            )));
        }
        if self.frames > self.rate.max_frames() {
            return Err(MidiError::InvalidMessage(format!(
                "MTC frame {} out of range for {:?}",
                self.frames, self.rate
            )));
        }
        // Drop-frame: frame numbers 00 and 01 do not exist at the start of
        // a minute, except at every tenth minute (00, 10, 20, ... 50).
        if self.rate.is_drop_frame()
            && !self.minutes.is_multiple_of(10)
            && self.seconds == 0
            && self.frames <= 1
        {
            return Err(MidiError::InvalidMessage(format!(
                "MTC drop-frame time {:02}:{:02}:{:02}:{:02} does not exist",
                self.hours, self.minutes, self.seconds, self.frames
            )));
        }
        Ok(())
    }

    /// Encode as a 10-byte full-frame SysEx message for `device`
    /// (`0x7F` addresses all devices).
    pub fn to_full_frame_bytes(&self, device: u8) -> [u8; 10] {
        let hr = (self.rate.rate_bits() << 5) | (self.hours & 0x1F);
        [
            0xF0,
            0x7F,
            device & 0x7F,
            0x01,
            0x01,
            hr,
            self.minutes,
            self.seconds,
            self.frames,
            0xF7,
        ]
    }

    /// Decode a full-frame message, returning the device id and time.
    /// Returns `None` when the bytes are not a well-formed MTC full frame;
    /// callers fall back to generic SysEx handling in that case.
    pub fn from_full_frame_bytes(bytes: &[u8]) -> Option<(u8, Self)> {
        if bytes.len() != 10
            || bytes[0] != 0xF0
            || bytes[1] != 0x7F
            || bytes[3] != 0x01
            || bytes[4] != 0x01
            || bytes[9] != 0xF7
        {
            return None;
        }
        if bytes[2..9].iter().any(|b| b & 0x80 != 0) {
            return None;
        }
        let hr = bytes[5];
        if hr & 0x80 != 0 {
            return None;
        }
        let rate = MtcFrameRate::from_rate_bits(hr >> 5)?;
        let time = MtcTime {
            rate,
            hours: hr & 0x1F,
            minutes: bytes[6],
            seconds: bytes[7],
            frames: bytes[8],
        };
        time.validate().ok()?;
        Some((bytes[2], time))
    }
}

/// Quarter-frame message types: which nibble of the running time travels
/// in this message. Types arrive at four times the frame rate; types 0-7
/// in order complete one time value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MtcQuarterFrameKind {
    /// Frame number, least significant nibble.
    FrameLsb = 0,
    /// Frame number, most significant bit.
    FrameMsb = 1,
    /// Seconds, least significant nibble.
    SecondsLsb = 2,
    /// Seconds, most significant nibble (0-5).
    SecondsMsb = 3,
    /// Minutes, least significant nibble.
    MinutesLsb = 4,
    /// Minutes, most significant nibble (0-5).
    MinutesMsb = 5,
    /// Hours, least significant nibble.
    HoursLsb = 6,
    /// Hours most significant bit plus frame rate.
    HoursMsbRate = 7,
}

impl MtcQuarterFrameKind {
    /// Decode the message-type nibble (high nibble of the QF data byte).
    pub fn from_nibble(nibble: u8) -> Option<Self> {
        match nibble & 0x07 {
            0 => Some(Self::FrameLsb),
            1 => Some(Self::FrameMsb),
            2 => Some(Self::SecondsLsb),
            3 => Some(Self::SecondsMsb),
            4 => Some(Self::MinutesLsb),
            5 => Some(Self::MinutesMsb),
            6 => Some(Self::HoursLsb),
            7 => Some(Self::HoursMsbRate),
            _ => None,
        }
    }

    /// Check a value nibble against the range this message type allows.
    /// The top bit of every data byte is already known clear.
    pub fn validate_value(self, value: u8) -> Result<()> {
        if value > 0x0F {
            return Err(MidiError::InvalidMessage(format!(
                "MTC quarter-frame value out of range: 0x{:02X}",
                value
            )));
        }
        let ok = match self {
            // Single bits / BCD tens digits / rate field with a zero top bit.
            Self::FrameMsb => value <= 1,
            Self::SecondsMsb | Self::MinutesMsb => value <= 5,
            Self::HoursMsbRate => value <= 7,
            Self::FrameLsb | Self::SecondsLsb | Self::MinutesLsb | Self::HoursLsb => true,
        };
        if !ok {
            return Err(MidiError::InvalidMessage(format!(
                "MTC quarter-frame value {} out of range for {:?}",
                value, self
            )));
        }
        Ok(())
    }

    /// Encode a quarter-frame data byte from this kind and value nibble.
    pub fn to_byte(self, value: u8) -> u8 {
        ((self as u8) << 4) | (value & 0x0F)
    }
}

/// Assembles running time from quarter-frame messages.
///
/// Pieces are stored by type, so out-of-order delivery still assembles;
/// a repeated type simply overwrites its nibble. `time()` returns the
/// assembled time once all eight pieces are present and valid.
#[derive(Debug, Clone, Default)]
pub struct MtcQuarterFrameAssembler {
    pieces: [Option<u8>; 8],
}

impl MtcQuarterFrameAssembler {
    /// Create an empty assembler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one quarter-frame piece. Value ranges are checked per type.
    pub fn push(&mut self, kind: MtcQuarterFrameKind, value: u8) -> Result<()> {
        kind.validate_value(value)?;
        self.pieces[kind as usize] = Some(value);
        Ok(())
    }

    /// Drop all collected pieces.
    pub fn reset(&mut self) {
        self.pieces = [None; 8];
    }

    /// Whether all eight pieces have arrived at least once.
    pub fn is_complete(&self) -> bool {
        self.pieces.iter().all(|piece| piece.is_some())
    }

    /// Assemble the running time, or `None` when pieces are missing or
    /// the assembled value fails validation (e.g. a drop-frame gap).
    pub fn time(&self) -> Option<MtcTime> {
        let get = |kind: MtcQuarterFrameKind| -> Option<u8> { self.pieces[kind as usize] };

        let frames = get(MtcQuarterFrameKind::FrameLsb)? | (get(MtcQuarterFrameKind::FrameMsb)? << 4);
        let seconds =
            get(MtcQuarterFrameKind::SecondsLsb)? | (get(MtcQuarterFrameKind::SecondsMsb)? << 4);
        let minutes =
            get(MtcQuarterFrameKind::MinutesLsb)? | (get(MtcQuarterFrameKind::MinutesMsb)? << 4);
        let rate_and_hour = get(MtcQuarterFrameKind::HoursMsbRate)?;
        let hours = get(MtcQuarterFrameKind::HoursLsb)? | ((rate_and_hour & 0x01) << 4);
        let rate = MtcFrameRate::from_rate_bits(rate_and_hour >> 1)?;

        MtcTime::new(rate, hours, minutes, seconds, frames).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_bits_round_trip() {
        for (rate, bits, max) in [
            (MtcFrameRate::Fps24, 0u8, 23u8),
            (MtcFrameRate::Fps25, 1u8, 24u8),
            (MtcFrameRate::Fps29_97Drop, 2u8, 29u8),
            (MtcFrameRate::Fps30, 3u8, 29u8),
        ] {
            assert_eq!(rate.rate_bits(), bits);
            assert_eq!(MtcFrameRate::from_rate_bits(bits), Some(rate));
            assert_eq!(rate.max_frames(), max);
        }
        assert!(MtcFrameRate::Fps29_97Drop.is_drop_frame());
        assert!(!MtcFrameRate::Fps30.is_drop_frame());
    }

    #[test]
    fn full_frame_round_trips_all_rates() {
        for rate in [
            MtcFrameRate::Fps24,
            MtcFrameRate::Fps25,
            MtcFrameRate::Fps29_97Drop,
            MtcFrameRate::Fps30,
        ] {
            let time = MtcTime::new(rate, 10, 20, 30, 12).unwrap();
            let bytes = time.to_full_frame_bytes(0x7F);
            assert_eq!(&bytes[..4], &[0xF0, 0x7F, 0x7F, 0x01]);
            assert_eq!(bytes[9], 0xF7);
            let (device, decoded) = MtcTime::from_full_frame_bytes(&bytes).unwrap();
            assert_eq!(device, 0x7F);
            assert_eq!(decoded, time);
        }
    }

    #[test]
    fn full_frame_rejects_wrong_shape() {
        assert!(MtcTime::from_full_frame_bytes(&[0xF0, 0x7F]).is_none());
        // Wrong sub-ids.
        assert!(
            MtcTime::from_full_frame_bytes(&[
                0xF0, 0x7F, 0x7F, 0x01, 0x02, 0x10, 0x20, 0x30, 0x0C, 0xF7
            ])
            .is_none()
        );
        // High bit set in a data byte.
        assert!(
            MtcTime::from_full_frame_bytes(&[
                0xF0, 0x7F, 0x7F, 0x01, 0x01, 0x90, 0x20, 0x30, 0x0C, 0xF7
            ])
            .is_none()
        );
    }

    #[test]
    fn drop_frame_gap_rejected_at_minute_start() {
        // 01:00:00:00 does not exist in drop-frame ...
        assert!(MtcTime::new(MtcFrameRate::Fps29_97Drop, 1, 1, 0, 0).is_err());
        assert!(MtcTime::new(MtcFrameRate::Fps29_97Drop, 1, 1, 0, 1).is_err());
        // ... but exists on tenth minutes and away from second zero ...
        assert!(MtcTime::new(MtcFrameRate::Fps29_97Drop, 1, 10, 0, 0).is_ok());
        assert!(MtcTime::new(MtcFrameRate::Fps29_97Drop, 1, 1, 0, 2).is_ok());
        assert!(MtcTime::new(MtcFrameRate::Fps29_97Drop, 1, 1, 1, 0).is_ok());
        // ... and non-drop 30 fps has no gaps at all.
        assert!(MtcTime::new(MtcFrameRate::Fps30, 1, 1, 0, 0).is_ok());
    }

    #[test]
    fn quarter_frame_value_ranges_enforced() {
        assert!(MtcQuarterFrameKind::FrameMsb.validate_value(1).is_ok());
        assert!(MtcQuarterFrameKind::FrameMsb.validate_value(2).is_err());
        assert!(MtcQuarterFrameKind::SecondsMsb.validate_value(5).is_ok());
        assert!(MtcQuarterFrameKind::SecondsMsb.validate_value(6).is_err());
        assert!(MtcQuarterFrameKind::HoursMsbRate.validate_value(7).is_ok());
        assert!(MtcQuarterFrameKind::HoursMsbRate.validate_value(8).is_err());
        assert!(MtcQuarterFrameKind::FrameLsb.validate_value(15).is_ok());
    }

    #[test]
    fn assembler_collects_ordered_and_unordered_pieces() {
        // 10:20:30:12 at 30 fps -> nibbles per type 0..7.
        let pieces = [(0u8, 0x2u8), (1, 0x1), (2, 0xE), (3, 0x1), (4, 0x4), (5, 0x1), (6, 0xA), (7, 0x6)];
        for order in [true, false] {
            let mut assembler = MtcQuarterFrameAssembler::new();
            assert_eq!(assembler.time(), None);
            let mut sequence: Vec<_> = pieces.to_vec();
            if !order {
                sequence.reverse();
            }
            for (index, (kind, value)) in sequence.iter().enumerate() {
                assembler
                    .push(MtcQuarterFrameKind::from_nibble(*kind).unwrap(), *value)
                    .unwrap();
                assert_eq!(assembler.time().is_some(), index == sequence.len() - 1);
            }
            let time = assembler.time().unwrap();
            assert_eq!(
                time,
                MtcTime::new(MtcFrameRate::Fps30, 10, 20, 30, 18).unwrap()
            );
        }
    }

    #[test]
    fn assembler_rejects_invalid_time_and_resets() {
        let mut assembler = MtcQuarterFrameAssembler::new();
        // 29.97 drop-frame 01:01:00:00 — a gap time.
        for (kind, value) in [(0u8, 0u8), (1, 0), (2, 0), (3, 0), (4, 1), (5, 0), (6, 1), (7, 0x4)] {
            assembler
                .push(MtcQuarterFrameKind::from_nibble(kind).unwrap(), value)
                .unwrap();
        }
        assert!(assembler.is_complete());
        assert_eq!(assembler.time(), None);
        assembler.reset();
        assert!(!assembler.is_complete());
        assert_eq!(assembler.time(), None);
    }
}
