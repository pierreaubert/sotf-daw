//! Private native limiter DSP state and arithmetic.
//!
//! The public plugin owns parameters, lifecycle and the telemetry cache. This
//! kernel owns only derived DSP histories and borrows the owner's cache when
//! processing. Its native arithmetic and meter cadence are preserved verbatim.

// Rust guideline compliant 2026-02-21
use super::misc::CACHE_UPDATE_THROTTLE;
use super::types::LimiterData;
use math_audio_dsp::fast_math::{fast_log10, fast_pow10};
use sotf_host::DualRelease;
use sotf_host::analyzer::RealTimeCache;
use sotf_host::plugin::{PluginResult, ProcessContext};
use sotf_host::simd::{enable_ftz_daz, flush_denormals_inplace};
use sotf_host::smoothing::Smoother;

const TRUE_PEAK_HISTORY: usize = 25;
// 49-tap, 4x Hann-windowed sinc interpolator used by libebur128-style
// BS.1770-compatible true-peak meters, stored [past input offset][output phase].
// Each kernel is generated off-line from
//   sinc(d / factor) * 0.5 * (1 + cos(pi * d / 24)),  d = -24..=24.
// f32 is deliberate for the plugin's f32 realtime path.
#[allow(clippy::excessive_precision)]
const BS1770_4X_HANN_SINC_KERNEL: [[f32; 4]; 13] = [
    [0.0, -0.000167441976, -0.000986013305, -0.001631726164],
    [0.0, 0.004895983143, 0.010358978572, 0.01035995497],
    [0.0, -0.018526005936, -0.033703603628, -0.030107748293],
    [0.0, 0.046265053486, 0.080138909395, 0.06915846968],
    [0.0, -0.103456725953, -0.181129655074, -0.16145852729],
    [0.0, 0.288683355573, 0.625773626012, 0.896465150711],
    [1.0, 0.896465150711, 0.625773626012, 0.288683355573],
    [0.0, -0.16145852729, -0.181129655074, -0.103456725953],
    [0.0, 0.06915846968, 0.080138909395, 0.046265053486],
    [0.0, -0.030107748293, -0.033703603628, -0.018526005936],
    [0.0, 0.01035995497, 0.010358978572, 0.004895983143],
    [0.0, -0.001631726164, -0.000986013305, -0.000167441976],
    [0.0, 0.0, 0.0, 0.0],
];

#[allow(clippy::excessive_precision)]
const BS1770_2X_HANN_SINC_KERNEL: [[f32; 2]; 25] = [
    [0.0, -0.000118399357],
    [0.0, 0.001153804635],
    [0.0, -0.003461982881],
    [0.0, 0.007325594412],
    [0.0, -0.013099864426],
    [0.0, 0.021289392984],
    [0.0, -0.032714333052],
    [0.0, 0.048902422887],
    [0.0, -0.073154952481],
    [0.0, 0.114168419527],
    [0.0, -0.204129958342],
    [0.0, 0.633896587165],
    [1.0, 0.633896587165],
    [0.0, -0.204129958342],
    [0.0, 0.114168419527],
    [0.0, -0.073154952481],
    [0.0, 0.048902422887],
    [0.0, -0.032714333052],
    [0.0, 0.021289392984],
    [0.0, -0.013099864426],
    [0.0, 0.007325594412],
    [0.0, -0.003461982881],
    [0.0, 0.001153804635],
    [0.0, -0.000118399357],
    [0.0, 0.0],
];

#[derive(Clone)]
pub(super) struct Bs1770TruePeakDetector {
    pub(super) history: [f32; TRUE_PEAK_HISTORY],
    write_pos: usize,
    oversample_factor: u8,
}

impl Bs1770TruePeakDetector {
    pub(super) fn new(sample_rate: u32) -> Self {
        Self {
            history: [0.0; TRUE_PEAK_HISTORY],
            write_pos: 0,
            oversample_factor: Self::factor_for_sample_rate(sample_rate),
        }
    }

    /// BS.1770 requires the measurement sampling frequency to be at least
    /// 192 kHz. These factors are the specified operating points for common
    /// 44.1/48 kHz families and retain a bounded fallback for other rates.
    fn factor_for_sample_rate(sample_rate: u32) -> u8 {
        if sample_rate < 96_000 {
            4
        } else if sample_rate < 192_000 {
            2
        } else {
            1
        }
    }

    fn set_sample_rate(&mut self, sample_rate: u32) {
        self.oversample_factor = Self::factor_for_sample_rate(sample_rate);
        self.reset();
    }

    #[inline]
    pub(super) fn detector_delay_samples(sample_rate: u32) -> usize {
        match Self::factor_for_sample_rate(sample_rate) {
            4 => 6,
            2 => 12,
            _ => 0,
        }
    }

    #[inline]
    fn history_sample(&self, past_offset: usize) -> f32 {
        self.history[(self.write_pos + TRUE_PEAK_HISTORY - past_offset) % TRUE_PEAK_HISTORY]
    }

    #[inline]
    fn push_and_interpolate_4x(&mut self, sample: f32) -> [f32; 4] {
        self.history[self.write_pos] = sample;
        let mut phases = [0.0f32; 4];
        for (past_offset, coefficients) in BS1770_4X_HANN_SINC_KERNEL.iter().enumerate() {
            let history_sample = self.history_sample(past_offset);
            for (phase, output) in phases.iter_mut().enumerate() {
                *output += coefficients[phase] * history_sample;
            }
        }
        self.write_pos = (self.write_pos + 1) % TRUE_PEAK_HISTORY;
        phases
    }

    #[inline]
    pub(super) fn process_linear(&mut self, sample: f32) -> f32 {
        match self.oversample_factor {
            4 => self
                .push_and_interpolate_4x(sample)
                .into_iter()
                .map(f32::abs)
                .fold(0.0, f32::max),
            2 => {
                self.history[self.write_pos] = sample;
                let mut phases = [0.0f32; 2];
                for (past_offset, coefficients) in BS1770_2X_HANN_SINC_KERNEL.iter().enumerate() {
                    let history_sample = self.history_sample(past_offset);
                    for (phase, output) in phases.iter_mut().enumerate() {
                        *output += coefficients[phase] * history_sample;
                    }
                }
                self.write_pos = (self.write_pos + 1) % TRUE_PEAK_HISTORY;
                phases[0].abs().max(phases[1].abs())
            }
            _ => sample.abs(),
        }
    }

    pub(super) fn reset(&mut self) {
        self.history.fill(0.0);
        self.write_pos = 0;
    }
}

pub(super) struct SlidingMaximum {
    values: Vec<f32>,
    indices: Vec<usize>,
    head: usize,
    len: usize,
    next_index: usize,
}

impl SlidingMaximum {
    fn new(capacity: usize) -> Self {
        Self {
            values: vec![0.0; capacity + 1],
            indices: vec![0; capacity + 1],
            head: 0,
            len: 0,
            next_index: 0,
        }
    }
    fn reset(&mut self) {
        self.head = 0;
        self.len = 0;
        self.next_index = 0;
    }
    fn push(&mut self, value: f32, window: usize) -> f32 {
        let capacity = self.values.len();
        while self.len > 0 {
            let tail = (self.head + self.len - 1) % capacity;
            if self.values[tail] > value {
                break;
            }
            self.len -= 1;
        }
        let tail = (self.head + self.len) % capacity;
        self.values[tail] = value;
        self.indices[tail] = self.next_index;
        self.len += 1;
        let oldest = self.next_index.saturating_add(1).saturating_sub(window);
        while self.len > 0 && self.indices[self.head] < oldest {
            self.head = (self.head + 1) % capacity;
            self.len -= 1;
        }
        self.next_index += 1;
        if self.len == 0 {
            value
        } else {
            self.values[self.head]
        }
    }
}

/// Scalar controls borrowed from the public parameter owner for one block.
#[derive(Clone, Copy)]
pub(super) struct KernelControls {
    pub(super) channels: usize,
    pub(super) soft: bool,
    pub(super) true_peak: bool,
    pub(super) isp_mode: bool,
    pub(super) dual_release: bool,
    pub(super) link_amount: f32,
}

/// Prepared native processing state, without a public parameter registry.
pub(super) struct NativeKernel {
    /// Threshold is smoothed in dB so equal time intervals produce equal
    /// perceptual gain changes rather than an asymmetric linear-amplitude ramp.
    pub(super) threshold_db_smoother: Smoother,
    pub(super) mix_smoother: Smoother,
    pub(super) envelope: f32,
    pub(super) release_coeff: f32,
    pub(super) lookahead_buffer: Vec<f32>,
    pub(super) lookahead_pos: usize,
    pub(super) lookahead_len: usize,
    pub(super) true_peak_detectors: Vec<Bs1770TruePeakDetector>,
    /// Detectors for the gain-modulated signal before predictive ISP correction.
    pub(super) output_isp_detectors: Vec<Bs1770TruePeakDetector>,
    /// Current maximum output-stage gain reduction, in dB.
    pub(super) isp_correction_db: f32,
    pub(super) isp_delay_buffer: Vec<f32>,
    pub(super) isp_delay_pos: usize,
    pub(super) isp_delay_len: usize,
    pub(super) isp_maxima: Vec<SlidingMaximum>,
    pub(super) isp_peaks: Vec<f32>,
    pub(super) isp_gains: Vec<f32>,
    pub(super) dual_release_env: DualRelease,
    pub(super) channel_dual_release: Vec<DualRelease>,
    pub(super) cache_update_counter: usize,
    pub(super) monitoring_peak_db: f32,
    pub(super) monitoring_gr_db: f32,
    pub(super) meter_peak_db: f32,
    pub(super) meter_gr_db: f32,
    /// Per-channel ISP (inter-sample true peak) in linear, tracked across blocks
    pub(super) monitoring_isp_linear: Vec<f32>,
    /// Per-channel peak scratch for the current frame.
    pub(super) channel_peaks: Vec<f32>,
    /// Per-channel gain-reduction envelopes for independent/partial linking.
    pub(super) channel_envelopes: Vec<f32>,
    pub(super) sliding_maxima: Vec<SlidingMaximum>,
}

impl NativeKernel {
    pub(super) fn new(
        channels: usize,
        threshold_db: f32,
        release_ms: f32,
        lookahead_ms: f32,
        max_lookahead_len: usize,
    ) -> Self {
        let sr = 44100;
        let lookahead_len = (lookahead_ms.max(0.0) * 0.001 * sr as f32) as usize;
        let detector_delay = Bs1770TruePeakDetector::detector_delay_samples(sr);
        // Reserve the detector delay plus a full interpolation support of preview.
        let isp_delay_len = 3 * detector_delay;
        Self {
            threshold_db_smoother: Smoother::new(threshold_db, 5.0, sr),
            mix_smoother: Smoother::new(1.0, 5.0, sr),
            envelope: 0.0,
            release_coeff: 0.0,
            lookahead_buffer: vec![0.0; max_lookahead_len * channels],
            lookahead_pos: 0,
            lookahead_len,
            true_peak_detectors: (0..channels)
                .map(|_| Bs1770TruePeakDetector::new(sr))
                .collect(),
            output_isp_detectors: (0..channels)
                .map(|_| Bs1770TruePeakDetector::new(sr))
                .collect(),
            isp_correction_db: 0.0,
            isp_delay_buffer: vec![0.0; isp_delay_len * channels],
            isp_delay_pos: 0,
            isp_delay_len,
            isp_maxima: (0..channels)
                .map(|_| SlidingMaximum::new(isp_delay_len + detector_delay + 1))
                .collect(),
            isp_peaks: vec![0.0; channels],
            isp_gains: vec![1.0; channels],
            dual_release_env: DualRelease::new(release_ms, release_ms * 5.0, sr),
            channel_dual_release: (0..channels)
                .map(|_| DualRelease::new(release_ms, release_ms * 5.0, sr))
                .collect(),
            cache_update_counter: 0,
            monitoring_peak_db: -100.0,
            monitoring_gr_db: 0.0,
            meter_peak_db: -100.0,
            meter_gr_db: 0.0,
            monitoring_isp_linear: vec![0.0; channels],
            channel_peaks: vec![0.0; channels],
            channel_envelopes: vec![0.0; channels],
            sliding_maxima: (0..channels.max(1))
                .map(|_| SlidingMaximum::new(max_lookahead_len))
                .collect(),
        }
    }

    pub(super) fn update_coefficients(
        &mut self,
        channels: usize,
        sample_rate: u32,
        release_ms: f32,
        lookahead_ms: f32,
        required_capacity: usize,
        initialized: bool,
    ) {
        self.release_coeff = (-1.0 / (release_ms * 0.001 * sample_rate as f32)).exp();
        let new_len = (lookahead_ms.max(0.0) * 0.001 * sample_rate as f32) as usize;
        let required_buffer_capacity = required_capacity * channels;
        if !initialized && self.lookahead_buffer.len() < required_buffer_capacity {
            self.lookahead_buffer.resize(required_buffer_capacity, 0.0);
        }
        if new_len != self.lookahead_len {
            self.lookahead_len = new_len;
            self.lookahead_buffer.fill(0.0);
            self.lookahead_pos = 0;
        }
        self.dual_release_env
            .set_times(release_ms, release_ms * 5.0, sample_rate);
        for release in &mut self.channel_dual_release {
            release.set_times(release_ms, release_ms * 5.0, sample_rate);
        }
    }

    pub(super) fn initialize(
        &mut self,
        channels: usize,
        sample_rate: u32,
        release_ms: f32,
        capacity: usize,
    ) {
        let detector_delay = Bs1770TruePeakDetector::detector_delay_samples(sample_rate);
        self.threshold_db_smoother.set_time(5.0, sample_rate);
        self.mix_smoother.set_time(5.0, sample_rate);
        // Resize true peak detectors if channel count changed
        self.true_peak_detectors
            .resize_with(channels, || Bs1770TruePeakDetector::new(sample_rate));
        self.output_isp_detectors
            .resize_with(channels, || Bs1770TruePeakDetector::new(sample_rate));
        for detector in &mut self.true_peak_detectors {
            detector.set_sample_rate(sample_rate);
        }
        for detector in &mut self.output_isp_detectors {
            detector.set_sample_rate(sample_rate);
        }
        self.channel_peaks.resize(channels, 0.0);
        self.channel_envelopes.resize(channels, 0.0);
        self.monitoring_isp_linear.resize(channels, 0.0);
        self.isp_correction_db = 0.0;
        // Three detector delays give the output stage a full interpolation
        // support of preview after accounting for the detector's own delay.
        self.isp_delay_len = 3 * detector_delay;
        self.isp_delay_buffer = vec![0.0; self.isp_delay_len * channels];
        self.isp_maxima = (0..channels)
            .map(|_| SlidingMaximum::new(self.isp_delay_len + detector_delay + 1))
            .collect();
        self.isp_peaks.resize(channels, 0.0);
        self.isp_gains.resize(channels, 1.0);
        self.dual_release_env = DualRelease::new(release_ms, release_ms * 5.0, sample_rate);
        self.channel_dual_release = (0..channels)
            .map(|_| DualRelease::new(release_ms, release_ms * 5.0, sample_rate))
            .collect();
        self.sliding_maxima = (0..channels.max(1))
            .map(|_| SlidingMaximum::new(capacity))
            .collect();
    }

    pub(super) fn reset(&mut self) {
        self.envelope = 0.0;
        self.channel_envelopes.fill(0.0);
        self.lookahead_buffer.fill(0.0);
        self.lookahead_pos = 0;
        for det in &mut self.true_peak_detectors {
            det.reset();
        }
        for det in &mut self.output_isp_detectors {
            det.reset();
        }
        self.isp_correction_db = 0.0;
        self.isp_delay_buffer.fill(0.0);
        self.isp_delay_pos = 0;
        self.isp_peaks.fill(0.0);
        self.isp_gains.fill(1.0);
        for maximum in &mut self.isp_maxima {
            maximum.reset();
        }
        self.dual_release_env.reset();
        for release in &mut self.channel_dual_release {
            release.reset();
        }
        self.cache_update_counter = 0;
        self.monitoring_peak_db = -100.0;
        self.monitoring_gr_db = 0.0;
        self.meter_peak_db = -100.0;
        self.meter_gr_db = 0.0;
        self.monitoring_isp_linear.fill(0.0);
        for maximum in &mut self.sliding_maxima {
            maximum.reset();
        }
        let threshold = self.threshold_db_smoother.target();
        self.threshold_db_smoother.reset(threshold);
        let mix = self.mix_smoother.target();
        self.mix_smoother.reset(mix);
    }

    pub(super) fn process_stream(
        &mut self,
        buffer: &mut [f32],
        context: &ProcessContext,
        controls: KernelControls,
        cache: &mut RealTimeCache<LimiterData>,
    ) -> PluginResult<usize> {
        self.process_observed(
            buffer,
            context,
            controls,
            Some(cache),
            |_, _, _, _, _, _| {},
        )
    }

    /// Observe actual first-stage and output-guard gains without a second registry.
    /// The callback receives (frame, channel, ISP stage, ISP ring position, input, output).
    pub(super) fn process_observed(
        &mut self,
        buffer: &mut [f32],
        context: &ProcessContext,
        controls: KernelControls,
        mut cache: Option<&mut RealTimeCache<LimiterData>>,
        mut observe: impl FnMut(usize, usize, bool, usize, f32, f32),
    ) -> PluginResult<usize> {
        enable_ftz_daz();
        let num_frames = context.num_frames;
        let expected_len = num_frames
            .checked_mul(controls.channels)
            .ok_or_else(|| "limiter buffer length overflow".to_string())?;
        if controls.channels == 0 {
            return Err("limiter requires at least one channel".into());
        }
        if buffer.len() != expected_len {
            return Err(format!(
                "limiter expected {expected_len} samples, got {}",
                buffer.len()
            ));
        }
        let use_true_peak = controls.true_peak || controls.isp_mode;
        let use_dual_release = controls.dual_release;
        // Any non-zero pre-delay must use the upcoming window; otherwise gain
        // releases before the delayed transient reaches the output.
        let use_feed_forward = self.lookahead_len > 0;
        let use_isp_mode = controls.isp_mode;
        let isp_window = self.isp_delay_len
            + Bs1770TruePeakDetector::detector_delay_samples(context.sample_rate)
            + 1;

        let link = controls.link_amount;
        let meter_interval = (context.sample_rate as usize / CACHE_UPDATE_THROTTLE.max(1)).max(1);

        #[allow(clippy::needless_range_loop)]
        for frame in 0..num_frames {
            let thresh = fast_pow10(self.threshold_db_smoother.advance() / 20.0);
            let mix = self.mix_smoother.advance();

            // Detect per-channel peaks using pre-allocated scratch.
            let nc = controls.channels;
            self.channel_peaks[..nc].fill(0.0);
            if use_true_peak {
                for ch in 0..nc {
                    let idx = frame * controls.channels + ch;
                    let sample = if buffer[idx].is_finite() {
                        buffer[idx]
                    } else {
                        0.0
                    };
                    buffer[idx] = sample;
                    let tp = self.true_peak_detectors[ch].process_linear(sample);
                    self.channel_peaks[ch] = tp;
                    // Track per-channel ISP
                    if tp > self.monitoring_isp_linear[ch] {
                        self.monitoring_isp_linear[ch] = tp;
                    }
                }
            } else {
                for ch in 0..nc {
                    let idx = frame * controls.channels + ch;
                    if !buffer[idx].is_finite() {
                        buffer[idx] = 0.0;
                    }
                    self.channel_peaks[ch] = buffer[idx].abs();
                }
            }

            // Apply channel linking: blend each channel's detector toward the
            // strict linked maximum. At link=0, each channel retains its own
            // detector and therefore its own gain-reduction history.
            let max_peak_ch = self.channel_peaks[..nc]
                .iter()
                .copied()
                .fold(0.0f32, f32::max);
            let fully_linked = link >= 1.0 || nc <= 1;
            let linked_peak = if fully_linked {
                max_peak_ch
            } else {
                let avg_peak = self.channel_peaks[..nc].iter().copied().sum::<f32>() / nc as f32;
                avg_peak * (1.0 - link) + max_peak_ch * link
            };
            for ch in 0..nc {
                if fully_linked {
                    self.channel_peaks[ch] = max_peak_ch;
                } else {
                    self.channel_peaks[ch] =
                        self.channel_peaks[ch] * (1.0 - link) + linked_peak * link;
                }
            }

            let linked_window_peak = (use_feed_forward && fully_linked)
                .then(|| self.sliding_maxima[0].push(self.channel_peaks[0], self.lookahead_len));

            // Full linking has one gain history and advances the program-
            // dependent release once per frame, regardless of channel count.
            // The previous maximum also preserves attenuation when automation
            // switches from independent channels to full linking.
            let envelope_count = if fully_linked {
                self.channel_envelopes[0] = self.envelope;
                1
            } else {
                nc
            };
            for ch in 0..envelope_count {
                let effective_peak = if use_feed_forward {
                    linked_window_peak.unwrap_or_else(|| {
                        self.sliding_maxima[ch].push(self.channel_peaks[ch], self.lookahead_len)
                    })
                } else {
                    self.channel_peaks[ch]
                };
                let over_db = 20.0 * fast_log10(effective_peak.max(1.0e-20) / thresh);
                let target_gr = if controls.soft {
                    const KNEE_DB: f32 = 1.0;
                    if over_db <= -KNEE_DB * 0.5 {
                        0.0
                    } else if over_db >= KNEE_DB * 0.5 {
                        over_db
                    } else {
                        (over_db + KNEE_DB * 0.5).powi(2) / (2.0 * KNEE_DB)
                    }
                } else {
                    over_db.max(0.0)
                };
                let envelope = &mut self.channel_envelopes[ch];
                if target_gr > *envelope {
                    *envelope = target_gr;
                } else {
                    let rc = if use_dual_release {
                        if fully_linked {
                            self.dual_release_env.process(*envelope)
                        } else {
                            self.channel_dual_release[ch].process(*envelope)
                        }
                    } else {
                        self.release_coeff
                    };
                    *envelope = target_gr + rc * (*envelope - target_gr);
                }
            }
            if fully_linked {
                let shared_envelope = self.channel_envelopes[0];
                self.channel_envelopes[..nc].fill(shared_envelope);
            }
            self.envelope = self.channel_envelopes[..nc]
                .iter()
                .copied()
                .fold(0.0, f32::max);

            for ch in 0..controls.channels {
                let idx = frame * controls.channels + ch;
                let input_sample = buffer[idx];

                let delayed = if self.lookahead_len == 0 {
                    input_sample
                } else {
                    let buf_idx = self.lookahead_pos * controls.channels + ch;
                    let delayed = self.lookahead_buffer[buf_idx];
                    self.lookahead_buffer[buf_idx] = input_sample;
                    delayed
                };

                let gain = fast_pow10(-self.channel_envelopes[ch] / 20.0);
                let wet = (delayed * gain).clamp(-thresh, thresh);
                observe(frame, ch, false, self.isp_delay_pos, delayed, wet);

                buffer[idx] = (1.0 - mix) * delayed + mix * wet;
            }
            // Reconstruct the already gain-modulated audio before it leaves
            // the output delay. Input peak detection alone cannot predict the
            // new inter-sample peaks created by a changing gain envelope.
            if use_isp_mode {
                for ch in 0..controls.channels {
                    let idx = frame * controls.channels + ch;
                    let output_tp = self.output_isp_detectors[ch].process_linear(buffer[idx]);
                    self.isp_peaks[ch] = self.isp_maxima[ch].push(output_tp, isp_window);
                }
                if fully_linked {
                    let peak = self.isp_peaks.iter().copied().fold(0.0, f32::max);
                    self.isp_peaks.fill(peak);
                    let gain = self.isp_gains.iter().copied().fold(1.0, f32::min);
                    self.isp_gains.fill(gain);
                }
                let mut min_gain = 1.0_f32;
                for ch in 0..controls.channels {
                    let target_gain = if self.isp_peaks[ch] > thresh {
                        thresh / self.isp_peaks[ch]
                    } else {
                        1.0
                    };
                    let gain = self.isp_gains[ch];
                    let gain = if target_gain < gain {
                        target_gain
                    } else {
                        target_gain + self.release_coeff * (gain - target_gain)
                    };
                    self.isp_gains[ch] = gain;
                    min_gain = min_gain.min(gain);
                    let idx = frame * controls.channels + ch;
                    let delayed = if self.isp_delay_len > 0 {
                        let delay_idx = self.isp_delay_pos * controls.channels + ch;
                        let sample = self.isp_delay_buffer[delay_idx];
                        self.isp_delay_buffer[delay_idx] = buffer[idx];
                        sample
                    } else {
                        buffer[idx]
                    };
                    buffer[idx] = (delayed * gain).clamp(-thresh, thresh);
                    observe(frame, ch, true, self.isp_delay_pos, delayed, buffer[idx]);
                }
                if self.isp_delay_len > 0 {
                    self.isp_delay_pos = (self.isp_delay_pos + 1) % self.isp_delay_len;
                }
                self.isp_correction_db = -20.0 * fast_log10(min_gain.max(1.0e-20));
            }

            if self.lookahead_len > 0 {
                self.lookahead_pos = (self.lookahead_pos + 1) % self.lookahead_len;
            }

            let frame_peak = self.channel_peaks[..nc]
                .iter()
                .copied()
                .fold(0.0_f32, f32::max);
            self.monitoring_peak_db = 20.0 * fast_log10(frame_peak.max(1.0e-10));
            self.monitoring_gr_db = self.envelope + self.isp_correction_db;
            self.meter_peak_db = self.meter_peak_db.max(self.monitoring_peak_db);
            self.meter_gr_db = self.meter_gr_db.max(self.monitoring_gr_db);
            self.cache_update_counter += 1;
            if self.cache_update_counter >= meter_interval {
                self.cache_update_counter = 0;
                if let Some(cache) = cache.as_deref_mut() {
                    cache.update(|d| {
                        d.gain_reduction_db = self.meter_gr_db;
                        d.peak_db = self.meter_peak_db;
                        d.is_limiting = self.meter_gr_db > 0.01;
                        if d.isp_dbtp.len() == controls.channels {
                            if use_true_peak {
                                for (ch, &lin) in self.monitoring_isp_linear.iter().enumerate() {
                                    d.isp_dbtp[ch] = if lin < 1e-12 {
                                        -120.0
                                    } else {
                                        20.0 * lin.log10()
                                    };
                                }
                            } else {
                                d.isp_dbtp.fill(-120.0);
                            }
                        }
                    });
                }
                self.meter_peak_db = -100.0;
                self.meter_gr_db = 0.0;
                self.monitoring_isp_linear.fill(0.0);
            }
        }

        flush_denormals_inplace(buffer);
        Ok(num_frames)
    }
}

#[cfg(test)]
mod remediation_tests {
    use super::*;
    use crate::limiter_plugin::LimiterPlugin;
    use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin};

    // Independent f64 reconstruction of libebur128's 49-tap coefficient
    // generator. It deliberately does not read either production kernel.
    fn oracle_coefficient(factor: usize, past_offset: usize, phase: usize) -> f64 {
        let j = factor * past_offset + phase;
        if j > 48 {
            return 0.0;
        }
        let m = j as f64 - 24.0;
        let x = m * std::f64::consts::PI / factor as f64;
        let sinc = if m.abs() <= 1.0e-6 { 1.0 } else { x.sin() / x };
        let window = 0.5 * (1.0 - (2.0 * std::f64::consts::PI * j as f64 / 48.0).cos());
        sinc * window
    }

    struct OracleDetector {
        history: [f64; TRUE_PEAK_HISTORY],
        factor: u8,
        kernels: [[f64; TRUE_PEAK_HISTORY]; 4],
    }

    impl OracleDetector {
        fn new(sample_rate: u32) -> Self {
            let factor = if sample_rate < 96_000 {
                4
            } else if sample_rate < 192_000 {
                2
            } else {
                1
            };
            Self {
                history: [0.0; TRUE_PEAK_HISTORY],
                factor,
                kernels: std::array::from_fn(|phase| {
                    std::array::from_fn(|past| oracle_coefficient(factor as usize, past, phase))
                }),
            }
        }

        fn process(&mut self, sample: f32) -> f64 {
            if self.factor == 1 {
                return sample.abs() as f64;
            }
            self.history.copy_within(..TRUE_PEAK_HISTORY - 1, 1);
            self.history[0] = sample as f64;
            let factor = self.factor as usize;
            (0..factor)
                .map(|phase| {
                    self.history
                        .iter()
                        .zip(self.kernels[phase])
                        .map(|(value, coefficient)| coefficient * value)
                        .sum::<f64>()
                        .abs()
                })
                .fold(0.0, f64::max)
        }
    }

    fn oracle_stream_peak(signal: &[f32], sample_rate: u32) -> f64 {
        let mut detector = OracleDetector::new(sample_rate);
        let mut peak = 0.0f64;
        for &sample in signal {
            peak = peak.max(detector.process(sample));
        }
        for _ in 0..TRUE_PEAK_HISTORY {
            peak = peak.max(detector.process(0.0));
        }
        peak
    }

    #[test]
    fn monotonic_window_matches_naive_maximum() {
        let values = [0.2, 0.8, 0.4, 0.1, 0.9, 0.3, 0.7, 0.6, 0.05];
        for window in 1..=values.len() {
            let mut maximum = SlidingMaximum::new(values.len());
            for (index, &value) in values.iter().enumerate() {
                let got = maximum.push(value, window);
                let begin = (index + 1).saturating_sub(window);
                let expected = values[begin..=index].iter().copied().fold(0.0, f32::max);
                assert_eq!(got, expected, "window {window}, index {index}");
            }
        }
    }

    #[test]
    fn bs1770_hann_sinc_impulse_matches_independent_fixed_coefficient_oracle() {
        let mut phase_detector = Bs1770TruePeakDetector::new(48_000);
        let mut peak_detector = Bs1770TruePeakDetector::new(48_000);
        for frame in 0..13 {
            let sample = if frame == 0 { 1.0 } else { 0.0 };
            let actual_phases = phase_detector.push_and_interpolate_4x(sample);
            for (phase, &actual_phase) in actual_phases.iter().enumerate() {
                let expected = oracle_coefficient(4, frame, phase) as f32;
                assert!(
                    (actual_phase - expected).abs() < 2.0e-7,
                    "impulse frame {frame}, phase {phase}: expected {expected}, got {}",
                    actual_phase
                );
            }
            let expected_peak = (0..4)
                .map(|phase| oracle_coefficient(4, frame, phase).abs() as f32)
                .fold(0.0, f32::max);
            assert!(
                (peak_detector.process_linear(sample) - expected_peak).abs() < 2.0e-7,
                "impulse frame {frame} peak"
            );
        }
        assert!((oracle_coefficient(4, 6, 1) - 0.896_465_150_711).abs() < 1.0e-12);
        assert_eq!(oracle_coefficient(4, 6, 0), 1.0);
        assert_eq!(Bs1770TruePeakDetector::detector_delay_samples(48_000), 6);
        assert_eq!(Bs1770TruePeakDetector::detector_delay_samples(96_000), 12);
        assert_eq!(Bs1770TruePeakDetector::detector_delay_samples(192_000), 0);
        assert_eq!(phase_detector.process_linear(0.0), 0.0);

        let mut two_x_detector = Bs1770TruePeakDetector::new(96_000);
        for frame in 0..25 {
            let sample = if frame == 0 { 1.0 } else { 0.0 };
            let expected = (0..2)
                .map(|phase| oracle_coefficient(2, frame, phase).abs() as f32)
                .fold(0.0, f32::max);
            let actual = two_x_detector.process_linear(sample);
            assert!(
                (actual - expected).abs() < 2.0e-7,
                "2x impulse frame {frame}: actual={actual}, expected={expected}"
            );
        }
    }

    #[test]
    fn bs1770_high_frequency_phase_sweep_meets_rate_appropriate_error_bounds() {
        let amplitude = 10.0_f32.powf(-3.0 / 20.0);
        for sample_rate in [44_100_u32, 48_000, 96_000, 192_000] {
            // 12 kHz is the high-frequency BS.1770/EBU conformance operating
            // point and remains below the transition band at every rate.
            let frequency = 12_000.0f32;
            for phase_index in 0..32 {
                let phase = std::f32::consts::TAU * phase_index as f32 / 32.0;
                let mut detector = Bs1770TruePeakDetector::new(sample_rate);
                let mut peak = 0.0_f32;
                for frame in 0..8192 {
                    let sample = amplitude
                        * (std::f32::consts::TAU * frequency * frame as f32 / sample_rate as f32
                            + phase)
                            .sin();
                    let detected = detector.process_linear(sample);
                    if frame >= 64 {
                        peak = peak.max(detected);
                    }
                }
                let error_db = 20.0 * (peak / amplitude).log10();
                assert!(
                    (-0.5..=0.2).contains(&error_db),
                    "{sample_rate} Hz, {frequency} Hz tone, phase {phase_index}: {peak} vs {amplitude} ({error_db} dB)"
                );
            }
        }
    }

    #[test]
    fn isp_alignment_requirement_tracks_rate_dependent_detector_delay() {
        for (sample_rate, lookahead_ms, should_initialize) in [
            (48_000, 0.10, false), // 4 samples cannot cover the 6-sample delay
            (48_000, 0.13, true),  // 6 samples
            (96_000, 0.10, false), // 9 samples cannot cover the 12-sample delay
            (96_000, 0.13, true),  // 12 samples
            (192_000, 0.0, true),  // native sample peak has no filter delay
        ] {
            let mut plugin = LimiterPlugin::new(1, -6.0, 50.0, lookahead_ms, false);
            plugin.true_peak = true;
            plugin.isp_mode = true;
            let result = plugin.initialize(sample_rate);
            assert_eq!(
                result.is_ok(),
                should_initialize,
                "sample_rate={sample_rate}, lookahead_ms={lookahead_ms}: {result:?}"
            );
        }
    }

    #[test]
    fn isp_limiter_holds_the_whole_output_below_ceiling_by_independent_dbtp_oracle() {
        let threshold_db = -6.0f32;
        let allowed_peak = 10.0f64.powf((threshold_db + 0.1) as f64 / 20.0);
        for sample_rate in [44_100_u32, 48_000, 96_000, 192_000] {
            let frequency = 18_000.0f32.min(sample_rate as f32 * 0.4);
            for phase_index in 0..8 {
                let mut plugin = LimiterPlugin::new(1, threshold_db, 50.0, 5.0, false);
                plugin.true_peak = true;
                plugin.isp_mode = true;
                plugin.rebuild_cached_parameters();
                plugin.initialize(sample_rate).unwrap();

                let programme_frames = 4096usize;
                let tail = plugin.latency_samples() + TRUE_PEAK_HISTORY;
                let phase = std::f32::consts::TAU * phase_index as f32 / 8.0;
                let mut output = vec![0.0f32; programme_frames + tail];
                for (frame, sample) in output[..programme_frames].iter_mut().enumerate() {
                    *sample = 1.2
                        * (std::f32::consts::TAU * frequency * frame as f32 / sample_rate as f32
                            + phase)
                            .sin();
                }
                let frames = output.len();
                plugin
                    .process_in_place(&mut output, &ProcessContext::new(sample_rate, frames))
                    .unwrap();

                let output_peak = oracle_stream_peak(&output, sample_rate);
                assert!(
                    output_peak <= allowed_peak,
                    "{sample_rate} Hz, phase {phase_index}: whole-output peak {:.3} dBTP exceeds {:.1} dBTP ceiling + 0.1 dB",
                    20.0 * output_peak.log10(),
                    threshold_db
                );
            }
        }
    }

    #[test]
    fn isp_corrects_bursts_and_two_tones_at_minimum_lookahead() {
        let ceiling_db = -6.0_f32;
        let allowed_peak = 10.0_f64.powf((ceiling_db as f64 + 0.1) / 20.0);
        for sample_rate in [44_100, 48_000, 88_200, 96_000, 192_000] {
            let delay = Bs1770TruePeakDetector::detector_delay_samples(sample_rate);
            // The extra microsecond avoids rounding the requested integer
            // number of input frames down when converting milliseconds.
            let minimum_lookahead = delay as f32 * 1000.0 / sample_rate as f32 + 0.001;
            for lookahead_ms in [minimum_lookahead, 5.0] {
                for release_ms in [10.0, 50.0] {
                    for two_tones in [false, true] {
                        for phase in 0..8 {
                            let mut plugin =
                                LimiterPlugin::new(1, ceiling_db, release_ms, lookahead_ms, false);
                            plugin.isp_mode = true;
                            plugin.initialize(sample_rate).unwrap();
                            let programme_frames = 4096;
                            let mut output =
                                vec![
                                    0.0;
                                    programme_frames + plugin.latency_samples() + TRUE_PEAK_HISTORY
                                ];
                            for (frame, sample) in output[..programme_frames].iter_mut().enumerate()
                            {
                                let angle = std::f64::consts::TAU
                                    * (12_000.0 * frame as f64 / sample_rate as f64
                                        + phase as f64 / 8.0);
                                *sample = if two_tones {
                                    if (frame / 79) % 3 < 2 {
                                        (angle.sin() + (angle * 1.317).cos()) as f32
                                    } else {
                                        0.0
                                    }
                                } else if (frame / 137) % 2 == 0 {
                                    (1.2 * angle.sin()) as f32
                                } else {
                                    0.0
                                };
                            }
                            for block in output.chunks_mut(127) {
                                plugin
                                    .process_in_place(
                                        block,
                                        &ProcessContext::new(sample_rate, block.len()),
                                    )
                                    .unwrap();
                            }
                            let peak = oracle_stream_peak(&output, sample_rate);
                            assert!(
                                peak <= allowed_peak,
                                "final output {:.5} dBTP, rate={sample_rate}, lookahead={lookahead_ms}, release={release_ms}, two_tones={two_tones}, phase={phase}",
                                20.0 * peak.log10()
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn isp_preserves_ceiling_utilization_for_a_steady_tone() {
        let sample_rate = 48_000;
        let mut plugin = LimiterPlugin::new(1, -6.0, 10.0, 0.13, false);
        plugin.isp_mode = true;
        plugin.initialize(sample_rate).unwrap();
        let mut output: Vec<f32> = (0..24_000)
            .map(|frame| (1.2 * (std::f64::consts::TAU * frame as f64 / 48.0).sin()) as f32)
            .collect();
        let frames = output.len();
        plugin
            .process_in_place(&mut output, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        let rms = (output[12_000..]
            .iter()
            .map(|&x| (x as f64).powi(2))
            .sum::<f64>()
            / 12_000.0)
            .sqrt();
        let ideal_rms = 10.0_f64.powf(-6.0 / 20.0) / std::f64::consts::SQRT_2;
        assert!(
            (20.0 * (rms / ideal_rms).log10()).abs() < 0.1,
            "output correction unnecessarily reduced a steady tone: {rms} versus {ideal_rms}"
        );
    }

    #[test]
    fn isp_threshold_steps_follow_the_smoothed_ceiling_in_final_output() {
        for sample_rate in [48_000, 96_000, 192_000] {
            let mut plugin = LimiterPlugin::new(1, -6.0, 10.0, 0.13, false);
            plugin.isp_mode = true;
            plugin.initialize(sample_rate).unwrap();
            let latency = plugin.latency_samples();
            let detector_delay = Bs1770TruePeakDetector::detector_delay_samples(sample_rate);
            let mut oracle = OracleDetector::new(sample_rate);
            let mut ceilings = Vec::new();
            for frame in 0..12_000 + latency + TRUE_PEAK_HISTORY {
                if frame == 4000 || frame == 8000 {
                    let target = if frame == 4000 { -18.0 } else { -3.0 };
                    plugin
                        .set_parameter(
                            ParameterId::from("threshold"),
                            ParameterValue::Float(target),
                        )
                        .unwrap();
                }
                let angle = std::f64::consts::TAU * 12_000.0 * frame as f64 / sample_rate as f64;
                let sample = if frame < 12_000 {
                    (angle.sin() + (1.317 * angle).cos()) as f32
                } else {
                    0.0
                };
                let mut output = [sample];
                plugin
                    .process_in_place(&mut output, &ProcessContext::new(sample_rate, 1))
                    .unwrap();
                ceilings.push(
                    10.0_f64.powf(plugin.kernel.threshold_db_smoother.current() as f64 / 20.0),
                );
                let peak = oracle.process(output[0]);
                let center = frame.saturating_sub(detector_delay);
                // The oracle's phases occupy [center, center + 1). During a
                // moving ceiling, compare against the larger interval endpoint.
                let ceiling = ceilings[center].max(ceilings[(center + 1).min(frame)]);
                assert!(
                    20.0 * (peak / ceiling).log10() <= 0.1,
                    "ISP exceeded the changing ceiling at rate={sample_rate}, frame={frame}, output={peak}, ceiling={ceiling}"
                );
                assert_eq!(plugin.latency_samples(), latency);
            }
        }
    }
}
