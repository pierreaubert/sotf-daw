//! MIDI message types and encoding/decoding

mod midi_message;
mod misc;
pub mod mmc;
pub mod mtc;
pub mod sysex;
pub mod ump;

pub use midi_message::*;
pub use mmc::{MmcCommand, MmcField, MmcResponse, MmcShuttleSpeed};
pub use mtc::{MtcFrameRate, MtcQuarterFrameAssembler, MtcQuarterFrameKind, MtcTime};
pub use sysex::{
    BulkTuningDump, GmMode, IdentityReply, ManufacturerId, MasterControl, Realtime, ScaleChannels,
    ScaleOctaveDump, SingleNoteChange, TuningFrequency,
};
pub use ump::{
    GROUP_MAX, Midi2Voice, NoteAttribute, SYSEX7_CAPACITY, SYSEX8_CAPACITY, SysexStatus,
    UmpMessage, UmpUtility,
};
