//! Prepared reference storage and causal base-rate AutoGain measurement intervals.

// Rust guideline compliant 2026-02-21
use sotf_host::analyzer::RealTimeCache;
use sotf_host::auto_gain::{AutoGain, AutoGainData};

pub(super) struct AutoGainClock {
    reference: Vec<f32>,
    delay: Vec<f32>,
    delay_read: usize,
    frames_into_interval: usize,
    interval_frames: usize,
}

impl AutoGainClock {
    pub(super) fn new(
        channels: usize,
        sample_rate: u32,
        delay_frames: usize,
    ) -> Result<Self, String> {
        let prepared_samples = |frames: usize| {
            frames
                .checked_mul(channels)
                .filter(|samples| *samples <= isize::MAX as usize / std::mem::size_of::<f32>())
                .ok_or_else(|| "EQ AutoGain reference capacity overflow".to_string())
        };
        let reference_samples = prepared_samples(super::eq_plugin::EQ_MAX_BLOCK_FRAMES)?;
        let delay_samples = prepared_samples(delay_frames)?;
        Ok(Self {
            reference: vec![0.0; reference_samples],
            delay: vec![0.0; delay_samples],
            delay_read: 0,
            frames_into_interval: 0,
            // Exactly 10 Hz at standard rates; integer frame cadence otherwise.
            interval_frames: (sample_rate as usize / 10).max(1),
        })
    }

    pub(super) fn reset(&mut self) {
        self.reference.fill(0.0);
        self.delay.fill(0.0);
        self.delay_read = 0;
        self.frames_into_interval = 0;
    }

    pub(super) fn capture(&mut self, input: &[f32]) {
        self.reference[..input.len()].copy_from_slice(input);
    }

    pub(super) fn compensate_saved(
        &mut self,
        gain: &mut AutoGain,
        cache: &mut RealTimeCache<AutoGainData>,
        output: &mut [f32],
        channels: usize,
    ) {
        let input = &mut self.reference[..output.len()];
        // Advance only after the raw operation succeeds. The input ring delays
        // the measurement reference, not the audio, by the prepared pair delay.
        if !self.delay.is_empty() {
            for sample in input.iter_mut() {
                std::mem::swap(sample, &mut self.delay[self.delay_read]);
                self.delay_read += 1;
                if self.delay_read == self.delay.len() {
                    self.delay_read = 0;
                }
            }
        }
        compensate(
            &mut self.frames_into_interval,
            self.interval_frames,
            gain,
            cache,
            input,
            output,
            channels,
        );
    }

    pub(super) fn compensate_native(
        &mut self,
        gain: &mut AutoGain,
        cache: &mut RealTimeCache<AutoGainData>,
        input: &[f32],
        output: &mut [f32],
        channels: usize,
    ) {
        debug_assert!(self.delay.is_empty());
        compensate(
            &mut self.frames_into_interval,
            self.interval_frames,
            gain,
            cache,
            input,
            output,
            channels,
        );
    }
}

fn compensate(
    frames_into_interval: &mut usize,
    interval_frames: usize,
    gain: &mut AutoGain,
    cache: &mut RealTimeCache<AutoGainData>,
    input: &[f32],
    output: &mut [f32],
    channels: usize,
) {
    let frames = output.len() / channels;
    let mut offset = 0;
    while offset < frames {
        let span = (interval_frames - *frames_into_interval).min(frames - offset);
        let samples = offset * channels..(offset + span) * channels;
        // Preserve the existing best-effort meter-error policy. Ingest every
        // frame, including diagnostics while disabled; derive targets only
        // after the old target has compensated this entire completed interval.
        let _ = gain.ingest_input(&input[samples.clone()]);
        let _ = gain.ingest_output(&output[samples.clone()]);
        gain.apply_compensation(&mut output[samples], span);
        offset += span;
        *frames_into_interval += span;
        if *frames_into_interval == interval_frames {
            *frames_into_interval = 0;
            gain.refresh_input_measurement();
            gain.refresh_output_measurement();
            let data = gain.get_data();
            cache.update(|published| *published = data);
        }
    }
}
