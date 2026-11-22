//! Verify playback with FFT backends that require scratch storage.

use crate::{UpmixerPlugin, UpmixerPluginParams};
use realfft::{ComplexToReal, FftError, RealToComplex};
use rustfft::num_complex::Complex;
use sotf_host::test_utils::{CountingAlloc, assert_no_allocs_or_deallocs};
use sotf_host::{Plugin, ProcessContext};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const SCRATCH: usize = 8;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

struct Forward {
    inner: Arc<dyn RealToComplex<f32>>,
    calls: Arc<AtomicUsize>,
}

impl RealToComplex<f32> for Forward {
    fn process(&self, input: &mut [f32], output: &mut [Complex<f32>]) -> Result<(), FftError> {
        let mut scratch = self.make_scratch_vec();
        self.process_with_scratch(input, output, &mut scratch)
    }
    fn process_with_scratch(
        &self,
        input: &mut [f32],
        output: &mut [Complex<f32>],
        scratch: &mut [Complex<f32>],
    ) -> Result<(), FftError> {
        assert!(scratch.len() >= self.get_scratch_len());
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.inner.process_with_scratch(input, output, scratch)
    }
    fn get_scratch_len(&self) -> usize {
        self.inner.get_scratch_len().max(SCRATCH)
    }
    fn len(&self) -> usize {
        self.inner.len()
    }
    fn make_input_vec(&self) -> Vec<f32> {
        self.inner.make_input_vec()
    }
    fn make_output_vec(&self) -> Vec<Complex<f32>> {
        self.inner.make_output_vec()
    }
    fn make_scratch_vec(&self) -> Vec<Complex<f32>> {
        vec![Complex::new(0.0, 0.0); self.get_scratch_len()]
    }
}

struct Inverse {
    inner: Arc<dyn ComplexToReal<f32>>,
    calls: Arc<AtomicUsize>,
}

impl ComplexToReal<f32> for Inverse {
    fn process(&self, input: &mut [Complex<f32>], output: &mut [f32]) -> Result<(), FftError> {
        let mut scratch = self.make_scratch_vec();
        self.process_with_scratch(input, output, &mut scratch)
    }
    fn process_with_scratch(
        &self,
        input: &mut [Complex<f32>],
        output: &mut [f32],
        scratch: &mut [Complex<f32>],
    ) -> Result<(), FftError> {
        assert!(scratch.len() >= self.get_scratch_len());
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.inner.process_with_scratch(input, output, scratch)
    }
    fn get_scratch_len(&self) -> usize {
        self.inner.get_scratch_len().max(SCRATCH)
    }
    fn len(&self) -> usize {
        self.inner.len()
    }
    fn make_input_vec(&self) -> Vec<Complex<f32>> {
        self.inner.make_input_vec()
    }
    fn make_output_vec(&self) -> Vec<f32> {
        self.inner.make_output_vec()
    }
    fn make_scratch_vec(&self) -> Vec<Complex<f32>> {
        vec![Complex::new(0.0, 0.0); self.get_scratch_len()]
    }
}

#[test]
fn main_and_hr_callbacks_work_with_plans_requiring_scratch() {
    let mut params = UpmixerPluginParams::default();
    params.core.speaker_config = "9.1.6".into();
    params.core.enable_hr_direct = true;
    let mut reference = UpmixerPlugin::from_params(params.clone());
    let mut plugin = UpmixerPlugin::from_params(params);
    reference.initialize(48_000.0).unwrap();
    plugin.initialize(48_000.0).unwrap();
    let calls: Vec<_> = (0..4).map(|_| Arc::new(AtomicUsize::new(0))).collect();
    plugin.fft.fft_forward = Arc::new(Forward {
        inner: Arc::clone(&plugin.fft.fft_forward),
        calls: Arc::clone(&calls[0]),
    });
    plugin.fft.fft_inverse = Arc::new(Inverse {
        inner: Arc::clone(&plugin.fft.fft_inverse),
        calls: Arc::clone(&calls[1]),
    });
    plugin.fft.hr_fft_forward = Arc::new(Forward {
        inner: Arc::clone(&plugin.fft.hr_fft_forward),
        calls: Arc::clone(&calls[2]),
    });
    plugin.fft.hr_fft_inverse = Arc::new(Inverse {
        inner: Arc::clone(&plugin.fft.hr_fft_inverse),
        calls: Arc::clone(&calls[3]),
    });
    plugin.fft.prepare_scratch();
    let frames = 8192;
    let input: Vec<_> = (0..frames * 2)
        .map(|index| ((index * 37 % 997) as f32 - 498.0) / 1000.0)
        .collect();
    let mut expected = vec![0.0; frames * reference.output_channels()];
    let mut actual = vec![0.0; frames * plugin.output_channels()];
    let context = ProcessContext::new(48_000, frames);
    reference.process(&input, &mut expected, &context).unwrap();
    assert_no_allocs_or_deallocs("FFT callback with prepared scratch", || {
        plugin.process(&input, &mut actual, &context).unwrap();
    });
    assert_eq!(actual, expected);
    assert!(actual.iter().any(|sample| sample.abs() > 1e-5));
    for call in calls {
        assert!(call.load(Ordering::Relaxed) > 0);
    }
    reference.resize_fft(1024);
    plugin.resize_fft(1024);
    assert!(plugin.fft.scratch.len() >= SCRATCH);
    reference.process(&input, &mut expected, &context).unwrap();
    assert_no_allocs_or_deallocs("FFT callback after replan", || {
        plugin.process(&input, &mut actual, &context).unwrap();
    });
    assert_eq!(actual, expected);
}
