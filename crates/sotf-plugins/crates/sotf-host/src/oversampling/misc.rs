/// Fixed chunk size for oversampling. Chosen to balance latency (~5ms @ 48kHz)
/// and efficiency.
pub const OS_CHUNK_SIZE: usize = 256;

/// Maximum number of channels supported by the oversampler.
/// 32 channels covers up to 9.1.6 with headroom.
pub(super) const MAX_OS_CHANNELS: usize = 32;

/// Map the current host transport snapshot to an internal chunk's input clock.
/// Buffered input precedes the current callback, and one callback may emit
/// several chunks. PPQ keeps the host's musical origin rather than assuming
/// sample zero is beat zero. Transport changes use the current host snapshot;
/// the fixed-chunk adapter does not schedule metadata changes within a chunk.
/// Event slices require separate buffering/rate conversion and remain empty.
pub(super) fn oversampled_context(
    context: &crate::plugin::ProcessContext<'_>,
    factor: u32,
    buffered_frames: usize,
    processed_frames: usize,
    os_frames: usize,
) -> crate::plugin::ProcessContext<'static> {
    let offset = processed_frames as i128 - buffered_frames as i128;
    let position = (i128::from(context.transport.sample_position) + offset)
        .clamp(0, i128::from(u64::MAX)) as u64;
    let mut transport = context.transport;
    transport.sample_position = position.saturating_mul(u64::from(factor));
    transport.ppq_position += offset as f64 / f64::from(context.sample_rate) * transport.bpm / 60.0;
    if let Some(range) = &mut transport.loop_range {
        range.start_sample = range.start_sample.saturating_mul(u64::from(factor));
        range.end_sample = range.end_sample.saturating_mul(u64::from(factor));
    }
    crate::plugin::ProcessContext::new(context.sample_rate * factor, os_frames)
        .with_transport(transport)
}

pub(super) fn advance_context(context: &mut crate::plugin::ProcessContext<'static>, frames: usize) {
    context.transport.sample_position = context
        .transport
        .sample_position
        .saturating_add(frames as u64);
    context.transport.ppq_position +=
        frames as f64 / f64::from(context.sample_rate) * context.transport.bpm / 60.0;
}

/// Convert interleaved audio to planar format.
///
/// `interleaved` is `[ch0_f0, ch1_f0, ch0_f1, ch1_f1, ...]`.
/// `planar[ch][frame]` is the output.
pub fn interleaved_to_planar(
    interleaved: &[f32],
    planar: &mut [Vec<f32>],
    num_frames: usize,
    num_channels: usize,
) {
    for ch in 0..num_channels {
        for frame in 0..num_frames {
            planar[ch][frame] = interleaved[frame * num_channels + ch];
        }
    }
}

/// Convert planar audio to interleaved format.
///
/// `planar[ch][frame]` is the input.
/// `interleaved` is `[ch0_f0, ch1_f0, ch0_f1, ch1_f1, ...]`.
pub fn planar_to_interleaved(
    planar: &[Vec<f32>],
    interleaved: &mut [f32],
    num_frames: usize,
    num_channels: usize,
) {
    for frame in 0..num_frames {
        for ch in 0..num_channels {
            interleaved[frame * num_channels + ch] = planar[ch][frame];
        }
    }
}
