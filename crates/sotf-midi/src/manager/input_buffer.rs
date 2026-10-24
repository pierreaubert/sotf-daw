use super::misc::is_realtime_status;
use crate::error::{MidiError, Result};
use crate::input_event::TimedMidiMessage;
use crate::message::MidiMessage;

pub(super) const MAX_SYSEX_BYTES: usize = 1024 * 1024;

/// Per-callback reusable buffer for the MIDI input thread. Channel-voice messages
/// are at most 3 bytes; SysEx falls back to heap. Held inside the midir callback
/// closure (via midir's user-data parameter) so the same allocation is reused on
/// every dispatch instead of allocating a fresh `Vec<u8>` per message.
pub(super) struct InputBuffer {
    /// Stack-sized inline buffer (covers all channel-voice messages).
    pub(super) inline: [u8; 3],
    /// Heap fallback for SysEx and other multi-byte system messages.
    pub(super) heap: Vec<u8>,
    /// In-progress multi-packet SysEx payload.
    pub(super) sysex: Vec<u8>,
    /// Ignore the remainder of a rejected SysEx without retaining it.
    discarding_sysex: bool,
    discarded_sysex_count: u64,
    sysex_started_at_micros: Option<u64>,
    next_sequence: u64,
}

impl InputBuffer {
    pub(super) fn new() -> Self {
        Self {
            inline: [0; 3],
            heap: Vec::with_capacity(64),
            sysex: Vec::with_capacity(256),
            discarding_sysex: false,
            discarded_sysex_count: 0,
            sysex_started_at_micros: None,
            next_sequence: 0,
        }
    }

    pub(super) fn discarded_sysex_count(&self) -> u64 {
        self.discarded_sysex_count
    }

    pub(super) fn parse_timed_message(
        &mut self,
        timestamp_micros: u64,
        port_index: usize,
        connection_id: u64,
        bytes: &[u8],
    ) -> Option<Result<TimedMidiMessage>> {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        if bytes.first() == Some(&0xF0) {
            self.sysex_started_at_micros = Some(timestamp_micros);
        }
        let parsed = self.parse_message(bytes)?;
        Some(parsed.map(|message| {
            let started_at = if matches!(message, MidiMessage::SystemExclusive { .. }) {
                self.sysex_started_at_micros
                    .take()
                    .unwrap_or(timestamp_micros)
            } else {
                timestamp_micros
            };
            TimedMidiMessage {
                timestamp_micros: started_at,
                received_at_micros: timestamp_micros,
                port_index,
                connection_id,
                sequence,
                message,
            }
        }))
    }

    pub(super) fn parse_message(&mut self, message: &[u8]) -> Option<Result<MidiMessage>> {
        if message.is_empty() {
            return Some(MidiMessage::from_bytes(message));
        }

        if self.discarding_sysex {
            if message.len() == 1 && is_realtime_status(message[0]) {
                return Some(MidiMessage::from_bytes(message));
            }
            if message[0] < 0x80 {
                if message.last() == Some(&0xF7) {
                    self.discarding_sysex = false;
                }
                return None;
            }
            self.discarding_sysex = false;
            if message[0] == 0xF7 {
                return None;
            }
        }

        if !self.sysex.is_empty() {
            if message.len() == 1 && is_realtime_status(message[0]) {
                return Some(MidiMessage::from_bytes(message));
            }
            if message[0] == 0xF0 {
                self.sysex.clear();
                self.discarded_sysex_count = self.discarded_sysex_count.saturating_add(1);
            } else if message[0] >= 0x80 && message[0] != 0xF7 {
                self.sysex.clear();
                self.discarded_sysex_count = self.discarded_sysex_count.saturating_add(1);
                return self.parse_message(message);
            }
            if message.len() > MAX_SYSEX_BYTES - self.sysex.len() {
                self.sysex.clear();
                self.discarding_sysex = message.last() != Some(&0xF7);
                self.discarded_sysex_count = self.discarded_sysex_count.saturating_add(1);
                return Some(Err(MidiError::InvalidMessage(
                    "SysEx exceeds the input size limit".into(),
                )));
            }
            self.sysex.extend_from_slice(message);
            if message.last() == Some(&0xF7) {
                let data = std::mem::take(&mut self.sysex);
                return Some(Ok(MidiMessage::SystemExclusive { data }));
            }
            return None;
        }

        if message[0] == 0xF0 && message.len() > MAX_SYSEX_BYTES {
            self.discarding_sysex = message.last() != Some(&0xF7);
            self.discarded_sysex_count = self.discarded_sysex_count.saturating_add(1);
            return Some(Err(MidiError::InvalidMessage(
                "SysEx exceeds the input size limit".into(),
            )));
        }

        if message[0] == 0xF0 && message.last() != Some(&0xF7) {
            self.sysex.extend_from_slice(message);
            return None;
        }

        if message.len() <= self.inline.len() {
            self.inline[..message.len()].copy_from_slice(message);
            Some(MidiMessage::from_bytes(&self.inline[..message.len()]))
        } else {
            self.heap.clear();
            self.heap.extend_from_slice(message);
            Some(MidiMessage::from_bytes(&self.heap))
        }
    }
}
