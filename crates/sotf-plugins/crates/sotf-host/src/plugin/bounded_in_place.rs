//! Prepared storage for explicitly partition-independent in-place processors.

// Rust guideline compliant 2026-02-21
use super::{PluginResult, ProcessContext};

// This bounds scratch and each inner call, not the public callback size.
// 256 frames amortizes copies while keeping storage small for sidechain buses.
const CHUNK_FRAMES: usize = 256;

#[derive(Debug, Default)]
pub(crate) struct BoundedInPlace {
    pub(crate) f32_samples: Vec<f32>,
    pub(crate) f64_samples: Vec<f64>,
    channels: [usize; 2],
    sample_rate: u32,
}

impl BoundedInPlace {
    pub(crate) fn invalidate(&mut self) {
        self.sample_rate = 0;
    }

    pub(crate) fn prepare(
        &mut self,
        enabled: bool,
        channels: [usize; 2],
        native_f64: bool,
        sample_rate: u32,
    ) -> PluginResult<()> {
        self.invalidate();
        if !enabled {
            return if channels[0] == channels[1] {
                Ok(())
            } else {
                Err("Asymmetric in-place adapter requires bounded subdivision support".into())
            };
        }
        if sample_rate == 0 || channels[1] == 0 || channels[0] < channels[1] {
            return Err("Invalid bounded in-place sample rate or channel layout".into());
        }
        let samples = channels[0]
            .checked_mul(CHUNK_FRAMES)
            .filter(|n| *n <= isize::MAX as usize / std::mem::size_of::<f64>())
            .ok_or_else(|| "Bounded in-place scratch capacity overflow".to_string())?;
        self.f32_samples.resize(samples, 0.0);
        if native_f64 && channels[0] != channels[1] {
            self.f64_samples.resize(samples, 0.0);
        }
        self.channels = channels;
        self.sample_rate = sample_rate;
        Ok(())
    }

    pub(crate) fn validate(
        &self,
        channels: [usize; 2],
        context: &ProcessContext<'_>,
    ) -> PluginResult<()> {
        if self.sample_rate == 0 {
            return Err("Bounded in-place adapter must be initialized before processing".into());
        }
        if self.channels != channels || self.sample_rate != context.sample_rate {
            return Err(
                "Bounded in-place layout or sample rate changed; reinitialize first".into(),
            );
        }
        Ok(())
    }
}

pub(crate) trait Cast<T> {
    fn cast(self) -> T;
}

macro_rules! sample_cast {
    ($from:ty, $to:ty) => {
        impl Cast<$to> for $from {
            #[inline]
            fn cast(self) -> $to {
                self as $to
            }
        }
    };
}
sample_cast!(f32, f32);
sample_cast!(f32, f64);
sample_cast!(f64, f32);
sample_cast!(f64, f64);

/// Process a fully validated block through prepared input-stride storage.
pub(crate) fn process<S: Copy + Cast<W>, W: Copy + Cast<S>>(
    scratch: &mut [W],
    input: &[S],
    output: &mut [S],
    context: &ProcessContext<'_>,
    channels: [usize; 2],
    mut process: impl FnMut(&mut [W], &ProcessContext<'_>) -> PluginResult<usize>,
) -> PluginResult<usize> {
    let [input_channels, output_channels] = channels;
    for first in (0..context.num_frames).step_by(CHUNK_FRAMES) {
        let frames = (context.num_frames - first).min(CHUNK_FRAMES);
        let work = &mut scratch[..frames * input_channels];
        for (dst, src) in work.iter_mut().zip(&input[first * input_channels..]) {
            *dst = (*src).cast();
        }
        // Opted-in processors ignore transport/events. Preserve the original
        // snapshot rather than inventing rebased event offsets or PPQ origins.
        let chunk_context = ProcessContext {
            num_frames: frames,
            ..*context
        };
        check_frames(process(work, &chunk_context)?, frames)?;
        for (src, dst) in work
            .chunks_exact(input_channels)
            .zip(output[first * output_channels..].chunks_exact_mut(output_channels))
        {
            for (dst, src) in dst.iter_mut().zip(src) {
                *dst = (*src).cast();
            }
        }
    }
    Ok(context.num_frames)
}

/// Retain every input-stride lane for direct in-place f64 fallback callers.
pub(crate) fn fallback_in_place(
    scratch: &mut [f32],
    buffer: &mut [f64],
    context: &ProcessContext<'_>,
    channels: usize,
    mut process: impl FnMut(&mut [f32], &ProcessContext<'_>) -> PluginResult<usize>,
) -> PluginResult<usize> {
    for block in buffer.chunks_mut(CHUNK_FRAMES * channels) {
        let work = &mut scratch[..block.len()];
        for (dst, src) in work.iter_mut().zip(block.iter()) {
            *dst = *src as f32;
        }
        let chunk_context = ProcessContext {
            num_frames: block.len() / channels,
            ..*context
        };
        check_frames(process(work, &chunk_context)?, chunk_context.num_frames)?;
        for (dst, src) in block.iter_mut().zip(work.iter()) {
            *dst = f64::from(*src);
        }
    }
    Ok(context.num_frames)
}

fn check_frames(actual: usize, expected: usize) -> PluginResult<()> {
    if actual != expected {
        return Err(format!(
            "Bounded in-place plugin returned {actual} frames, expected {expected}"
        ));
    }
    Ok(())
}
