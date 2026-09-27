//! Analog limiter: mastering limiter core into a shared analog color stage.
//!
//! Signal flow per block: the wrapped [`LimiterPlugin`] runs first on the
//! interleaved host buffer (composition, not a fork — the core crate owns all
//! gain-computer behavior), then [`AnalogColorStage`] applies the selected
//! `math-analog` model in place. Reported latency is the core's lookahead
//! latency; the color stage adds none.

use super::params::{
    AnalogLimiterPluginParams, CORE_KEYS, PARAMS as ALIM, model_id_for_name,
};
use sotf_host::parameters::{Parameter, ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::parametric_plugin::{ParameterSchema, ParameterSet};
use sotf_host::plugin::{
    PluginCompileMetadata, PluginCostClass, PluginInfo, PluginResult, ProcessContext,
};
use sotf_host::simd::enable_ftz_daz;
use sotf_plugin_analog_common::{AnalogColorStage, MODEL_NAMES};
use sotf_plugin_limiter::params::PARAMS as LIM;
use sotf_host::param_specs::find_by_key as lim_pk;
use sotf_plugin_limiter::{LimiterPlugin, LimiterPluginParams};
use std::sync::Arc;
use std::any::Any;

/// Prepared block ceiling for the analog stage; larger host blocks are
/// chunked by the stage itself.
pub const MAX_BLOCK_FRAMES: usize = 8192;

pub struct AnalogLimiterPlugin {
    channels: usize,
    sample_rate: u32,
    initialized: bool,

    core: LimiterPlugin,
    stage: AnalogColorStage,

    // Analog mirror state (the core owns limiter state; see get_parameter).
    model_id: u32,
    drive_db: f32,
    color: f32,
    character: f32,
    trim_db: f32,

    cached_parameters: Vec<Parameter>,
}

fn core_params_from(
    params: &AnalogLimiterPluginParams,
) -> LimiterPluginParams {
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
            core,
            stage: AnalogColorStage::new(channels),
            model_id: 0,
            drive_db: params.analog_drive,
            color: params.analog_color,
            character: params.analog_character,
            trim_db: params.analog_trim,
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

    pub fn from_params(
        channels: usize,
        params: AnalogLimiterPluginParams,
    ) -> PluginResult<Self> {
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
                    .unwrap_or_else(|| {
                        Parameter::new_float(key, key, 0.0, 0.0, 1.0)
                    })
            })
            .collect();
        cached.push(
            Parameter::new_string(
                "analog_model",
                "Analog Model",
                self.model_name().to_string(),
            )
            .with_description("Analog coloration model applied after the limiter core")
            .with_group("Analog"),
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

    fn apply_analog_float(&mut self, key: &str, value: f32) -> PluginResult<()> {
        match key {
            "analog_drive" => {
                self.drive_db = value;
                self.stage.set_drive_db(value)?;
            }
            "analog_color" => {
                self.color = value;
                self.stage.set_color(value)?;
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
                if model_id_for_name(name).is_none() {
                    return Err(format!("Unknown analog model: {name}"));
                }
            }
        }
        for (id, value) in &values {
            let key = id.as_str();
            if key == "analog_model" {
                let name = value.as_string().unwrap_or(MODEL_NAMES[0]);
                if let Some(model_id) = model_id_for_name(name) {
                    self.model_id = model_id;
                    self.stage.set_model_id(model_id)?;
                }
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
        Ok(())
    }

    fn reset(&mut self) {
        self.core.reset();
        self.stage.reset();
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
        Ok(frames)
    }
}
