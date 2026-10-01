use super::native_kernel::{Bs1770TruePeakDetector, KernelControls, NativeKernel};
use super::oversampled_core::Controls;
use super::oversampled_path::OversampledPath;
use super::types::LimiterData;
use super::types::LimiterPluginParams;
use crate::params::PARAMS as LM;
use sotf_host::ParametricInPlacePlugin;
use sotf_host::analyzer::RealTimeCache;
use sotf_host::param_specs::{UpdateMode, find_by_key as pk};
use sotf_host::parameters::{Parameter, ParameterId, ParameterImportance, ParameterValue};
use sotf_host::parametric_plugin::{ParameterSchema, ParameterSet};
use sotf_host::plugin::{
    PluginCompileMetadata, PluginCompiledOp, PluginCostClass, PluginDrainResult, PluginInfo,
    PluginResult, ProcessContext, TailLength,
};
use std::any::Any;
use std::sync::Arc;

// Bound one drain callback independently of the configured lookahead duration.
const MAX_DRAIN_FRAMES: usize = 256;

pub struct LimiterPlugin {
    pub(super) channels: usize,
    pub(super) sample_rate: u32,
    initialized: bool,
    oversampling: usize,
    oversampled: Option<Box<OversampledPath>>,
    has_input: bool,
    /// `Some(0)` is complete; either `Some` value requires reset before new input.
    drain_remaining: Option<usize>,
    pub(super) param_threshold: ParameterId,
    pub(super) threshold_db: f32,
    pub(super) param_release: ParameterId,
    pub(super) release_ms: f32,
    pub(super) param_lookahead: ParameterId,
    pub(super) lookahead_ms: f32,
    pub(super) param_soft: ParameterId,
    pub(super) soft: bool,
    pub(super) param_true_peak: ParameterId,
    pub(super) true_peak: bool,
    pub(super) param_isp_mode: ParameterId,
    pub(super) isp_mode: bool,
    pub(super) param_dual_release: ParameterId,
    pub(super) dual_release: bool,
    pub(super) param_mix: ParameterId,
    pub(super) mix: f32,
    pub(super) param_feed_forward: ParameterId,
    pub(super) feed_forward: bool,
    pub(super) param_link_amount: ParameterId,
    pub(super) link_amount: f32,
    pub(super) kernel: NativeKernel,
    pub(super) cached_parameters: Vec<Parameter>,
    pub(super) cache: RealTimeCache<LimiterData>,
}

impl LimiterPlugin {
    pub fn new(
        channels: usize,
        threshold_db: f32,
        release_ms: f32,
        lookahead_ms: f32,
        soft: bool,
    ) -> Self {
        let mut p = Self {
            channels,
            sample_rate: 44100,
            initialized: false,
            oversampling: 0,
            oversampled: None,
            has_input: false,
            drain_remaining: None,
            param_threshold: ParameterId::from("threshold"),
            threshold_db,
            param_release: ParameterId::from("release"),
            release_ms,
            param_lookahead: ParameterId::from("lookahead"),
            lookahead_ms,
            param_soft: ParameterId::from("soft"),
            soft,
            param_true_peak: ParameterId::from("true_peak"),
            true_peak: false,
            param_isp_mode: ParameterId::from("isp_mode"),
            isp_mode: false,
            param_dual_release: ParameterId::from("dual_release"),
            dual_release: false,
            param_mix: ParameterId::from("mix"),
            mix: 1.0,
            param_feed_forward: ParameterId::from("feed_forward"),
            feed_forward: false,
            param_link_amount: ParameterId::from("link_amount"),
            link_amount: pk(LM, "link_amount").default_f64() as f32,
            kernel: NativeKernel::new(
                channels,
                threshold_db,
                release_ms,
                lookahead_ms,
                Self::max_lookahead_len(44100),
            ),
            cached_parameters: Vec::new(),
            cache: RealTimeCache::new(LimiterData {
                isp_dbtp: vec![-120.0; channels],
                output_isp_dbtp: vec![-120.0; channels],
                ..LimiterData::default()
            }),
        };
        p.rebuild_cached_parameters();
        p
    }

    pub(super) fn max_lookahead_len(sample_rate: u32) -> usize {
        let max_ms = pk(LM, "lookahead").max_f64() as f32;
        ((max_ms * 0.001 * sample_rate as f32) as usize).max(1)
    }

    pub(super) fn rebuild_cached_parameters(&mut self) {
        self.cached_parameters = vec![
            Parameter::new_float(
                "threshold",
                "Threshold",
                self.threshold_db,
                pk(LM, "threshold").min_f64() as f32,
                pk(LM, "threshold").max_f64() as f32,
            )
            .with_description("Ceiling level (dB)")
            .with_group("Dynamics")
            .with_importance(ParameterImportance::Critical),
            Parameter::new_float(
                "release",
                "Release",
                self.release_ms,
                pk(LM, "release").min_f64() as f32,
                pk(LM, "release").max_f64() as f32,
            )
            .with_description("Release time (ms)")
            .with_group("Timing")
            .with_importance(ParameterImportance::Critical),
            Parameter::new_float(
                "lookahead",
                "Lookahead",
                self.lookahead_ms,
                pk(LM, "lookahead").min_f64() as f32,
                pk(LM, "lookahead").max_f64() as f32,
            )
            .with_description("Structural predictive lookahead / host latency (ms)")
            .with_group("Timing")
            .with_importance(ParameterImportance::Useful)
            .with_update_mode(UpdateMode::Structural),
            Parameter::new_bool("soft", "Soft", self.soft)
                .with_description("Use a one-dB gain-computer knee")
                .with_group("Dynamics")
                .with_importance(ParameterImportance::Useful),
            Parameter::new_bool("true_peak", "True Peak", self.true_peak)
                .with_description(
                    "Use rate-appropriate ITU-R BS.1770-compatible true-peak detection",
                )
                .with_group("Detection")
                .with_importance(ParameterImportance::Useful),
            Parameter::new_bool("isp_mode", "ISP Limit", self.isp_mode)
                .with_description("Predictive output ISP correction with additional host latency")
                .with_group("Detection")
                .with_importance(ParameterImportance::Useful)
                .with_update_mode(UpdateMode::Structural),
            Parameter::new_bool("dual_release", "Dual Release", self.dual_release)
                .with_description("Program-dependent fast/slow release")
                .with_group("Timing")
                .with_importance(ParameterImportance::Useful),
            Parameter::new_float(
                "mix",
                "Mix",
                self.mix,
                pk(LM, "mix").min_f64() as f32,
                pk(LM, "mix").max_f64() as f32,
            )
            .with_description("Dry/wet mix (0 = dry, 1 = limited)")
            .with_group("Output")
            .with_importance(ParameterImportance::Useful),
            // Must match PARAMS order: idx 8=link_amount, idx 9=feed_forward
            Parameter::new_float(
                "link_amount",
                "Link",
                self.link_amount,
                pk(LM, "link_amount").min_f64() as f32,
                pk(LM, "link_amount").max_f64() as f32,
            )
            .with_description("Channel linking (0=independent, 1=linked)")
            .with_group("Detection")
            .with_importance(ParameterImportance::Useful),
            Parameter::new_bool("feed_forward", "Feed Forward", self.feed_forward)
                .with_description("Compatibility flag; lookahead is always predictive")
                .with_group("Detection")
                .with_importance(ParameterImportance::Useful),
            Parameter::new_int(
                "oversampling",
                "Oversampling",
                self.oversampling as i32,
                0,
                2,
            )
            .with_description("Prepared audio-rate factor: 0=1x, 1=2x, 2=4x")
            .with_group("Quality")
            .with_update_mode(UpdateMode::Structural),
        ];
    }

    pub fn from_params(channels: usize, params: LimiterPluginParams) -> Self {
        let finite_or = |value: f32, key: &str| {
            if value.is_finite() {
                value
            } else {
                pk(LM, key).default_f64() as f32
            }
        };
        let threshold = finite_or(params.threshold_db, "threshold").clamp(
            pk(LM, "threshold").min_f64() as f32,
            pk(LM, "threshold").max_f64() as f32,
        );
        let release = finite_or(params.release_ms, "release").clamp(
            pk(LM, "release").min_f64() as f32,
            pk(LM, "release").max_f64() as f32,
        );
        let lookahead = finite_or(params.lookahead_ms, "lookahead").clamp(
            pk(LM, "lookahead").min_f64() as f32,
            pk(LM, "lookahead").max_f64() as f32,
        );
        let mut p = Self::new(channels, threshold, release, lookahead, params.soft);
        p.oversampling = params.oversampling;
        p.true_peak = params.true_peak;
        p.isp_mode = params.isp_mode;
        p.dual_release = params.dual_release;
        p.mix = finite_or(params.mix, "mix").clamp(0.0, 1.0);
        p.kernel.mix_smoother.reset(p.mix);
        p.feed_forward = params.feed_forward;
        p.link_amount = finite_or(params.link_amount, "link_amount").clamp(0.0, 1.0);
        p.rebuild_cached_parameters();
        p
    }

    pub(super) fn update_coefficients(&mut self) {
        self.kernel.update_coefficients(
            self.channels,
            self.sample_rate,
            self.release_ms,
            self.lookahead_ms,
            Self::max_lookahead_len(self.sample_rate),
            self.initialized,
        );
    }
}

impl LimiterPlugin {
    // Shared by bulk control-thread state restoration and allocation-free
    // single-parameter updates from realtime hosts.
    fn apply_value(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()> {
        if self.drain_remaining.is_some()
            || self
                .oversampled
                .as_ref()
                .is_some_and(|path| path.is_draining())
        {
            // Hosts may resend their unchanged snapshot between drain calls.
            // Do not retarget smoothers or modify delayed audio after EOS.
            return if self.parametric_get_parameter(&id).as_ref() == Some(&value) {
                Ok(())
            } else {
                Err("limiter requires reset before changing parameters after drain".into())
            };
        }
        if id.as_str() == "oversampling" {
            let choice = value
                .as_int()
                .filter(|choice| (0..=2).contains(choice))
                .ok_or_else(|| "oversampling requires choice 0, 1, or 2".to_string())?
                as usize;
            if self.initialized && choice != self.oversampling {
                return Err("oversampling changes latency and requires a graph rebuild".into());
            }
            self.oversampling = choice;
        } else if id == self.param_threshold {
            let val = value
                .as_float()
                .unwrap_or(pk(LM, "threshold").default_f64() as f32);
            if val.is_finite() {
                self.threshold_db = val;
                self.kernel
                    .threshold_db_smoother
                    .set_target(self.threshold_db);
            }
        } else if id == self.param_release {
            let val = value
                .as_float()
                .unwrap_or(pk(LM, "release").default_f64() as f32);
            if val.is_finite() {
                self.release_ms = val.max(1.0);
                self.update_coefficients();
            }
        } else if id == self.param_lookahead {
            if self.initialized {
                return Err("lookahead changes latency and requires a graph rebuild".into());
            }
            let val = value
                .as_float()
                .unwrap_or(pk(LM, "lookahead").default_f64() as f32);
            if val.is_finite() {
                self.lookahead_ms = val.max(0.0);
                self.update_coefficients();
            }
        } else if id == self.param_soft {
            let soft = value.as_bool().unwrap_or(pk(LM, "soft").default_bool());
            if soft && self.isp_mode {
                return Err("soft knee is unavailable in guaranteed ISP mode".into());
            }
            self.soft = soft;
        } else if id == self.param_true_peak {
            self.true_peak = value
                .as_bool()
                .unwrap_or(pk(LM, "true_peak").default_bool());
        } else if id == self.param_isp_mode {
            let enabled = value.as_bool().unwrap_or(pk(LM, "isp_mode").default_bool());
            let detector_delay = Bs1770TruePeakDetector::detector_delay_samples(self.sample_rate);
            if self.initialized && enabled != self.isp_mode && detector_delay > 0 {
                return Err("ISP mode changes latency and requires a graph rebuild".into());
            }
            if enabled
                && (self.mix < 1.0 || self.soft || self.kernel.lookahead_len < detector_delay)
            {
                return Err(format!(
                    "ISP mode requires 100% wet, hard limiting, and at least {detector_delay} lookahead samples at {} Hz",
                    self.sample_rate
                ));
            }
            self.isp_mode = enabled;
        } else if id == self.param_dual_release {
            self.dual_release = value
                .as_bool()
                .unwrap_or(pk(LM, "dual_release").default_bool());
        } else if id == self.param_mix {
            let val = value
                .as_float()
                .unwrap_or(pk(LM, "mix").default_f64() as f32);
            if val.is_finite() {
                if self.isp_mode && val < 1.0 {
                    return Err("ISP mode requires 100% wet mix".into());
                }
                self.mix = val.clamp(0.0, 1.0);
                self.kernel.mix_smoother.set_target(self.mix);
            }
        } else if id == self.param_feed_forward {
            self.feed_forward = value.as_bool().unwrap_or(false);
        } else if id == self.param_link_amount {
            let val = value.as_float().unwrap_or(1.0);
            if val.is_finite() {
                self.link_amount = val.clamp(0.0, 1.0);
            }
        } else {
            return Err(format!("Unknown parameter: {}", id));
        }
        Ok(())
    }
}

impl LimiterPlugin {
    // Shared unchanged DSP arithmetic for input processing and EOS zero continuation.
    fn process_stream(
        &mut self,
        buffer: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        self.kernel.process_stream(
            buffer,
            context,
            KernelControls {
                channels: self.channels,
                soft: self.soft,
                true_peak: self.true_peak,
                isp_mode: self.isp_mode,
                dual_release: self.dual_release,
                link_amount: self.link_amount,
            },
            &mut self.cache,
        )
    }
}

impl LimiterPlugin {
    fn controls(&self) -> Controls {
        Controls {
            threshold: self.threshold_db,
            release: self.release_ms,
            soft: self.soft,
            true_peak: self.true_peak,
            dual_release: self.dual_release,
            link: self.link_amount,
        }
    }
}

impl ParametricInPlacePlugin for LimiterPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Limiter", env!("CARGO_PKG_VERSION"), "SotF")
    }

    fn cost_class(&self) -> PluginCostClass {
        if self.oversampling == 0 {
            PluginCostClass::Dynamics
        } else {
            PluginCostClass::Fft
        }
    }

    fn channels(&self) -> usize {
        self.channels
    }
    fn parameter_schema(&self) -> ParameterSchema {
        self.cached_parameters.clone()
    }

    fn current_values(&self) -> ParameterSet {
        let mut values = ParameterSet::new();
        values.insert(
            self.param_threshold.clone(),
            ParameterValue::Float(self.threshold_db),
        );
        values.insert(
            self.param_release.clone(),
            ParameterValue::Float(self.release_ms),
        );
        values.insert(
            self.param_lookahead.clone(),
            ParameterValue::Float(self.lookahead_ms),
        );
        values.insert(self.param_soft.clone(), ParameterValue::Bool(self.soft));
        values.insert(
            self.param_true_peak.clone(),
            ParameterValue::Bool(self.true_peak),
        );
        values.insert(
            self.param_isp_mode.clone(),
            ParameterValue::Bool(self.isp_mode),
        );
        values.insert(
            self.param_dual_release.clone(),
            ParameterValue::Bool(self.dual_release),
        );
        values.insert(self.param_mix.clone(), ParameterValue::Float(self.mix));
        values.insert(
            self.param_feed_forward.clone(),
            ParameterValue::Bool(self.feed_forward),
        );
        values.insert(
            self.param_link_amount.clone(),
            ParameterValue::Float(self.link_amount),
        );
        values.insert(
            ParameterId::from("oversampling"),
            ParameterValue::Int(self.oversampling as i32),
        );
        values
    }

    fn parametric_validate_parameter(
        &self,
        id: &ParameterId,
        value: &ParameterValue,
    ) -> PluginResult<()> {
        self.cached_parameters
            .iter()
            .find(|parameter| &parameter.id == id)
            .ok_or_else(|| format!("Unknown parameter: {id}"))?
            .validate(value)
            .map_err(|error| format!("{id}: {error}"))
    }

    fn parametric_set_parameter(
        &mut self,
        id: ParameterId,
        value: ParameterValue,
    ) -> PluginResult<()> {
        self.parametric_validate_parameter(&id, &value)?;
        self.apply_value(id, value)
    }

    fn parametric_get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        match id.as_str() {
            "oversampling" => Some(ParameterValue::Int(self.oversampling as i32)),
            "threshold" => Some(ParameterValue::Float(self.threshold_db)),
            "release" => Some(ParameterValue::Float(self.release_ms)),
            "lookahead" => Some(ParameterValue::Float(self.lookahead_ms)),
            "soft" => Some(ParameterValue::Bool(self.soft)),
            "true_peak" => Some(ParameterValue::Bool(self.true_peak)),
            "isp_mode" => Some(ParameterValue::Bool(self.isp_mode)),
            "dual_release" => Some(ParameterValue::Bool(self.dual_release)),
            "mix" => Some(ParameterValue::Float(self.mix)),
            "feed_forward" => Some(ParameterValue::Bool(self.feed_forward)),
            "link_amount" => Some(ParameterValue::Float(self.link_amount)),
            _ => None,
        }
    }

    fn apply_values(&mut self, values: ParameterSet) -> PluginResult<()> {
        // Reject the new structural factor before the ordered legacy loop can
        // retarget mix or another earlier field. Legacy-only restoration keeps
        // its existing semantics; this is not a general transaction layer.
        if let Some(value) = values.get(&ParameterId::from("oversampling")) {
            let choice = value
                .as_int()
                .filter(|choice| (0..=2).contains(choice))
                .ok_or_else(|| "oversampling requires choice 0, 1, or 2".to_string())?
                as usize;
            if self.initialized && choice != self.oversampling {
                return Err("oversampling changes latency and requires a graph rebuild".into());
            }
        }
        for (id, value) in values {
            self.apply_value(id, value)?;
        }
        Ok(())
    }

    fn initialize(&mut self, sample_rate: u32) -> PluginResult<()> {
        if sample_rate == 0 || self.channels == 0 {
            return Err("limiter requires nonzero sample rate and channels".into());
        }
        if self.oversampling != 0 {
            let factor = match self.oversampling {
                1 => 2,
                2 => 4,
                _ => return Err("oversampling requires choice 0, 1, or 2".into()),
            };
            // Every fallible preparation precedes changes to the live path.
            let prepared = OversampledPath::prepare(
                self.channels,
                sample_rate,
                factor,
                self.lookahead_ms,
                self.controls(),
                self.isp_mode,
                self.mix,
            )?;
            self.oversampled = Some(Box::new(prepared));
            self.sample_rate = sample_rate;
            self.initialized = true;
            self.has_input = false;
            self.drain_remaining = None;
            // Retain the native quantization for existing structural validation.
            self.kernel.lookahead_len =
                (self.lookahead_ms.max(0.0) * 0.001 * sample_rate as f32) as usize;
            return Ok(());
        }
        self.sample_rate = sample_rate;
        self.initialized = false;
        self.update_coefficients();
        let detector_delay = Bs1770TruePeakDetector::detector_delay_samples(sample_rate);
        if self.isp_mode
            && (self.mix < 1.0 || self.soft || self.kernel.lookahead_len < detector_delay)
        {
            return Err(format!(
                "ISP mode requires 100% wet, hard limiting, and at least {detector_delay} lookahead samples at {sample_rate} Hz"
            ));
        }
        self.kernel.initialize(
            self.channels,
            sample_rate,
            self.release_ms,
            Self::max_lookahead_len(sample_rate),
        );
        self.initialized = true;
        self.reset();
        Ok(())
    }

    fn reset(&mut self) {
        let controls = self.controls();
        if let Some(path) = &mut self.oversampled {
            path.reset(controls, self.isp_mode, self.mix);
        }
        self.has_input = false;
        self.drain_remaining = None;
        self.kernel.reset();
    }

    fn process_in_place(
        &mut self,
        buffer: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        if !self.initialized || context.sample_rate != self.sample_rate {
            return Err("limiter requires initialization at the process sample rate".into());
        }
        let controls = self.controls();
        if let Some(path) = &mut self.oversampled {
            return path.process(
                buffer,
                context,
                controls,
                self.isp_mode,
                self.mix,
                &mut self.cache,
            );
        }
        if context.num_frames > 0 && self.drain_remaining.is_some() {
            return Err("limiter requires reset before processing input after drain".into());
        }
        let frames = self.process_stream(buffer, context)?;
        self.has_input |= frames > 0;
        Ok(frames)
    }

    fn drain_output_frames_max(&self) -> usize {
        if self.oversampling != 0 {
            return MAX_DRAIN_FRAMES;
        }
        self.latency_samples().min(MAX_DRAIN_FRAMES)
    }

    fn begin_drain(&mut self, context: &ProcessContext) -> PluginResult<()> {
        if self.oversampling == 0 {
            return Ok(());
        }
        let controls = self.controls();
        self.oversampled
            .as_mut()
            .ok_or_else(|| "oversampled limiter requires initialization".to_string())?
            .begin(context, controls, self.isp_mode, self.mix)
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        if self.oversampling != 0 {
            return self.oversampled.as_ref().and_then(|path| path.bound());
        }
        if !self.initialized {
            return None;
        }
        let remaining = if self.has_input {
            self.drain_remaining
                .unwrap_or_else(|| self.latency_samples())
        } else {
            0
        };
        // Every successful full-capacity call consumes up to 256 retained
        // frames, with completion on the final output call. Empty state still
        // needs one successful terminal call.
        std::num::NonZeroU64::new(remaining.div_ceil(MAX_DRAIN_FRAMES).max(1) as u64)
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<PluginDrainResult> {
        if !self.initialized || context.sample_rate != self.sample_rate {
            return Err("limiter requires initialization at the drain sample rate".into());
        }
        let controls = self.controls();
        if let Some(path) = &mut self.oversampled {
            return path.drain(
                output,
                context,
                controls,
                self.isp_mode,
                self.mix,
                &mut self.cache,
            );
        }
        if !output.len().is_multiple_of(self.channels) {
            return Err("limiter drain output must contain whole channel frames".into());
        }
        if !self.has_input || self.drain_remaining == Some(0) {
            return Ok(PluginDrainResult::COMPLETE);
        }
        // The only retained audio is in the active lookahead and ISP output
        // rings. Detector history and release envelopes cannot synthesize audio.
        let remaining = self
            .drain_remaining
            .unwrap_or_else(|| self.latency_samples());
        if remaining == 0 {
            self.drain_remaining = Some(0);
            return Ok(PluginDrainResult::COMPLETE);
        }
        let frames = (output.len() / self.channels)
            .min(remaining)
            .min(MAX_DRAIN_FRAMES);
        if frames == 0 {
            return Err("limiter drain needs at least one output frame".into());
        }
        // Rate, shape, and capacity are validated before touching caller audio
        // or entering EOS. The existing kernel uses this exact slice in-place.
        let samples = frames * self.channels;
        output[..samples].fill(0.0);
        let mut drain_context = *context;
        drain_context.num_frames = frames;
        self.process_stream(&mut output[..samples], &drain_context)?;
        self.drain_remaining = Some(remaining - frames);
        Ok(PluginDrainResult {
            frames,
            complete: remaining == frames,
        })
    }

    fn tail_length(&self) -> TailLength {
        if self.oversampling != 0 {
            return self
                .oversampled
                .as_ref()
                .map_or(TailLength::Unknown, |path| path.tail());
        }
        if self.initialized {
            TailLength::Finite(self.latency_samples() as u64)
        } else {
            TailLength::Unknown
        }
    }

    fn process_compiled_f32(
        &mut self,
        op: PluginCompiledOp,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Option<Result<usize, String>> {
        if op != PluginCompiledOp::Limiter || self.oversampling != 0 {
            return None;
        }
        let sample_len = context.num_frames.checked_mul(self.channels)?;
        if input.len() < sample_len || output.len() < sample_len {
            return Some(Err(format!(
                "limiter compiled buffer too small: need {sample_len} samples, input={}, output={}",
                input.len(),
                output.len()
            )));
        }
        output[..sample_len].copy_from_slice(&input[..sample_len]);
        Some(self.process_in_place(&mut output[..sample_len], context))
    }

    fn latency_samples(&self) -> usize {
        if self.oversampling != 0 {
            return self.oversampled.as_ref().map_or(0, |path| path.latency());
        }
        self.kernel.lookahead_len
            + if self.isp_mode {
                self.kernel.isp_delay_len
            } else {
                0
            }
    }

    fn compile_metadata(&self) -> PluginCompileMetadata {
        let latency_samples = self.latency_samples();
        PluginCompileMetadata {
            cost_class: self.cost_class(),
            compiled_op: (self.oversampling == 0 && latency_samples == 0)
                .then_some(PluginCompiledOp::Limiter),
            static_gain: None,
            linear: false,
            time_invariant_for_block: false,
            channel_mixing: self.link_amount > 0.0 && self.channels > 1,
            stateful: true,
            latency_samples,
            can_absorb_input_gain: false,
            can_absorb_output_gain: false,
            can_merge_with_eq: false,
            boundary: true,
        }
    }

    fn get_data(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        Some(self.cache.load() as Arc<dyn Any + Send + Sync>)
    }
}
