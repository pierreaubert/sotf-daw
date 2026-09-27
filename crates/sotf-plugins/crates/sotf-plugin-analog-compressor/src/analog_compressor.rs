//! Analog compressor: single-band feed-forward compressor into a shared
//! analog color stage.
//!
//! The dynamics core is native to this crate but built only on host
//! primitives: one linked [`EnvelopeFollower`] on the maximum absolute input
//! across channels (proper stereo-bus behavior with no extra link knob), a
//! soft-knee gain computer, static plus auto makeup, and parallel mix. The
//! [`AnalogColorStage`] then applies the selected `math-analog` model in
//! place. The whole chain is zero-latency.

use super::params::{AnalogCompressorPluginParams, PARAMS as ACOMP, model_id_for_name};
use sotf_host::EnvelopeFollower;
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

/// Floor for envelope-to-dB conversion (-140 dB).
const ENV_EPS: f32 = 1e-7;

/// Auto-makeup follower timing: follow rising GR fast, release slowly.
const AUTO_MAKEUP_ATTACK_MS: f32 = 10.0;
const AUTO_MAKEUP_RELEASE_MS: f32 = 500.0;

/// Auto-makeup ceiling in dB.
const AUTO_MAKEUP_MAX_DB: f32 = 24.0;

/// Soft-knee gain reduction in dB for a detector level in dB.
///
/// Standard quadratic knee: hard knee when `knee_db` is 0, ratio 1:1 when
/// `ratio` is 1.
fn gain_reduction_db(level_db: f32, threshold_db: f32, ratio: f32, knee_db: f32) -> f32 {
    let slope = 1.0 - 1.0 / ratio.max(1.0);
    if knee_db <= 0.0 {
        return ((level_db - threshold_db).max(0.0)) * slope;
    }
    let lower = threshold_db - knee_db * 0.5;
    let upper = threshold_db + knee_db * 0.5;
    if level_db <= lower {
        0.0
    } else if level_db >= upper {
        (level_db - threshold_db) * slope
    } else {
        let over = level_db - lower;
        slope * over * over / (2.0 * knee_db)
    }
}

pub struct AnalogCompressorPlugin {
    channels: usize,
    sample_rate: u32,
    initialized: bool,

    threshold_db: f32,
    ratio: f32,
    attack_ms: f32,
    release_ms: f32,
    knee_db: f32,
    makeup_db: f32,
    mix: f32,
    auto_makeup: bool,

    model_id: u32,
    drive_db: f32,
    color: f32,
    character: f32,
    trim_db: f32,

    detector: EnvelopeFollower,
    makeup_follower: EnvelopeFollower,
    stage: AnalogColorStage,

    cached_parameters: Vec<Parameter>,
}

impl AnalogCompressorPlugin {
    pub fn new(channels: usize) -> Self {
        let channels = channels.max(1);
        let params = AnalogCompressorPluginParams::default();
        let mut plugin = Self {
            channels,
            sample_rate: 0,
            initialized: false,
            threshold_db: params.threshold,
            ratio: params.ratio,
            attack_ms: params.attack,
            release_ms: params.release,
            knee_db: params.knee,
            makeup_db: params.makeup,
            mix: params.mix,
            auto_makeup: params.auto_makeup,
            model_id: 0,
            drive_db: params.analog_drive,
            color: params.analog_color,
            character: params.analog_character,
            trim_db: params.analog_trim,
            detector: EnvelopeFollower::new(params.attack, params.release, 48_000),
            makeup_follower: EnvelopeFollower::new(
                AUTO_MAKEUP_ATTACK_MS,
                AUTO_MAKEUP_RELEASE_MS,
                48_000,
            ),
            stage: AnalogColorStage::new(channels),
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
        params: AnalogCompressorPluginParams,
    ) -> PluginResult<Self> {
        Self::from_params(channels, params)
    }

    pub fn from_params(
        channels: usize,
        params: AnalogCompressorPluginParams,
    ) -> PluginResult<Self> {
        if channels == 0 {
            return Err("Analog compressor requires at least one channel".to_string());
        }
        if model_id_for_name(&params.analog_model).is_none() {
            return Err(format!("Unknown analog model: {}", params.analog_model));
        }
        // Route everything through apply_values so construction and host
        // automation share one validation path.
        let mut plugin = Self::new(channels);
        let mut values = ParameterSet::new();
        for (key, value) in [
            ("threshold", ParameterValue::Float(params.threshold)),
            ("ratio", ParameterValue::Float(params.ratio)),
            ("attack", ParameterValue::Float(params.attack)),
            ("release", ParameterValue::Float(params.release)),
            ("knee", ParameterValue::Float(params.knee)),
            ("makeup", ParameterValue::Float(params.makeup)),
            ("mix", ParameterValue::Float(params.mix)),
        ] {
            values.insert(ParameterId::from(key), value);
        }
        values.insert(
            ParameterId::from("auto_makeup"),
            ParameterValue::Bool(params.auto_makeup),
        );
        values.insert(
            ParameterId::from("analog_model"),
            ParameterValue::String(params.analog_model.clone()),
        );
        for (key, value) in [
            ("analog_drive", params.analog_drive),
            ("analog_color", params.analog_color),
            ("analog_character", params.analog_character),
            ("analog_trim", params.analog_trim),
        ] {
            values.insert(ParameterId::from(key), ParameterValue::Float(value));
        }
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
        let f = |key: &str, label: &str, value: f32, importance: ParameterImportance| {
            Parameter::new_float(
                key,
                label,
                value,
                pk(ACOMP, key).min_f64() as f32,
                pk(ACOMP, key).max_f64() as f32,
            )
            .with_description(pk(ACOMP, key).doc)
            .with_group(pk(ACOMP, key).group)
            .with_importance(importance)
        };
        use ParameterImportance::{Critical, Useful};
        self.cached_parameters = vec![
            f("threshold", "Threshold", self.threshold_db, Critical),
            f("ratio", "Ratio", self.ratio, Critical),
            f("attack", "Attack", self.attack_ms, Useful),
            f("release", "Release", self.release_ms, Useful),
            f("knee", "Knee", self.knee_db, Useful),
            f("makeup", "Makeup", self.makeup_db, Useful),
            f("mix", "Mix", self.mix, Useful),
            Parameter::new_bool("auto_makeup", "Auto Makeup", self.auto_makeup)
                .with_description("Add smoothed measured gain reduction back as makeup")
                .with_group("Dynamics")
                .with_importance(Useful),
            Parameter::new_string(
                "analog_model",
                "Analog Model",
                self.model_name().to_string(),
            )
            .with_description("Analog coloration model applied after the compressor core")
            .with_group("Analog")
            .with_importance(Critical),
            f("analog_drive", "Analog Drive", self.drive_db, Useful),
            f("analog_color", "Analog Color", self.color, Critical),
            f("analog_character", "Analog Character", self.character, Useful),
            f("analog_trim", "Analog Trim", self.trim_db, Useful),
        ];
    }

    fn retime_detector(&mut self) {
        self.detector.set_times(
            self.attack_ms,
            self.release_ms,
            self.sample_rate_for_build(),
        );
    }

    fn sample_rate_for_build(&self) -> u32 {
        if self.sample_rate == 0 {
            48_000
        } else {
            self.sample_rate
        }
    }
}

impl ParametricInPlacePlugin for AnalogCompressorPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Analog Compressor", "0.5.0", "SotF")
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

    fn current_values(&self) -> ParameterSet {
        let mut values = ParameterSet::new();
        for (key, value) in [
            ("threshold", self.threshold_db),
            ("ratio", self.ratio),
            ("attack", self.attack_ms),
            ("release", self.release_ms),
            ("knee", self.knee_db),
            ("makeup", self.makeup_db),
            ("mix", self.mix),
            ("analog_drive", self.drive_db),
            ("analog_color", self.color),
            ("analog_character", self.character),
            ("analog_trim", self.trim_db),
        ] {
            values.insert(ParameterId::from(key), ParameterValue::Float(value));
        }
        values.insert(
            ParameterId::from("auto_makeup"),
            ParameterValue::Bool(self.auto_makeup),
        );
        values.insert(
            ParameterId::from("analog_model"),
            ParameterValue::String(self.model_name().to_string()),
        );
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
            if id.as_str() == "auto_makeup" && value.as_bool().is_none() {
                return Err("auto_makeup must be a bool".to_string());
            }
        }
        let mut timing_dirty = false;
        for (id, value) in &values {
            let key = id.as_str();
            match key {
                "threshold" => self.threshold_db = value.as_float().unwrap_or(self.threshold_db),
                "ratio" => self.ratio = value.as_float().unwrap_or(self.ratio),
                "attack" => {
                    self.attack_ms = value.as_float().unwrap_or(self.attack_ms);
                    timing_dirty = true;
                }
                "release" => {
                    self.release_ms = value.as_float().unwrap_or(self.release_ms);
                    timing_dirty = true;
                }
                "knee" => self.knee_db = value.as_float().unwrap_or(self.knee_db),
                "makeup" => self.makeup_db = value.as_float().unwrap_or(self.makeup_db),
                "mix" => self.mix = value.as_float().unwrap_or(self.mix),
                "auto_makeup" => self.auto_makeup = value.as_bool().unwrap_or(self.auto_makeup),
                "analog_model" => {
                    let name = value.as_string().unwrap_or(MODEL_NAMES[0]);
                    if let Some(model_id) = model_id_for_name(name) {
                        self.model_id = model_id;
                        self.stage.set_model_id(model_id)?;
                    }
                }
                "analog_drive" => {
                    self.drive_db = value.as_float().unwrap_or(self.drive_db);
                    self.stage.set_drive_db(self.drive_db)?;
                }
                "analog_color" => {
                    self.color = value.as_float().unwrap_or(self.color);
                    self.stage.set_color(self.color)?;
                }
                "analog_character" => {
                    self.character = value.as_float().unwrap_or(self.character);
                    self.stage.set_character(self.character)?;
                }
                "analog_trim" => {
                    self.trim_db = value.as_float().unwrap_or(self.trim_db);
                    self.stage.set_output_trim_db(self.trim_db)?;
                }
                _ => return Err(format!("Unknown parameter: {id}")),
            }
        }
        if timing_dirty {
            self.retime_detector();
        }
        self.rebuild_cached_parameters();
        Ok(())
    }

    fn initialize(&mut self, sample_rate: u32) -> PluginResult<()> {
        if sample_rate == 0 {
            return Err("Analog compressor sample rate must be greater than zero".to_string());
        }
        self.sample_rate = sample_rate;
        self.retime_detector();
        self.makeup_follower = EnvelopeFollower::new(
            AUTO_MAKEUP_ATTACK_MS,
            AUTO_MAKEUP_RELEASE_MS,
            sample_rate,
        );
        self.stage
            .prepare(sample_rate, MAX_BLOCK_FRAMES)
            .map_err(|e| format!("Analog compressor stage prepare failed: {e}"))?;
        self.push_analog_state()?;
        self.initialized = true;
        Ok(())
    }

    fn reset(&mut self) {
        self.detector.reset();
        self.makeup_follower.reset();
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
            return Err("Analog compressor must be initialized before processing".to_string());
        }
        if context.sample_rate != self.sample_rate {
            return Err(format!(
                "Analog compressor context rate {} Hz differs from initialized rate {} Hz",
                context.sample_rate, self.sample_rate
            ));
        }
        let total = frames
            .checked_mul(channels)
            .ok_or_else(|| "Analog compressor frame/sample count overflow".to_string())?;
        if buffer.len() < total {
            return Err(format!(
                "Analog compressor buffer too short ({} < {})",
                buffer.len(),
                total
            ));
        }
        let threshold = self.threshold_db;
        let ratio = self.ratio;
        let knee = self.knee_db;
        let makeup = self.makeup_db;
        let mix = self.mix;
        let auto = self.auto_makeup;
        // Compressor core first, then the analog color stage, both in place.
        for frame in 0..frames {
            // Linked detection: hottest channel drives one shared envelope.
            let mut peak = 0.0f32;
            for ch in 0..channels {
                peak = peak.max(buffer[frame * channels + ch].abs());
            }
            let env = self.detector.process(peak);
            let level_db = 20.0 * (env.max(ENV_EPS)).log10();
            let gr_db = gain_reduction_db(level_db, threshold, ratio, knee);
            let auto_db = if auto {
                self.makeup_follower.process(gr_db).min(AUTO_MAKEUP_MAX_DB)
            } else {
                0.0
            };
            let gain = 10.0f32.powf((makeup + auto_db - gr_db) / 20.0);
            for ch in 0..channels {
                let idx = frame * channels + ch;
                let dry = buffer[idx];
                buffer[idx] = dry + mix * (dry * gain - dry);
            }
        }
        self.stage
            .process_interleaved(&mut buffer[..total], frames)
            .map_err(|e| format!("Analog compressor color stage failed: {e}"))?;
        Ok(frames)
    }
}
