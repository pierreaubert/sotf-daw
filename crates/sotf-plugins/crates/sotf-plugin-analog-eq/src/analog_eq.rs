//! Analog EQ: fixed 4-band parametric EQ into a shared analog color stage.
//!
//! Signal flow per block: biquad core (low-shelf, 2 peaks, high-shelf) runs
//! first on the interleaved host buffer, then [`AnalogColorStage`] applies the
//! selected `math-analog` model in place. Coefficient updates use
//! [`Biquad::update_params`](math_audio_iir_fir::Biquad::update_params), which
//! recomputes coefficients without resetting delay state or allocating.

use super::params::{AnalogEqPluginParams, PARAMS as AEQ, model_id_for_name};
use math_audio_iir_fir::{Biquad, BiquadFilterType};
use sotf_host::param_specs::find_by_key as pk;
use sotf_host::parameters::{Parameter, ParameterId, ParameterImportance, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::parametric_plugin::{ParameterSchema, ParameterSet};
use sotf_host::plugin::{
    PluginCompileMetadata, PluginCostClass, PluginInfo, PluginResult, ProcessContext,
};
use sotf_host::simd::enable_ftz_daz;
use sotf_plugin_analog_common::{AnalogColorStage, MODEL_NAMES};

/// Prepared block ceiling for the analog stage; larger host blocks are
/// chunked by the stage itself.
pub const MAX_BLOCK_FRAMES: usize = 8192;

/// Fixed shelf Q (shelves expose no Q knob).
const SHELF_Q: f64 = 0.71;

/// Band order: low-shelf, peak 1, peak 2, high-shelf.
const BAND_TYPES: [BiquadFilterType; 4] = [
    BiquadFilterType::Lowshelf,
    BiquadFilterType::Peak,
    BiquadFilterType::Peak,
    BiquadFilterType::Highshelf,
];

pub struct AnalogEqPlugin {
    channels: usize,
    sample_rate: u32,
    initialized: bool,

    // Band state (f64 DSP precision).
    low_freq: f64,
    low_gain: f64,
    mid1_freq: f64,
    mid1_gain: f64,
    mid1_q: f64,
    mid2_freq: f64,
    mid2_gain: f64,
    mid2_q: f64,
    high_freq: f64,
    high_gain: f64,

    // Analog stage state.
    model_id: u32,
    drive_db: f32,
    color: f32,
    character: f32,
    trim_db: f32,

    // Per-channel biquad chains (4 bands each), preallocated at construction.
    filters: Vec<Vec<Biquad<f64>>>,
    stage: AnalogColorStage,

    cached_parameters: Vec<Parameter>,
}

fn band_params(plugin: &AnalogEqPlugin) -> [(f64, f64, f64); 4] {
    [
        (plugin.low_freq, plugin.low_gain, SHELF_Q),
        (plugin.mid1_freq, plugin.mid1_gain, plugin.mid1_q),
        (plugin.mid2_freq, plugin.mid2_gain, plugin.mid2_q),
        (plugin.high_freq, plugin.high_gain, SHELF_Q),
    ]
}

impl AnalogEqPlugin {
    pub fn new(channels: usize) -> Self {
        let channels = channels.max(1);
        let params = AnalogEqPluginParams::default();
        let mut plugin = Self {
            channels,
            sample_rate: 0,
            initialized: false,
            low_freq: params.low_freq,
            low_gain: params.low_gain,
            mid1_freq: params.mid1_freq,
            mid1_gain: params.mid1_gain,
            mid1_q: params.mid1_q,
            mid2_freq: params.mid2_freq,
            mid2_gain: params.mid2_gain,
            mid2_q: params.mid2_q,
            high_freq: params.high_freq,
            high_gain: params.high_gain,
            model_id: 0,
            drive_db: params.analog_drive as f32,
            color: params.analog_color as f32,
            character: params.analog_character as f32,
            trim_db: params.analog_trim as f32,
            filters: Vec::new(),
            stage: AnalogColorStage::new(channels),
            cached_parameters: Vec::new(),
        };
        plugin.rebuild_filters(48_000);
        // Defaults are inside every model range by construction.
        plugin
            .push_analog_state()
            .expect("default analog state is valid");
        plugin.rebuild_cached_parameters();
        plugin
    }

    /// Compatibility alias for callers already using the explicit fallible name.
    pub fn try_from_params(channels: usize, params: AnalogEqPluginParams) -> PluginResult<Self> {
        Self::from_params(channels, params)
    }

    pub fn from_params(channels: usize, params: AnalogEqPluginParams) -> PluginResult<Self> {
        if channels == 0 {
            return Err("Analog EQ requires at least one channel".to_string());
        }
        let model_id = model_id_for_name(&params.analog_model)
            .ok_or_else(|| format!("Unknown analog model: {}", params.analog_model))?;
        let mut plugin = Self::new(channels);
        plugin.low_freq = params.low_freq;
        plugin.low_gain = params.low_gain;
        plugin.mid1_freq = params.mid1_freq;
        plugin.mid1_gain = params.mid1_gain;
        plugin.mid1_q = params.mid1_q;
        plugin.mid2_freq = params.mid2_freq;
        plugin.mid2_gain = params.mid2_gain;
        plugin.mid2_q = params.mid2_q;
        plugin.high_freq = params.high_freq;
        plugin.high_gain = params.high_gain;
        plugin.model_id = model_id;
        plugin.drive_db = params.analog_drive as f32;
        plugin.color = params.analog_color as f32;
        plugin.character = params.analog_character as f32;
        plugin.trim_db = params.analog_trim as f32;
        plugin.validate_bands()?;
        plugin.rebuild_filters(plugin.sample_rate_for_build());
        plugin.push_analog_state()?;
        plugin.rebuild_cached_parameters();
        Ok(plugin)
    }

    fn sample_rate_for_build(&self) -> u32 {
        if self.sample_rate == 0 {
            48_000
        } else {
            self.sample_rate
        }
    }

    fn validate_bands(&self) -> PluginResult<()> {
        for (freq, gain, q) in band_params(self) {
            if !freq.is_finite() || freq <= 0.0 {
                return Err(format!("Invalid band frequency {freq}"));
            }
            if !gain.is_finite() {
                return Err(format!("Invalid band gain {gain}"));
            }
            if !q.is_finite() || q <= 0.0 {
                return Err(format!("Invalid band Q {q}"));
            }
        }
        Ok(())
    }

    /// (Re)build the per-channel filter bank in place. Only called from
    /// construction, `initialize`, and parameter application — never from the
    /// realtime process path with a steady layout.
    fn rebuild_filters(&mut self, sample_rate: u32) {
        let sr = sample_rate.max(1) as f64;
        let bands = band_params(self);
        if self.filters.len() != self.channels {
            self.filters = (0..self.channels)
                .map(|_| {
                    bands
                        .iter()
                        .enumerate()
                        .map(|(i, (freq, gain, q))| {
                            Biquad::new(BAND_TYPES[i], *freq, sr, *q, *gain)
                        })
                        .collect()
                })
                .collect();
        } else {
            for channel in &mut self.filters {
                for (filter, (i, (freq, gain, q))) in
                    channel.iter_mut().zip(bands.iter().enumerate())
                {
                    filter.update_params(BAND_TYPES[i], *freq, sr, *q, *gain);
                }
            }
        }
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
        let f = |key: &str, label: &str, value: f64, importance: ParameterImportance| {
            Parameter::new_float(
                key,
                label,
                value as f32,
                pk(AEQ, key).min_f64() as f32,
                pk(AEQ, key).max_f64() as f32,
            )
            .with_description(pk(AEQ, key).doc)
            .with_group(pk(AEQ, key).group)
            .with_importance(importance)
        };
        use ParameterImportance::{Critical, Useful};
        self.cached_parameters = vec![
            f("low_freq", "Low Freq", self.low_freq, Useful),
            f("low_gain", "Low Gain", self.low_gain, Critical),
            f("mid1_freq", "LowMid Freq", self.mid1_freq, Useful),
            f("mid1_gain", "LowMid Gain", self.mid1_gain, Critical),
            f("mid1_q", "LowMid Q", self.mid1_q, Useful),
            f("mid2_freq", "HighMid Freq", self.mid2_freq, Useful),
            f("mid2_gain", "HighMid Gain", self.mid2_gain, Critical),
            f("mid2_q", "HighMid Q", self.mid2_q, Useful),
            f("high_freq", "High Freq", self.high_freq, Useful),
            f("high_gain", "High Gain", self.high_gain, Critical),
            Parameter::new_string(
                "analog_model",
                "Analog Model",
                self.model_name().to_string(),
            )
            .with_description("Analog coloration model applied after the EQ core")
            .with_group("Analog")
            .with_importance(Critical),
            f("analog_drive", "Analog Drive", self.drive_db as f64, Useful),
            f("analog_color", "Analog Color", self.color as f64, Critical),
            f(
                "analog_character",
                "Analog Character",
                self.character as f64,
                Useful,
            ),
            f("analog_trim", "Analog Trim", self.trim_db as f64, Useful),
        ];
    }

    fn apply_float(&mut self, id: &ParameterId, value: f32) -> PluginResult<()> {
        let key = id.as_str();
        match key {
            "low_freq" => self.low_freq = value as f64,
            "low_gain" => self.low_gain = value as f64,
            "mid1_freq" => self.mid1_freq = value as f64,
            "mid1_gain" => self.mid1_gain = value as f64,
            "mid1_q" => self.mid1_q = value as f64,
            "mid2_freq" => self.mid2_freq = value as f64,
            "mid2_gain" => self.mid2_gain = value as f64,
            "mid2_q" => self.mid2_q = value as f64,
            "high_freq" => self.high_freq = value as f64,
            "high_gain" => self.high_gain = value as f64,
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
            _ => return Err(format!("Unknown parameter: {id}")),
        }
        Ok(())
    }
}

impl ParametricInPlacePlugin for AnalogEqPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Analog EQ", "0.5.0", "SotF")
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

    fn parameter_schema(&self) -> ParameterSchema {
        self.cached_parameters.clone()
    }

    fn parametric_get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        let value = match id.as_str() {
            "low_freq" => self.low_freq as f32,
            "low_gain" => self.low_gain as f32,
            "mid1_freq" => self.mid1_freq as f32,
            "mid1_gain" => self.mid1_gain as f32,
            "mid1_q" => self.mid1_q as f32,
            "mid2_freq" => self.mid2_freq as f32,
            "mid2_gain" => self.mid2_gain as f32,
            "mid2_q" => self.mid2_q as f32,
            "high_freq" => self.high_freq as f32,
            "high_gain" => self.high_gain as f32,
            "analog_drive" => self.drive_db,
            "analog_color" => self.color,
            "analog_character" => self.character,
            "analog_trim" => self.trim_db,
            // String values retain the owned control-query contract.
            "analog_model" => return Some(ParameterValue::String(self.model_name().to_string())),
            _ => return None,
        };
        Some(ParameterValue::Float(value))
    }

    fn current_values(&self) -> ParameterSet {
        let mut values = ParameterSet::new();
        let floats: [(&str, f64); 14] = [
            ("low_freq", self.low_freq),
            ("low_gain", self.low_gain),
            ("mid1_freq", self.mid1_freq),
            ("mid1_gain", self.mid1_gain),
            ("mid1_q", self.mid1_q),
            ("mid2_freq", self.mid2_freq),
            ("mid2_gain", self.mid2_gain),
            ("mid2_q", self.mid2_q),
            ("high_freq", self.high_freq),
            ("high_gain", self.high_gain),
            ("analog_drive", self.drive_db as f64),
            ("analog_color", self.color as f64),
            ("analog_character", self.character as f64),
            ("analog_trim", self.trim_db as f64),
        ];
        for (key, value) in floats {
            values.insert(ParameterId::from(key), ParameterValue::Float(value as f32));
        }
        values.insert(
            ParameterId::from("analog_model"),
            ParameterValue::String(self.model_name().to_string()),
        );
        values
    }

    fn apply_values(&mut self, values: ParameterSet) -> PluginResult<()> {
        // Validate everything before mutating any state so bulk updates are
        // all-or-nothing at the schema level.
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
        // Model replacement precedes target updates, so both the targets and
        // their initial smoothing trajectories are independent of map order.
        if let Some(value) = values.get(&ParameterId::from("analog_model"))
            && let Some(name) = value.as_string()
            && let Some(model_id) = model_id_for_name(name)
        {
            self.stage.set_model_id(model_id)?;
            self.model_id = model_id;
        }
        let mut bands_dirty = false;
        for (id, value) in &values {
            if id.as_str() == "analog_model" {
                continue;
            }
            let Some(float) = value.as_float() else {
                return Err(format!("{id}: expected float value"));
            };
            self.apply_float(id, float)?;
            bands_dirty = bands_dirty
                || !matches!(
                    id.as_str(),
                    "analog_drive" | "analog_color" | "analog_character" | "analog_trim"
                );
        }
        self.validate_bands()?;
        if bands_dirty {
            self.rebuild_filters(self.sample_rate_for_build());
        }
        self.rebuild_cached_parameters();
        Ok(())
    }

    fn initialize(&mut self, sample_rate: u32) -> PluginResult<()> {
        if sample_rate == 0 {
            return Err("Analog EQ sample rate must be greater than zero".to_string());
        }
        self.sample_rate = sample_rate;
        self.rebuild_filters(sample_rate);
        self.stage
            .prepare(sample_rate, MAX_BLOCK_FRAMES)
            .map_err(|e| format!("Analog EQ stage prepare failed: {e}"))?;
        self.push_analog_state()?;
        self.initialized = true;
        Ok(())
    }

    fn reset(&mut self) {
        for channel in &mut self.filters {
            for filter in channel {
                filter.reset();
            }
        }
        self.stage.reset();
    }

    fn process_in_place(
        &mut self,
        buffer: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        enable_ftz_daz();
        let frames = context.num_frames;
        let channels = self.channels;
        if !self.initialized {
            return Err("Analog EQ must be initialized before processing".to_string());
        }
        if context.sample_rate != self.sample_rate {
            return Err(format!(
                "Analog EQ context rate {} Hz differs from initialized rate {} Hz",
                context.sample_rate, self.sample_rate
            ));
        }
        let total = frames
            .checked_mul(channels)
            .ok_or_else(|| "Analog EQ frame/sample count overflow".to_string())?;
        if buffer.len() < total {
            return Err(format!(
                "Analog EQ buffer too short ({} < {})",
                buffer.len(),
                total
            ));
        }
        // Core EQ first, then the analog color stage, both in place.
        for frame in 0..frames {
            for (ch, channel) in self.filters.iter_mut().enumerate() {
                let idx = frame * channels + ch;
                let mut sample = buffer[idx] as f64;
                for filter in channel.iter_mut() {
                    sample = filter.process(sample);
                }
                buffer[idx] = sample as f32;
            }
        }
        self.stage
            .process_interleaved(&mut buffer[..total], frames)
            .map_err(|e| format!("Analog EQ color stage failed: {e}"))?;
        Ok(frames)
    }
}
