//! Explicit stereo source-remix stage; deterministic Downmix is unchanged.
//!
//! This QA-only plugin follows an ordinary downmix. It estimates a spatially
//! centered foreground, not speech identity. Foreground/bed gains are bounded;
//! uncertain or unavailable learned evidence uses neutral remix. Equal gains
//! are an explicit deterministic global-gain request and require no separation.

// Rust guideline compliant 2026-02-21
mod model;
use math_audio_dsp::stft::RealFftProcessor;
pub use model::{SourceMask, spatial_features};
use rustfft::num_complex::Complex;
use serde::{Deserialize, Serialize};
use sotf_host::plugin::{
    Plugin, PluginCompileMetadata, PluginCostClass, PluginDrainResult, PluginInfo, PluginResult,
    ProcessContext, TailLength,
};
use sotf_host::{Parameter, ParameterId, ParameterValue};

/// Fixed analysis size adds a declared512 samples after the ordinary downmix.
pub const FFT_SIZE: usize = 512;
const HOP: usize = FFT_SIZE / 2;
const BINS: usize = HOP + 1;
// Five-ms control smoothing is frozen by the validated model policy.
const SMOOTH_SECONDS: f64 = 0.005;

/// Explicit structural method choice, separate from ordinary downmix settings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemixMethod {
    #[default]
    Bypass,
    Analytic,
    Learned,
}

/// Bounded source-gain request for the separate optional stage.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemixParams {
    pub foreground_gain_db: f32,
    pub bed_gain_db: f32,
    pub mix: f32,
    pub method: RemixMethod,
}
impl Default for RemixParams {
    fn default() -> Self {
        Self {
            foreground_gain_db: 0.0,
            bed_gain_db: 0.0,
            mix: 1.0,
            method: RemixMethod::Bypass,
        }
    }
}
impl RemixParams {
    fn validate(&self) -> PluginResult<()> {
        if [self.foreground_gain_db, self.bed_gain_db]
            .iter()
            .any(|v| !v.is_finite() || !(-6.0..=6.0).contains(v))
            || !self.mix.is_finite()
            || !(0.0..=1.0).contains(&self.mix)
        {
            return Err("source-remix gains require±6dB and mix0..1".into());
        }
        Ok(())
    }
}

/// Source-frame decisions; drain freezes these counters and last spectral gains.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RemixCounters {
    pub frames: u64,
    pub foreground_bins: u64,
    pub bed_bins: u64,
    pub uncertain_bins: u64,
    pub unavailable_bins: u64,
    pub invalid_bins: u64,
    pub boundary_bins: u64,
    pub bypass_bins: u64,
    pub common_gain_bins: u64,
}

/// A separate unadmitted source-aware remix plugin with deterministic neutral fallback.
pub struct SourceRemix {
    params: RemixParams,
    model: Option<SourceMask>,
    rate: f64,
    pole: f32,
    foreground: f32,
    bed: f32,
    mix: f32,
    fft: RealFftProcessor,
    window: Vec<f32>,
    input: [Vec<f32>; 2],
    fill: usize,
    spectra: [Vec<Complex<f32>>; 2],
    ola: Vec<f32>,
    read: usize,
    write: usize,
    prefix: bool,
    mid_gain: Vec<f32>,
    side_gain: Vec<f32>,
    rendered_frames: u64,
    counters: RemixCounters,
    has_input: bool,
    phase: usize,
    remaining: Option<usize>,
    silence: Vec<f32>,
    ids: [ParameterId; 3],
}
impl std::fmt::Debug for SourceRemix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceRemix")
            .field("params", &self.params)
            .field("sample_rate", &self.rate)
            .field("model_available", &self.model.is_some())
            .field("counters", &self.counters)
            .finish_non_exhaustive()
    }
}

impl SourceRemix {
    /// Prepare a separate stereo stage on the control thread.
    ///
    /// # Errors
    /// Rejects invalid gains/mix or nonpositive/nonfinite sample clock.
    pub fn new(
        params: RemixParams,
        model: Option<SourceMask>,
        sample_rate: f64,
    ) -> PluginResult<Self> {
        params.validate()?;
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("source remix requires a positive finite sample rate".into());
        }
        let mut result = Self {
            params,
            model,
            rate: sample_rate,
            pole: 0.0,
            foreground: 1.0,
            bed: 1.0,
            mix: 0.0,
            fft: RealFftProcessor::new_bidirectional(FFT_SIZE),
            window: sotf_host::stft_common::generate_sqrt_hann_window(FFT_SIZE),
            input: [vec![0.0; FFT_SIZE], vec![0.0; FFT_SIZE]],
            fill: HOP,
            spectra: [
                vec![Complex::new(0.0, 0.0); BINS],
                vec![Complex::new(0.0, 0.0); BINS],
            ],
            ola: vec![0.0; 4 * FFT_SIZE],
            read: 0,
            write: HOP,
            prefix: true,
            mid_gain: vec![1.0; BINS],
            side_gain: vec![1.0; BINS],
            rendered_frames: 0,
            counters: RemixCounters::default(),
            has_input: false,
            phase: 0,
            remaining: None,
            silence: vec![0.0; HOP * 2],
            ids: [
                "foreground_gain_db".into(),
                "bed_gain_db".into(),
                "mix".into(),
            ],
        };
        result.initialize(sample_rate)?;
        Ok(result)
    }

    /// Copy source-frame selection counters without allocation.
    pub fn counters(&self) -> RemixCounters {
        self.counters
    }
    /// Read the last conditional M/S gains for independent trace reconstruction.
    pub fn spectral_gains(&self) -> (&[f32], &[f32]) {
        (&self.mid_gain, &self.side_gain)
    }
    /// Count all rendered FFT frames, including frozen-gain EOF frames.
    pub fn rendered_frames(&self) -> u64 {
        self.rendered_frames
    }
    /// Read the exact prepared analysis window for independent numerical references.
    pub fn analysis_window(&self) -> &[f32] {
        &self.window
    }

    fn prepare_gains(&mut self) {
        self.counters.frames = self.counters.frames.saturating_add(1);
        let target_a = 10_f32.powf(self.params.foreground_gain_db / 20.0);
        let target_b = 10_f32.powf(self.params.bed_gain_db / 20.0);
        self.foreground = target_a + self.pole * (self.foreground - target_a);
        self.bed = target_b + self.pole * (self.bed - target_b);
        self.mix = self.params.mix + self.pole * (self.mix - self.params.mix);
        let supported = [44100.0, 48000.0, 96000.0].contains(&self.rate);
        for k in 0..BINS {
            self.mid_gain[k] = 1.0;
            self.side_gain[k] = 1.0;
            if self.params.method == RemixMethod::Bypass || self.mix == 0.0 {
                self.counters.bypass_bins += 1;
                continue;
            }
            if self.foreground == self.bed {
                let common = 1.0 + self.mix * (self.bed - 1.0);
                self.mid_gain[k] = common;
                self.side_gain[k] = common;
                self.counters.common_gain_bins += 1;
                continue;
            }
            if k == 0 || k == BINS - 1 {
                self.counters.boundary_bins += 1;
                continue;
            }
            if self.params.method == RemixMethod::Learned && (!supported || self.model.is_none()) {
                self.counters.unavailable_bins += 1;
                continue;
            }
            let Some(features) = spatial_features(self.spectra[0][k], self.spectra[1][k], k) else {
                self.counters.invalid_bins += 1;
                continue;
            };
            let prediction = match self.params.method {
                RemixMethod::Learned => self.model.as_ref().and_then(|m| m.predict(&features)),
                RemixMethod::Analytic => Some(
                    ((2.0 * features[0] - 1.0) / features[0].max(1e-20)).clamp(0.0, 1.0) as f32,
                ),
                RemixMethod::Bypass => unreachable!(),
            };
            let Some(p) = prediction else {
                self.counters.invalid_bins += 1;
                continue;
            };
            if p >= 0.85 {
                self.counters.foreground_bins += 1;
            } else if p <= 0.15 {
                self.counters.bed_bins += 1;
            } else {
                self.counters.uncertain_bins += 1;
                continue;
            }
            // Positive conditional eigenvalues lie between requested source
            // gains. Mixing with identity retains the same±6dB operator bound.
            self.mid_gain[k] = 1.0 + self.mix * (self.bed + (self.foreground - self.bed) * p - 1.0);
            self.side_gain[k] = 1.0 + self.mix * (self.bed - 1.0);
        }
    }

    fn stream(&mut self, input: &[f32], output: &mut [f32], frames: usize, frozen: bool) {
        for i in 0..frames {
            for ch in 0..2 {
                output[2 * i + ch] = self.ola[2 * self.read + ch];
                self.ola[2 * self.read + ch] = 0.0;
                self.input[ch][self.fill] = input[2 * i + ch];
            }
            self.read = (self.read + 1) % (2 * FFT_SIZE);
            self.fill += 1;
            if self.fill == FFT_SIZE {
                for ch in 0..2 {
                    for j in 0..FFT_SIZE {
                        self.fft.time_buffer[j] = self.input[ch][j] * self.window[j];
                    }
                    self.fft.forward();
                    self.spectra[ch].copy_from_slice(&self.fft.freq_buffer);
                }
                if !frozen {
                    self.prepare_gains();
                }
                for ch in 0..2 {
                    for k in 0..BINS {
                        let mid = (self.spectra[0][k] + self.spectra[1][k]) * 0.5;
                        let side = (self.spectra[0][k] - self.spectra[1][k]) * 0.5;
                        self.fft.freq_buffer[k] = mid * self.mid_gain[k]
                            + side
                                * (if ch == 0 {
                                    self.side_gain[k]
                                } else {
                                    -self.side_gain[k]
                                });
                    }
                    self.fft.freq_buffer[0].im = 0.0;
                    self.fft.freq_buffer[BINS - 1].im = 0.0;
                    self.fft.inverse();
                    for j in if self.prefix { HOP } else { 0 }..FFT_SIZE {
                        let position = (self.write + j) % (2 * FFT_SIZE);
                        self.ola[2 * position + ch] +=
                            self.fft.time_buffer[j] * self.window[j] / FFT_SIZE as f32;
                    }
                    self.input[ch].copy_within(HOP..FFT_SIZE, 0);
                    self.input[ch][HOP..].fill(0.0);
                }
                self.fill = HOP;
                self.write = (self.write + HOP) % (2 * FFT_SIZE);
                self.prefix = false;
                self.rendered_frames += 1;
            }
        }
    }
}

impl Plugin for SourceRemix {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Centered Source Remix (QA)", "0.1.0", "SotF")
    }
    fn input_channels(&self) -> usize {
        2
    }
    fn output_channels(&self) -> usize {
        2
    }
    fn latency_samples(&self) -> usize {
        FFT_SIZE
    }
    fn compile_metadata(&self) -> PluginCompileMetadata {
        PluginCompileMetadata::nonlinear(PluginCostClass::Fft, None, FFT_SIZE, true)
    }
    fn parameters(&self) -> Vec<Parameter> {
        vec![
            Parameter::new_float(
                "foreground_gain_db",
                "Foreground gain",
                self.params.foreground_gain_db,
                -6.0,
                6.0,
            ),
            Parameter::new_float(
                "bed_gain_db",
                "Bed gain",
                self.params.bed_gain_db,
                -6.0,
                6.0,
            ),
            Parameter::new_float("mix", "Remix amount", self.params.mix, 0.0, 1.0),
        ]
    }
    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        self.ids.iter().position(|v| v == id).map(|index| {
            ParameterValue::Float(
                [
                    self.params.foreground_gain_db,
                    self.params.bed_gain_db,
                    self.params.mix,
                ][index],
            )
        })
    }
    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()> {
        self.validate_parameter(&id, &value)?;
        let index = self
            .ids
            .iter()
            .position(|v| v == &id)
            .ok_or("unknown source-remix parameter")?;
        let value = value
            .as_float()
            .ok_or("source-remix controls require float values")?;
        let mut next = self.params.clone();
        match index {
            0 => next.foreground_gain_db = value,
            1 => next.bed_gain_db = value,
            _ => next.mix = value,
        };
        next.validate()?;
        self.params = next;
        if !self.has_input {
            self.foreground = 10_f32.powf(self.params.foreground_gain_db / 20.0);
            self.bed = 10_f32.powf(self.params.bed_gain_db / 20.0);
            self.mix = self.params.mix;
        }
        Ok(())
    }
    fn validate_parameter(&self, id: &ParameterId, value: &ParameterValue) -> PluginResult<()> {
        if self.remaining.is_some() {
            return Err("reset source remix before control changes after drain".into());
        }
        let index = self
            .ids
            .iter()
            .position(|v| v == id)
            .ok_or("unknown source-remix parameter")?;
        let value = value
            .as_float()
            .ok_or("source-remix controls require float values")?;
        let mut next = self.params.clone();
        match index {
            0 => next.foreground_gain_db = value,
            1 => next.bed_gain_db = value,
            _ => next.mix = value,
        };
        next.validate()
    }
    fn initialize(&mut self, sample_rate: f64) -> PluginResult<()> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("source remix requires a positive finite sample clock".into());
        }
        self.rate = sample_rate;
        self.pole = (-(HOP as f64) / sample_rate / SMOOTH_SECONDS).exp() as f32;
        self.reset();
        Ok(())
    }
    fn reset(&mut self) {
        for v in &mut self.input {
            v.fill(0.0);
        }
        for v in &mut self.spectra {
            v.fill(Complex::new(0.0, 0.0));
        }
        self.ola.fill(0.0);
        self.fill = HOP;
        self.read = 0;
        self.write = HOP;
        self.prefix = true;
        self.mid_gain.fill(1.0);
        self.side_gain.fill(1.0);
        self.foreground = 10_f32.powf(self.params.foreground_gain_db / 20.0);
        self.bed = 10_f32.powf(self.params.bed_gain_db / 20.0);
        self.mix = self.params.mix;
        self.rendered_frames = 0;
        self.counters = RemixCounters::default();
        self.has_input = false;
        self.phase = 0;
        self.remaining = None;
    }
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        let n = context.num_frames;
        let used = n
            .checked_mul(2)
            .ok_or("source-remix frame count overflow")?;
        if context.sample_rate != self.rate
            || input.len() < used
            || output.len() < used
            || (n > 0 && self.remaining.is_some())
            || input[..used]
                .iter()
                .any(|v| !v.is_finite() || v.abs() > 64.0)
        {
            return Err("invalid source-remix rate, geometry, state or bounded input".into());
        }
        self.stream(&input[..used], &mut output[..used], n, false);
        self.has_input |= n > 0;
        self.phase = (self.phase + n % HOP) % HOP;
        Ok(n)
    }
    fn tail_length(&self) -> TailLength {
        TailLength::Finite((2 * FFT_SIZE - 1) as u64)
    }
    fn drain_output_frames_max(&self) -> usize {
        HOP
    }
    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        // Framewise bound remains valid even for one-frame output capacities.
        let remaining = if self.has_input {
            self.remaining
                .unwrap_or(2 * FFT_SIZE - HOP + (HOP - self.phase) % HOP)
        } else {
            0
        };
        std::num::NonZeroU64::new(remaining.max(1) as u64)
    }
    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<PluginDrainResult> {
        if context.sample_rate != self.rate || !output.len().is_multiple_of(2) {
            return Err("source-remix drain needs matched rate/stereo geometry".into());
        }
        if !self.has_input || self.remaining == Some(0) {
            return Ok(PluginDrainResult::COMPLETE);
        }
        if output.is_empty() {
            return Err("source-remix drain needs positive capacity".into());
        }
        let remaining = self
            .remaining
            .unwrap_or(2 * FFT_SIZE - HOP + (HOP - self.phase) % HOP);
        let count = remaining.min(output.len() / 2).min(HOP);
        let silence = std::mem::take(&mut self.silence);
        self.stream(&silence[..count * 2], &mut output[..count * 2], count, true);
        self.silence = silence;
        self.remaining = Some(remaining - count);
        Ok(PluginDrainResult {
            frames: count,
            complete: remaining == count,
        })
    }
}
