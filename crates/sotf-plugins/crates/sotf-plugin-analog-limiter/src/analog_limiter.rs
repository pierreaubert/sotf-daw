//! Analog limiter: mastering limiter core into a shared analog color stage.
//!
//! Signal flow per block: the wrapped [`LimiterPlugin`] runs first on the
//! interleaved host buffer (composition, not a fork — the core crate owns all
//! gain-computer behavior), then [`AnalogColorStage`] applies the selected
//! `math-analog` model in place, and finally a zero-latency safety clamp
//! enforces the threshold ceiling on the emitted samples. Reported latency is
//! the core's lookahead latency; the color stage and the final clamp add none.
//!
//! Output ceiling contract: `threshold` is the final emitted sample-peak
//! ceiling when `mix` is fully wet (1.0), enforced after color and trim. A dry
//! blend (`mix` below 1.0) can exceed it, exactly like the clean core. The
//! `true_peak` toggle enables rate-appropriate inter-sample peak detection in
//! the core detector; it is detection only and carries no strict output
//! true-peak guarantee, before or after color.

use super::params::{AnalogLimiterPluginParams, CORE_KEYS, PARAMS as ALIM, model_id_for_name};
use sotf_host::param_specs::{UpdateMode, find_by_key as lim_pk};
use sotf_host::parameters::{Parameter, ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::parametric_plugin::{ParameterSchema, ParameterSet};
use sotf_host::plugin::{
    PluginCompileMetadata, PluginCostClass, PluginDrainResult, PluginInfo, PluginResult,
    ProcessContext, TailLength,
};
use sotf_host::simd::enable_ftz_daz;
use sotf_plugin_analog_common::{AnalogColorStage, MODEL_NAMES};
use sotf_plugin_limiter::params::PARAMS as LIM;
use sotf_plugin_limiter::{LimiterPlugin, LimiterPluginParams};
use std::any::Any;
use std::sync::Arc;

/// Prepared block ceiling for the analog stage; larger host blocks are
/// chunked by the stage itself.
pub const MAX_BLOCK_FRAMES: usize = 8192;

/// Convert a threshold in dB to the linear final-output ceiling.
///
/// Uses the exact standard conversion so the emitted ceiling never stacks an
/// approximation error on top of the core value.
fn ceiling_linear(threshold_db: f32) -> f32 {
    // Standard amplitude-decibel divisor: 20 dB per decade of amplitude.
    10f32.powf(threshold_db / 20.0)
}

/// Clamp every sample to `±ceiling`, leaving in-range samples untouched.
///
/// Samples already within the ceiling are never written, so the guard is a
/// bit-exact no-op below the ceiling. Non-finite samples compare false and
/// pass through unchanged, matching the core wet-path clamp.
fn clamp_to_ceiling(samples: &mut [f32], ceiling: f32) {
    if !ceiling.is_finite() {
        return;
    }
    let floor = -ceiling;
    for sample in samples.iter_mut() {
        if *sample > ceiling {
            *sample = ceiling;
        } else if *sample < floor {
            *sample = floor;
        }
    }
}

pub struct AnalogLimiterPlugin {
    channels: usize,
    sample_rate: u32,
    initialized: bool,
    has_input: bool,
    drained: bool,
    /// Proves the model's current and future amount are exactly zero. A target
    /// of zero alone is insufficient during a fade from a nonzero amount.
    zero_color_epoch: bool,

    core: LimiterPlugin,
    stage: AnalogColorStage,

    // Analog mirror state (the core owns limiter state; see get_parameter).
    model_id: u32,
    drive_db: f32,
    color: f32,
    character: f32,
    trim_db: f32,
    // Mirrored core targets driving the final safety clamp. Written only after
    // the core accepts the same validated value, so the mirrors never drift.
    threshold_db: f32,
    mix: f32,

    cached_parameters: Vec<Parameter>,
}

fn core_params_from(params: &AnalogLimiterPluginParams) -> LimiterPluginParams {
    LimiterPluginParams {
        threshold_db: params.threshold,
        release_ms: params.release,
        lookahead_ms: params.lookahead,
        soft: params.soft,
        true_peak: params.true_peak,
        mix: params.mix,
        // Advanced core controls stay at core defaults; they are not exposed.
        isp_mode: false,
        dual_release: false,
        feed_forward: false,
        link_amount: lim_pk(LIM, "link_amount").default_f64() as f32,
        oversampling: 0,
    }
}

impl AnalogLimiterPlugin {
    pub fn new(channels: usize) -> Self {
        let channels = channels.max(1);
        let params = AnalogLimiterPluginParams::default();
        let core = LimiterPlugin::from_params(channels, core_params_from(&params));
        let mut plugin = Self {
            channels,
            sample_rate: 0,
            initialized: false,
            has_input: false,
            drained: false,
            zero_color_epoch: false,
            core,
            stage: AnalogColorStage::new(channels),
            model_id: 0,
            drive_db: params.analog_drive,
            color: params.analog_color,
            character: params.analog_character,
            trim_db: params.analog_trim,
            threshold_db: params.threshold,
            mix: params.mix,
            cached_parameters: Vec::new(),
        };
        plugin
            .push_analog_state()
            .expect("default analog state is valid");
        plugin.rebuild_cached_parameters();
        plugin
    }

    /// Compatibility alias for callers already using the explicit fallible name.
    pub fn try_from_params(
        channels: usize,
        params: AnalogLimiterPluginParams,
    ) -> PluginResult<Self> {
        Self::from_params(channels, params)
    }

    pub fn from_params(channels: usize, params: AnalogLimiterPluginParams) -> PluginResult<Self> {
        if channels == 0 {
            return Err("Analog limiter requires at least one channel".to_string());
        }
        if model_id_for_name(&params.analog_model).is_none() {
            return Err(format!("Unknown analog model: {}", params.analog_model));
        }
        // Route everything through apply_values so construction and host
        // automation share one validation path.
        let mut plugin = Self::new(channels);
        let mut values = ParameterSet::new();
        values.insert(
            ParameterId::from("threshold"),
            ParameterValue::Float(params.threshold),
        );
        values.insert(
            ParameterId::from("release"),
            ParameterValue::Float(params.release),
        );
        values.insert(
            ParameterId::from("lookahead"),
            ParameterValue::Float(params.lookahead),
        );
        values.insert(ParameterId::from("soft"), ParameterValue::Bool(params.soft));
        values.insert(
            ParameterId::from("true_peak"),
            ParameterValue::Bool(params.true_peak),
        );
        values.insert(ParameterId::from("mix"), ParameterValue::Float(params.mix));
        values.insert(
            ParameterId::from("analog_model"),
            ParameterValue::String(params.analog_model.clone()),
        );
        values.insert(
            ParameterId::from("analog_drive"),
            ParameterValue::Float(params.analog_drive),
        );
        values.insert(
            ParameterId::from("analog_color"),
            ParameterValue::Float(params.analog_color),
        );
        values.insert(
            ParameterId::from("analog_character"),
            ParameterValue::Float(params.analog_character),
        );
        values.insert(
            ParameterId::from("analog_trim"),
            ParameterValue::Float(params.analog_trim),
        );
        plugin.apply_values(values)?;
        Ok(plugin)
    }

    fn push_analog_state(&mut self) -> PluginResult<()> {
        self.stage.set_model_id(self.model_id)?;
        self.stage.set_drive_db(self.drive_db)?;
        self.stage.set_color(self.color)?;
        self.stage.set_character(self.character)?;
        self.stage.set_output_trim_db(self.trim_db)?;
        Ok(())
    }

    fn model_name(&self) -> &'static str {
        MODEL_NAMES
            .get(self.model_id as usize)
            .copied()
            .unwrap_or(MODEL_NAMES[0])
    }

    fn rebuild_cached_parameters(&mut self) {
        let core_params = self.core.parameters();
        let mut cached: Vec<Parameter> = CORE_KEYS
            .iter()
            .map(|key| {
                core_params
                    .iter()
                    .find(|parameter| parameter.id.as_str() == *key)
                    .cloned()
                    .unwrap_or_else(|| Parameter::new_float(key, key, 0.0, 0.0, 1.0))
            })
            .collect();
        cached.push(
            Parameter::new_string(
                "analog_model",
                "Analog Model",
                self.model_name().to_string(),
            )
            .with_description("Analog coloration model applied after the limiter core")
            .with_group("Analog")
            .with_update_mode(UpdateMode::Structural),
        );
        for (key, label, value) in [
            ("analog_drive", "Analog Drive", self.drive_db),
            ("analog_color", "Analog Color", self.color),
            ("analog_character", "Analog Character", self.character),
            ("analog_trim", "Analog Trim", self.trim_db),
        ] {
            cached.push(
                Parameter::new_float(
                    key,
                    label,
                    value,
                    sotf_host::param_specs::find_by_key(ALIM, key).min_f64() as f32,
                    sotf_host::param_specs::find_by_key(ALIM, key).max_f64() as f32,
                )
                .with_description(sotf_host::param_specs::find_by_key(ALIM, key).doc)
                .with_group("Analog"),
            );
        }
        self.cached_parameters = cached;
    }

    /// Enforce the threshold ceiling on final samples when fully wet.
    ///
    /// The clean core bounds only fully wet output ("dry mix can exceed it");
    /// this guard preserves that contract after color and trim. It adds no
    /// latency, keeps no state, and allocates nothing.
    fn apply_final_ceiling(&self, samples: &mut [f32]) {
        // `mix` is validated to 0..=1, so `>= 1.0` means fully wet without a
        // float-equality comparison.
        if self.mix >= 1.0 {
            clamp_to_ceiling(samples, ceiling_linear(self.threshold_db));
        }
    }

    fn apply_analog_float(&mut self, key: &str, value: f32) -> PluginResult<()> {
        match key {
            "analog_drive" => {
                self.drive_db = value;
                self.stage.set_drive_db(value)?;
            }
            "analog_color" => {
                self.stage.set_color(value)?;
                self.color = value;
                if value != 0.0 {
                    self.zero_color_epoch = false;
                }
            }
            "analog_character" => {
                self.character = value;
                self.stage.set_character(value)?;
            }
            "analog_trim" => {
                self.trim_db = value;
                self.stage.set_output_trim_db(value)?;
            }
            _ => return Err(format!("Unknown analog parameter: {key}")),
        }
        Ok(())
    }
}

impl ParametricInPlacePlugin for AnalogLimiterPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Analog Limiter", "0.5.0", "SotF")
    }

    fn cost_class(&self) -> PluginCostClass {
        PluginCostClass::Dynamics
    }

    fn compile_metadata(&self) -> PluginCompileMetadata {
        PluginCompileMetadata::nonlinear(
            PluginCostClass::Dynamics,
            None,
            self.latency_samples(),
            false,
        )
    }

    fn channels(&self) -> usize {
        self.channels
    }

    fn latency_samples(&self) -> usize {
        // The color stage adds no latency; report the core lookahead latency.
        self.core.latency_samples()
    }

    fn get_data(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        self.core.get_data()
    }

    fn parameter_schema(&self) -> ParameterSchema {
        self.cached_parameters.clone()
    }

    fn parametric_get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        match id.as_str() {
            "analog_drive" => Some(ParameterValue::Float(self.drive_db)),
            "analog_color" => Some(ParameterValue::Float(self.color)),
            "analog_character" => Some(ParameterValue::Float(self.character)),
            "analog_trim" => Some(ParameterValue::Float(self.trim_db)),
            // String values retain the owned control-query contract.
            "analog_model" => Some(ParameterValue::String(self.model_name().to_string())),
            key if CORE_KEYS.contains(&key) => self.core.get_parameter(id),
            _ => None,
        }
    }

    fn current_values(&self) -> ParameterSet {
        let mut values = ParameterSet::new();
        for key in CORE_KEYS {
            let id = ParameterId::from(*key);
            if let Some(value) = self.core.get_parameter(&id) {
                values.insert(id, value);
            }
        }
        values.insert(
            ParameterId::from("analog_model"),
            ParameterValue::String(self.model_name().to_string()),
        );
        for (key, value) in [
            ("analog_drive", self.drive_db),
            ("analog_color", self.color),
            ("analog_character", self.character),
            ("analog_trim", self.trim_db),
        ] {
            values.insert(ParameterId::from(key), ParameterValue::Float(value));
        }
        values
    }

    fn apply_values(&mut self, values: ParameterSet) -> PluginResult<()> {
        if self.drained {
            return if values
                .iter()
                .all(|(id, value)| self.parametric_get_parameter(id).as_ref() == Some(value))
            {
                Ok(())
            } else {
                Err("reset the analog limiter before changing controls after drain".into())
            };
        }
        // Validate everything before mutating any state.
        for (id, value) in &values {
            let Some(parameter) = self
                .cached_parameters
                .iter()
                .find(|parameter| &parameter.id == id)
            else {
                return Err(format!("Unknown parameter: {id}"));
            };
            parameter
                .validate(value)
                .map_err(|error| format!("{id}: {error}"))?;
            if id.as_str() == "analog_model" {
                let Some(name) = value.as_string() else {
                    return Err("analog_model must be a string".to_string());
                };
                let Some(new_id) = model_id_for_name(name) else {
                    return Err(format!("Unknown analog model: {name}"));
                };
                // Structural: replacement allocates and re-prepares the
                // stage on the control thread, so a live change on an
                // initialized instance is refused; adopt the model at
                // construction or state restore instead. Repeating the
                // committed model stays a no-op success for snapshot
                // resends, mirroring the bridge/FFI no-op shields.
                if self.initialized && new_id != self.model_id {
                    return Err("analog_model change requires reconstruction".to_string());
                }
            }
        }
        // Model replacement precedes target updates, so both the targets and
        // their initial smoothing trajectories are independent of map order.
        if let Some(value) = values.get(&ParameterId::from("analog_model"))
            && let Some(name) = value.as_string()
            && let Some(model_id) = model_id_for_name(name)
        {
            self.stage.set_model_id(model_id)?;
            self.model_id = model_id;
        }
        for (id, value) in &values {
            let key = id.as_str();
            if key == "analog_model" {
                continue;
            }
            if key.starts_with("analog_") {
                let Some(float) = value.as_float() else {
                    return Err(format!("{id}: expected float value"));
                };
                self.apply_analog_float(key, float)?;
                continue;
            }
            // Limiter-core key: the core validates against its own schema,
            // whose ranges match this plugin's carried specs.
            self.core.set_parameter(id.clone(), value.clone())?;
            // Mirror the accepted core targets for the final safety clamp.
            // Validation above guarantees finite in-range floats, which the
            // core stores as given (its mix clamp is a no-op in range).
            if key == "threshold" {
                if let Some(float) = value.as_float() {
                    self.threshold_db = float;
                }
            } else if key == "mix"
                && let Some(float) = value.as_float()
            {
                self.mix = float;
            }
        }
        self.rebuild_cached_parameters();
        Ok(())
    }

    fn initialize(&mut self, sample_rate: u32) -> PluginResult<()> {
        if sample_rate == 0 {
            return Err("Analog limiter sample rate must be greater than zero".to_string());
        }
        self.core.initialize(sample_rate)?;
        self.stage
            .prepare(sample_rate, MAX_BLOCK_FRAMES)
            .map_err(|e| format!("Analog limiter stage prepare failed: {e}"))?;
        self.push_analog_state()?;
        self.sample_rate = sample_rate;
        self.initialized = true;
        self.reset();
        Ok(())
    }

    fn reset(&mut self) {
        self.core.reset();
        self.stage.reset();
        self.has_input = false;
        self.drained = false;
        self.zero_color_epoch = self.color == 0.0;
    }

    fn process_in_place(
        &mut self,
        buffer: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        enable_ftz_daz();
        let frames = context.num_frames;
        if !self.initialized {
            return Err("Analog limiter must be initialized before processing".to_string());
        }
        if frames > 0 && self.drained {
            return Err("reset the analog limiter before processing after drain".into());
        }
        // Limiter core first, then the analog color stage, both in place.
        let done = self.core.process_in_place(buffer, context)?;
        if done != frames {
            return Err(format!(
                "Analog limiter core processed {done} of {frames} frames"
            ));
        }
        let total = frames
            .checked_mul(self.channels)
            .ok_or_else(|| "Analog limiter frame/sample count overflow".to_string())?;
        self.stage
            .process_interleaved(&mut buffer[..total], frames)
            .map_err(|e| format!("Analog limiter color stage failed: {e}"))?;
        // Final safety ceiling after color and trim; transparent below it.
        self.apply_final_ceiling(&mut buffer[..total]);
        self.has_input |= frames > 0;
        Ok(frames)
    }

    fn tail_length(&self) -> TailLength {
        if !self.initialized {
            TailLength::Unknown
        } else if self.zero_color_epoch {
            self.core.tail_length()
        } else {
            TailLength::Infinite
        }
    }

    fn drain_output_frames_max(&self) -> usize {
        if self.initialized && self.zero_color_epoch {
            self.core.drain_output_frames_max()
        } else {
            0
        }
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        if !self.initialized {
            None
        } else if self.zero_color_epoch {
            // Color processing neither buffers frames nor adds drain calls
            // in the existing eligible zero-color epoch.
            self.core.drain_call_bound()
        } else {
            // A work bound for the existing unsupported-path COMPLETE result,
            // not a claim that recursive color has finite audio support.
            Some(std::num::NonZeroU64::MIN)
        }
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<PluginDrainResult> {
        if !self.initialized || context.sample_rate != self.sample_rate {
            return Err("analog limiter drain requires the initialized sample rate".into());
        }
        if !output.len().is_multiple_of(self.channels) {
            return Err("analog limiter drain requires whole output frames".into());
        }
        if !self.zero_color_epoch {
            // Color recurrence needs a separately selected rendering policy.
            // Preserve legacy EOS behavior without claiming finite support.
            return Ok(PluginDrainResult::COMPLETE);
        }
        let result = self.core.drain(output, context)?;
        // The prepared stage accepts this bounded, exact channel shape. Advance
        // it only for returned frames, preserving the ordinary core->color path.
        self.stage
            .process_interleaved(&mut output[..result.frames * self.channels], result.frames)?;
        // Same emitted-output contract as ordinary processing.
        self.apply_final_ceiling(&mut output[..result.frames * self.channels]);
        self.drained |= self.has_input;
        Ok(result)
    }
}
