//! Shared program writes for wide in-place and compact separate buffers.

// Rust guideline compliant 2026-02-21
use sotf_host::simd::flush_denormals_inplace;

pub(super) trait Sample: Copy {
    fn into_f32(self) -> f32;
    fn from_f32(value: f32) -> Self;
    fn flush(samples: &mut [Self]);
}

impl Sample for f32 {
    #[inline]
    fn into_f32(self) -> f32 {
        self
    }
    #[inline]
    fn from_f32(value: f32) -> Self {
        value
    }
    fn flush(samples: &mut [Self]) {
        flush_denormals_inplace(samples);
    }
}

impl Sample for f64 {
    #[inline]
    fn into_f32(self) -> f32 {
        self as f32
    }
    #[inline]
    fn from_f32(value: f32) -> Self {
        f64::from(value)
    }
    fn flush(samples: &mut [Self]) {
        for sample in samples {
            let mut value = *sample as f32;
            flush_denormals_inplace(std::slice::from_mut(&mut value));
            *sample = f64::from(value);
        }
    }
}

pub(super) trait AudioBuffer {
    fn input_len(&self) -> usize;
    fn read(&self, index: usize) -> f32;
    fn write(&mut self, frame: usize, channel: usize, stride: usize, value: f32);
    fn flush(&mut self, channels: usize, stride: usize);
}

impl<S: Sample> AudioBuffer for [S] {
    fn input_len(&self) -> usize {
        <[S]>::len(self)
    }
    #[inline]
    fn read(&self, index: usize) -> f32 {
        self[index].into_f32()
    }
    #[inline]
    fn write(&mut self, frame: usize, channel: usize, stride: usize, value: f32) {
        self[frame * stride + channel] = S::from_f32(value);
    }
    fn flush(&mut self, channels: usize, stride: usize) {
        if channels == stride {
            S::flush(self);
        } else {
            for frame in self.chunks_exact_mut(stride) {
                S::flush(&mut frame[..channels]);
            }
        }
    }
}

pub(super) struct SeparateBuffer<'a, S> {
    pub(super) input: &'a [S],
    pub(super) output: &'a mut [S],
    pub(super) channels: usize,
}

impl<S: Sample> AudioBuffer for SeparateBuffer<'_, S> {
    fn input_len(&self) -> usize {
        self.input.len()
    }
    #[inline]
    fn read(&self, index: usize) -> f32 {
        self.input[index].into_f32()
    }
    #[inline]
    fn write(&mut self, frame: usize, channel: usize, _stride: usize, value: f32) {
        self.output[frame * self.channels + channel] = S::from_f32(value);
    }
    fn flush(&mut self, _channels: usize, _stride: usize) {
        S::flush(self.output);
    }
}

pub(super) fn validate_f32_range(input: &[f64]) -> Result<(), String> {
    if input.iter().any(|&value| !(value as f32).is_finite()) {
        Err("Gate input cannot be represented as finite f32 samples".into())
    } else {
        Ok(())
    }
}
