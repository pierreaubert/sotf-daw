//! Translate native transport without discarding its independent musical origin.

// Rust guideline compliant 2026-02-21
use nih_plug::prelude::Transport;
use sotf_host::plugin::{LoopRange, ProcessContext, TimeSignature, TransportInfo};

/// Available transport values at the start of one native process slice.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeTransport {
    pub playing: bool,
    pub recording: bool,
    pub sample_position: Option<i64>,
    pub ppq_position: Option<f64>,
    pub bpm: Option<f64>,
    pub time_signature: Option<(i32, i32)>,
    pub loop_range: Option<(i64, i64)>,
}

impl From<&Transport> for NativeTransport {
    fn from(native: &Transport) -> Self {
        Self {
            playing: native.playing,
            recording: native.recording,
            sample_position: native.pos_samples(),
            ppq_position: native.pos_beats(),
            bpm: native.tempo,
            time_signature: native.time_sig_numerator.zip(native.time_sig_denominator),
            loop_range: native.loop_range_samples(),
        }
    }
}

/// Retain a bounded timeline when the host omits position or musical metadata.
///
/// Explicit native positions always win, including seeks and loop wraps. Missing
/// positions advance only while playing. Negative preroll remains signed inside
/// the cursor and is clipped to zero for SOTF's unsigned sample clock; its PPQ
/// origin remains intact. Invalid or absent tempo/signature retains the last
/// valid value (initially 120 BPM and 4/4). Reset or activation clears the cursor.
#[doc(hidden)]
#[derive(Debug, Default)]
pub struct TransportTracker {
    next_sample: i64,
    next_ppq: f64,
    previous: TransportInfo,
}

impl TransportTracker {
    /// Build one allocation-free context and advance the fallback timeline.
    pub fn context(
        &mut self,
        native: NativeTransport,
        sample_rate: f64,
        frames: usize,
    ) -> ProcessContext<'static> {
        let bpm = native
            .bpm
            .filter(|bpm| bpm.is_finite() && *bpm > 0.0)
            .unwrap_or(self.previous.bpm);
        let sample = native.sample_position.unwrap_or(self.next_sample);
        let beat_delta = (i128::from(sample) - i128::from(self.next_sample)) as f64
            / sample_rate
            * (bpm / 60.0);
        let estimated_ppq = finite_or(self.next_ppq + beat_delta, self.next_ppq);
        let ppq = native
            .ppq_position
            .filter(|ppq| ppq.is_finite())
            .unwrap_or(estimated_ppq);
        let time_signature = native
            .time_signature
            .and_then(|(numerator, denominator)| {
                let numerator = u8::try_from(numerator).ok()?;
                let denominator = u8::try_from(denominator).ok()?;
                (numerator > 0 && denominator > 0).then_some(TimeSignature {
                    numerator,
                    denominator,
                })
            })
            .unwrap_or(self.previous.time_signature);
        // A loop that crosses preroll is clipped to the representable timeline.
        // Entirely negative, reversed, and empty ranges are not advertised.
        let loop_range = native
            .loop_range
            .and_then(|(start, end)| LoopRange::new(start.max(0) as u64, end.max(0) as u64));
        let transport = TransportInfo {
            playing: native.playing,
            recording: native.recording,
            looping: loop_range.is_some(),
            sample_position: sample.max(0) as u64,
            bpm,
            time_signature,
            ppq_position: ppq,
            loop_range,
        };
        let advance = if native.playing {
            i64::try_from(frames).unwrap_or(i64::MAX)
        } else {
            0
        };
        self.next_sample = sample.saturating_add(advance);
        let elapsed = (i128::from(self.next_sample) - i128::from(sample)) as f64
            / sample_rate;
        self.next_ppq = finite_or(ppq + elapsed * (bpm / 60.0), ppq);
        self.previous = transport;
        ProcessContext::new(sample_rate, frames).with_transport(transport)
    }
}

fn finite_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() { value } else { fallback }
}

#[cfg(test)]
mod tests;
