// Rust guideline compliant 2026-02-21
use crate::message::MidiMessage;

/// A MIDI input message with timing and connection provenance.
///
/// Timestamps are microseconds from the MIDI backend's clock for this
/// connection. They are not audio-frame or wall-clock timestamps.
#[derive(Debug, Clone, PartialEq)]
pub struct TimedMidiMessage {
    /// Time of the first packet for SysEx, or the callback time otherwise.
    pub timestamp_micros: u64,
    /// Callback time that completed this message.
    pub received_at_micros: u64,
    /// Input port index used when the connection was opened.
    pub port_index: usize,
    /// Monotonic identity assigned to each input connection from a manager.
    pub connection_id: u64,
    /// Zero-based callback sequence within this connection.
    pub sequence: u64,
    /// Parsed MIDI payload.
    pub message: MidiMessage,
}
