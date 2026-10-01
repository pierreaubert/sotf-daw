pub mod params;
pub mod profile;

use crate::params::PARAMS as HP;
use crate::profile::{
    PROFILE_FLOOR_MIN_DB, PROFILE_THRESHOLD_MARGIN_DB, CaptureState, LINK_INDEPENDENT,
    LINK_LINKED, NoiseProfileData, ReductionCurve,
};
use plugins_denoiser::hiss::HissReducer;
use plugins_denoiser::spectral_hiss::{
    SPECTRAL_HISS_FFT_SIZE, SPECTRAL_HISS_NUM_BINS, SpectralHissReducer,
};
const DRAIN_HOP: usize = SPECTRAL_HISS_FFT_SIZE / 4;
use serde::{Deserialize, Serialize};
use sotf_host::param_bridge;
use sotf_host::param_specs::find_by_key as pk;
use sotf_host::parameters::{Parameter, ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::parametric_plugin::{ParameterSchema, ParameterSet};
use sotf_host::plugin::{
    PluginCompileMetadata, PluginCostClass, PluginDrainResult, PluginInfo, PluginResult,
    ProcessContext, TailLength,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HissReducerPluginParams {
    #[serde(default = "d_enabled")]
    pub enabled: bool,
    #[serde(default = "d_threshold_db")]
    pub threshold_db: f32,
    #[serde(default = "d_frequency_hz")]
    pub frequency_hz: f32,
    #[serde(default = "d_strength")]
    pub strength: f32,
    #[serde(default = "d_spectral_mode")]
    pub spectral_mode: bool,
    #[serde(default = "d_use_captured_profile")]
    pub use_captured_profile: bool,
    #[serde(default = "d_curve_low")]
    pub curve_low: f32,
    #[serde(default = "d_curve_mid")]
    pub curve_mid: f32,
    #[serde(default = "d_curve_high")]
    pub curve_high: f32,
    #[serde(default = "d_link_mode")]
    pub link_mode: i32,
    /// Opt-in spectral transient guard. Off by default, so old presets,
    /// profiles, and defaults never enable it; stored in both modes but
    /// applied by the spectral reducer only.
    #[serde(default = "d_transient_guard")]
    pub transient_guard: bool,
    /// Load-only carrier for a persisted noise profile. Construction imports
    /// it into the pre-allocated live store and clears this field; use
    /// [`HissReducerPlugin::persisted_params`] to export state for saving.
    #[serde(default)]
    pub captured_profile: Option<NoiseProfileData>,
}

fn d_enabled() -> bool {
    pk(HP, "enabled").default_bool()
}
fn d_threshold_db() -> f32 {
    pk(HP, "threshold_db").default_f32()
}
fn d_frequency_hz() -> f32 {
    pk(HP, "frequency_hz").default_f32()
}
fn d_strength() -> f32 {
    pk(HP, "strength").default_f32()
}
fn d_spectral_mode() -> bool {
    pk(HP, "spectral_mode").default_bool()
}
fn d_use_captured_profile() -> bool {
    pk(HP, "use_captured_profile").default_bool()
}
fn d_curve_low() -> f32 {
    pk(HP, "curve_low").default_f32()
}
fn d_curve_mid() -> f32 {
    pk(HP, "curve_mid").default_f32()
}
fn d_curve_high() -> f32 {
    pk(HP, "curve_high").default_f32()
}
fn d_link_mode() -> i32 {
    pk(HP, "link_mode").default_i32()
}
fn d_transient_guard() -> bool {
    pk(HP, "transient_guard").default_bool()
}

impl Default for HissReducerPluginParams {
    fn default() -> Self {
        Self {
            enabled: d_enabled(),
            threshold_db: d_threshold_db(),
            frequency_hz: d_frequency_hz(),
            strength: d_strength(),
            spectral_mode: d_spectral_mode(),
            use_captured_profile: d_use_captured_profile(),
            curve_low: d_curve_low(),
            curve_mid: d_curve_mid(),
            curve_high: d_curve_high(),
            link_mode: d_link_mode(),
            transient_guard: d_transient_guard(),
            captured_profile: None,
        }
    }
}

pub struct HissReducerPlugin {
    channels: usize,
    sample_rate: u32,
    initialized: bool,
    params: HissReducerPluginParams,
    reducer: HissReducer,
    spectral_reducer: SpectralHissReducer,
    cached_parameters: Vec<Parameter>,
    has_input: bool,
    source_phase: usize,
    drain_remaining: Option<usize>,
    drain_cache: Vec<f32>,
    drain_frames: usize,
    drain_pos: usize,
    capture: CaptureState,
    profile_floor_db: Vec<f32>,
    has_profile: bool,
    profile_sample_rate: u32,
    profile_cutoff_hz: f32,
    profile_frames: u64,
    // Pre-sized per-bin spectral curve table (SPECTRAL_HISS_NUM_BINS,
    // 1.0 default). Rebuilt by refresh_curve_gains() and pushed to the
    // backend by refresh_backend_params(); sized once at construction
    // so control-rate refreshes never allocate.
    curve_gains: Vec<f32>,
}

impl HissReducerPlugin {
    pub fn new(channels: usize) -> Self {
        Self::try_new(channels).expect("HissReducerPlugin requires at least one channel")
    }

    pub fn from_params(channels: usize, params: HissReducerPluginParams) -> Self {
        Self::try_from_params(channels, params)
            .expect("HissReducerPlugin requires at least one channel")
    }

    pub fn try_new(channels: usize) -> PluginResult<Self> {
        Self::try_from_params(channels, HissReducerPluginParams::default())
    }

    pub fn try_from_params(channels: usize, params: HissReducerPluginParams) -> PluginResult<Self> {
        Self::try_from_params_at_sample_rate(channels, 48_000, params)
    }

    pub fn try_from_params_at_sample_rate(
        channels: usize,
        sample_rate: u32,
        params: HissReducerPluginParams,
    ) -> PluginResult<Self> {
        if channels == 0 {
            return Err("HissReducerPlugin requires at least one channel".to_string());
        }
        let mut params = Self::canonicalize_params(params, sample_rate, channels)?;
        // The params profile is a load-only carrier; move it into the
        // pre-allocated live store so later capture completion and restore
        // calls never allocate.
        let imported_profile = params.captured_profile.take();
        let mut reducer = Self::build_reducer(channels, &params);
        reducer.initialize(sample_rate)?;
        reducer.set_enabled(params.enabled, true);
        let mut spectral_reducer = SpectralHissReducer::new(channels);
        spectral_reducer.set_enabled(params.enabled);
        spectral_reducer.initialize(sample_rate)?;
        spectral_reducer.set_params(params.frequency_hz, params.threshold_db, params.strength);
        let mut plugin = Self {
            channels,
            sample_rate,
            initialized: false,
            reducer,
            spectral_reducer,
            params,
            cached_parameters: Vec::new(),
            has_input: false,
            source_phase: 0,
            drain_remaining: None,
            drain_cache: vec![0.0; DRAIN_HOP * channels],
            drain_frames: 0,
            drain_pos: 0,
            capture: CaptureState::new(channels),
            profile_floor_db: vec![PROFILE_FLOOR_MIN_DB; channels],
            has_profile: false,
            profile_sample_rate: sample_rate,
            profile_cutoff_hz: 4_000.0,
            profile_frames: 0,
            curve_gains: vec![1.0; SPECTRAL_HISS_NUM_BINS],
        };
        if let Some(data) = imported_profile {
            // Canonicalization already validated the shape and the channel
            // agreement; copy defensively into the pre-sized store.
            if data.floor_db_per_channel.len() == channels {
                plugin.profile_floor_db.copy_from_slice(&data.floor_db_per_channel);
                plugin.has_profile = true;
                plugin.profile_sample_rate = data.sample_rate;
                plugin.profile_cutoff_hz = data.measurement_cutoff_hz;
                plugin.profile_frames = data.frames_analyzed;
            }
        }
        plugin.refresh_backend_params();
        plugin.rebuild_cached_parameters();
        Ok(plugin)
    }

    fn maximum_frequency(sample_rate: u32) -> PluginResult<f32> {
        if sample_rate == 0 {
            return Err("sample rate must be nonzero".to_string());
        }
        let minimum = pk(HP, "frequency_hz").min_f64() as f32;
        let maximum = (sample_rate as f32 * 0.45).min(pk(HP, "frequency_hz").max_f64() as f32);
        if maximum < minimum {
            return Err(format!(
                "sample rate {sample_rate} is too low for the {minimum} Hz minimum cutoff"
            ));
        }
        Ok(maximum)
    }

    fn canonicalize_params(
        mut params: HissReducerPluginParams,
        sample_rate: u32,
        channels: usize,
    ) -> PluginResult<HissReducerPluginParams> {
        let defaults = HissReducerPluginParams::default();
        let maximum_frequency = Self::maximum_frequency(sample_rate)?;
        params.threshold_db = if params.threshold_db.is_finite() {
            params.threshold_db.clamp(
                pk(HP, "threshold_db").min_f64() as f32,
                pk(HP, "threshold_db").max_f64() as f32,
            )
        } else {
            defaults.threshold_db
        };
        params.frequency_hz = if params.frequency_hz.is_finite() {
            params
                .frequency_hz
                .clamp(pk(HP, "frequency_hz").min_f64() as f32, maximum_frequency)
        } else {
            defaults.frequency_hz.min(maximum_frequency)
        };
        params.strength = if params.strength.is_finite() {
            params.strength.clamp(
                pk(HP, "strength").min_f64() as f32,
                pk(HP, "strength").max_f64() as f32,
            )
        } else {
            defaults.strength
        };
        params.curve_low = ReductionCurve::canonicalize(params.curve_low);
        params.curve_mid = ReductionCurve::canonicalize(params.curve_mid);
        params.curve_high = ReductionCurve::canonicalize(params.curve_high);
        params.link_mode = params.link_mode.clamp(LINK_INDEPENDENT, LINK_LINKED);
        params.captured_profile = match params.captured_profile {
            Some(data) => {
                // Corrupt blobs fail the whole load transactionally; a
                // well-formed blob for another channel count is simply
                // inapplicable and is dropped. Floors are broadband level
                // references, so any sample rate stays valid.
                data.validate()?;
                if data.channels != channels {
                    None
                } else {
                    Some(data)
                }
            }
            None => None,
        };
        Ok(params)
    }

    fn build_reducer(channels: usize, params: &HissReducerPluginParams) -> HissReducer {
        let mut reducer = HissReducer::new(channels);
        reducer.set_params(params.frequency_hz, params.threshold_db, params.strength);
        reducer
    }

    fn param_value(&self, index: usize) -> Option<f64> {
        match index {
            0 => Some(if self.params.enabled { 1.0 } else { 0.0 }),
            1 => Some(self.params.threshold_db as f64),
            2 => Some(self.params.frequency_hz as f64),
            3 => Some(self.params.strength as f64),
            4 => Some(if self.params.spectral_mode { 1.0 } else { 0.0 }),
            5 => Some(if self.capture.is_active() { 1.0 } else { 0.0 }),
            6 => Some(if self.params.use_captured_profile {
                1.0
            } else {
                0.0
            }),
            7 => Some(0.0),
            8 => Some(self.params.curve_low as f64),
            9 => Some(self.params.curve_mid as f64),
            10 => Some(self.params.curve_high as f64),
            11 => Some(self.params.link_mode as f64),
            12 => Some(if self.params.transient_guard {
                1.0
            } else {
                0.0
            }),
            _ => None,
        }
    }

    fn rebuild_cached_parameters(&mut self) {
        self.cached_parameters = param_bridge::build_parameters(HP, |i| self.param_value(i));
    }

    fn apply_parameter(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()> {
        match id.as_str() {
            "enabled" => {
                self.params.enabled = value
                    .as_bool()
                    .ok_or_else(|| "enabled must be a bool".to_string())?;
                self.reducer.set_enabled(self.params.enabled, false);
                self.spectral_reducer.set_enabled(self.params.enabled);
            }
            "threshold_db" => {
                self.params.threshold_db = value
                    .as_float()
                    .ok_or_else(|| "threshold_db must be a float".to_string())?;
                self.refresh_backend_params();
            }
            "frequency_hz" => {
                self.params.frequency_hz = value
                    .as_float()
                    .ok_or_else(|| "frequency_hz must be a float".to_string())?;
                self.refresh_backend_params();
            }
            "strength" => {
                self.params.strength = value
                    .as_float()
                    .ok_or_else(|| "strength must be a float".to_string())?;
                self.refresh_backend_params();
            }
            "spectral_mode" => {
                self.params.spectral_mode = value
                    .as_bool()
                    .ok_or_else(|| "spectral_mode must be a bool".to_string())?;
            }
            "learn_noise" => {
                let fire = value
                    .as_bool()
                    .ok_or_else(|| "learn_noise must be a bool".to_string())?;
                if fire {
                    // Restarting replaces any partial measurement; the stored
                    // profile is only overwritten when the new capture
                    // completes.
                    self.capture
                        .start(self.sample_rate, self.params.frequency_hz)?;
                } else {
                    self.capture.cancel();
                }
            }
            "use_captured_profile" => {
                self.params.use_captured_profile = value
                    .as_bool()
                    .ok_or_else(|| "use_captured_profile must be a bool".to_string())?;
                self.refresh_backend_params();
            }
            "clear_profile" => {
                let fire = value
                    .as_bool()
                    .ok_or_else(|| "clear_profile must be a bool".to_string())?;
                if fire {
                    self.clear_stored_profile();
                }
            }
            "curve_low" => {
                self.params.curve_low = value
                    .as_float()
                    .ok_or_else(|| "curve_low must be a float".to_string())?;
                self.refresh_backend_params();
            }
            "curve_mid" => {
                self.params.curve_mid = value
                    .as_float()
                    .ok_or_else(|| "curve_mid must be a float".to_string())?;
                self.refresh_backend_params();
            }
            "curve_high" => {
                self.params.curve_high = value
                    .as_float()
                    .ok_or_else(|| "curve_high must be a float".to_string())?;
                self.refresh_backend_params();
            }
            "link_mode" => {
                let mode = value
                    .as_int()
                    .ok_or_else(|| "link_mode must be an int".to_string())?;
                self.params.link_mode = mode.clamp(LINK_INDEPENDENT, LINK_LINKED);
                self.refresh_backend_params();
            }
            "transient_guard" => {
                self.params.transient_guard = value
                    .as_bool()
                    .ok_or_else(|| "transient_guard must be a bool".to_string())?;
                self.refresh_backend_params();
            }
            _ => return Err(format!("Unknown parameter: {id}")),
        }
        Ok(())
    }
    fn clear_drain(&mut self) {
        self.has_input = false;
        self.source_phase = 0;
        self.drain_remaining = None;
        self.drain_cache.fill(0.0);
        self.drain_frames = 0;
        self.drain_pos = 0;
    }

    fn effective_threshold_db(&self) -> f32 {
        if self.params.spectral_mode || !self.params.use_captured_profile || !self.has_profile {
            return self.params.threshold_db;
        }
        let mut floor = PROFILE_FLOOR_MIN_DB;
        for candidate in &self.profile_floor_db {
            floor = floor.max(*candidate);
        }
        self.params
            .threshold_db
            .max(floor + PROFILE_THRESHOLD_MARGIN_DB)
    }

    fn refresh_backend_params(&mut self) {
        let effective = self.effective_threshold_db();
        self.reducer.set_params(
            self.params.frequency_hz,
            effective,
            self.params.strength,
        );
        self.reducer.set_linked(self.params.link_mode == LINK_LINKED);
        self.refresh_curve_gains();
        self.spectral_reducer.set_params(
            self.params.frequency_hz,
            self.params.threshold_db,
            self.params.strength,
        );
        // The pushed buffers are pre-sized and validated at construction,
        // so these setters cannot fail here. Call unconditionally (never
        // put the call itself inside debug_assert! — it would vanish in
        // release); the assertions only document the contract in tests.
        let curve_result = self.spectral_reducer.set_curve_gains(&self.curve_gains);
        debug_assert!(curve_result.is_ok());
        let noise_result = self.spectral_reducer.set_external_noise(
            self.params.use_captured_profile && self.has_profile,
            &self.profile_floor_db,
        );
        debug_assert!(noise_result.is_ok());
        self.spectral_reducer
            .set_linked(self.params.link_mode == LINK_LINKED);
        // The guard is a plain backend flag store: infallible and
        // allocation-free, inert unless set. Pushed unconditionally like
        // the other spectral settings; the time-domain path never
        // consults the spectral reducer, so time-domain audio is provably
        // untouched by this flag. Deliberately not auto-enabled for old
        // profiles: legacy sessions keep legacy sound.
        self.spectral_reducer
            .set_transient_guard(self.params.transient_guard);
    }

    fn clear_stored_profile(&mut self) {
        self.capture.cancel();
        self.has_profile = false;
        self.profile_floor_db.fill(PROFILE_FLOOR_MIN_DB);
        self.profile_frames = 0;
        self.refresh_backend_params();
    }

    fn export_profile(&self) -> Option<NoiseProfileData> {
        if !self.has_profile {
            return None;
        }
        Some(NoiseProfileData {
            format_version: crate::profile::PROFILE_FORMAT_VERSION,
            sample_rate: self.profile_sample_rate,
            channels: self.channels,
            measurement_cutoff_hz: self.profile_cutoff_hz,
            floor_db_per_channel: self.profile_floor_db.clone(),
            frames_analyzed: self.profile_frames,
        })
    }

    /// Returns true while a noise capture is accumulating.
    pub fn is_capturing(&self) -> bool {
        self.capture.is_active()
    }

    /// Returns capture progress in 0.0..=1.0 (0.0 while idle).
    pub fn capture_progress(&self) -> f32 {
        self.capture.progress()
    }

    /// Returns true when a captured profile is stored.
    pub fn has_captured_profile(&self) -> bool {
        self.has_profile
    }

    /// Returns the loudest stored per-channel floor, if any profile exists.
    pub fn overall_profile_floor_db(&self) -> Option<f32> {
        if !self.has_profile {
            return None;
        }
        Some(
            self.profile_floor_db
                .iter()
                .copied()
                .fold(PROFILE_FLOOR_MIN_DB, f32::max),
        )
    }

    /// Returns stored profile metadata as (sample rate, cutoff, frames).
    pub fn profile_metadata(&self) -> Option<(u32, f32, u64)> {
        if !self.has_profile {
            return None;
        }
        Some((
            self.profile_sample_rate,
            self.profile_cutoff_hz,
            self.profile_frames,
        ))
    }

    /// Returns the current per-frequency reduction curve.
    pub fn reduction_curve(&self) -> ReductionCurve {
        ReductionCurve {
            low: self.params.curve_low,
            mid: self.params.curve_mid,
            high: self.params.curve_high,
        }
    }

    /// Rebuilds the pre-sized per-bin curve table from the curve params.
    ///
    /// Allocation-free: fills owned storage. Canonicalizes each value so
    /// the backend length/range validation cannot fail. Runs on
    /// control-rate refreshes only (construction, `initialize`, named
    /// and batch parameter updates, capture completion, profile
    /// restore/clear), never on the sample path.
    fn refresh_curve_gains(&mut self) {
        let curve = self.reduction_curve();
        let rate = self.sample_rate.max(1) as f32;
        for (bin, gain) in self.curve_gains.iter_mut().enumerate() {
            let freq_hz = bin as f32 * rate / SPECTRAL_HISS_FFT_SIZE as f32;
            *gain = ReductionCurve::canonicalize(curve.gain_at(freq_hz));
        }
    }

    /// Returns the live per-bin spectral curve table.
    ///
    /// Entry `bin` holds the canonicalized reduction scale at
    /// `bin * sample_rate / 1024` Hz. The table always has
    /// `SPECTRAL_HISS_NUM_BINS` entries and is rebuilt from the curve
    /// params (all 1.0 by default) whenever settings refresh.
    pub fn curve_gains(&self) -> &[f32] {
        &self.curve_gains
    }

    /// Returns the current link mode (`LINK_INDEPENDENT`/`LINK_LINKED`).
    pub fn link_mode(&self) -> i32 {
        self.params.link_mode
    }

    /// Returns true when the spectral transient guard is enabled.
    pub fn transient_guard(&self) -> bool {
        self.params.transient_guard
    }

    /// Exports the full saveable state, including the stored profile.
    ///
    /// This allocates and is intended for preset/save threads, never the
    /// audio path.
    pub fn persisted_params(&self) -> HissReducerPluginParams {
        let mut params = self.params.clone();
        params.captured_profile = self.export_profile();
        params
    }

    /// Restores a noise profile after validating it transactionally.
    ///
    /// The previous profile is kept when validation fails. Unlike preset
    /// loading (which drops channel-mismatched blobs), this explicit call
    /// rejects them as an error. Borrows the data and never allocates.
    pub fn restore_profile(&mut self, data: &NoiseProfileData) -> PluginResult<()> {
        data.validate()?;
        if data.channels != self.channels {
            return Err(format!(
                "noise profile has {} channels, plugin has {}",
                data.channels, self.channels
            ));
        }
        self.profile_floor_db
            .copy_from_slice(&data.floor_db_per_channel);
        self.has_profile = true;
        self.profile_sample_rate = data.sample_rate;
        self.profile_cutoff_hz = data.measurement_cutoff_hz;
        self.profile_frames = data.frames_analyzed;
        self.refresh_backend_params();
        Ok(())
    }

    /// Discards the stored profile and any partial capture. Never allocates.
    pub fn clear_captured_profile(&mut self) {
        self.clear_stored_profile();
    }
}

impl ParametricInPlacePlugin for HissReducerPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Hiss Reducer", env!("CARGO_PKG_VERSION"), "SotF")
            .with_description("Persistent low-level high-frequency reducer")
    }

    fn cost_class(&self) -> PluginCostClass {
        if self.params.spectral_mode {
            PluginCostClass::Fft
        } else {
            PluginCostClass::Iir
        }
    }

    fn compile_metadata(&self) -> PluginCompileMetadata {
        PluginCompileMetadata::nonlinear(self.cost_class(), None, self.latency_samples(), false)
    }

    fn channels(&self) -> usize {
        self.channels
    }

    fn parameter_schema(&self) -> ParameterSchema {
        self.cached_parameters.clone()
    }

    fn parametric_get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        match id.as_str() {
            "enabled" => Some(ParameterValue::Bool(self.params.enabled)),
            "threshold_db" => Some(ParameterValue::Float(self.params.threshold_db)),
            "frequency_hz" => Some(ParameterValue::Float(self.params.frequency_hz)),
            "strength" => Some(ParameterValue::Float(self.params.strength)),
            "spectral_mode" => Some(ParameterValue::Bool(self.params.spectral_mode)),
            "learn_noise" => Some(ParameterValue::Bool(self.capture.is_active())),
            "use_captured_profile" => {
                Some(ParameterValue::Bool(self.params.use_captured_profile))
            }
            "clear_profile" => Some(ParameterValue::Bool(false)),
            "curve_low" => Some(ParameterValue::Float(self.params.curve_low)),
            "curve_mid" => Some(ParameterValue::Float(self.params.curve_mid)),
            "curve_high" => Some(ParameterValue::Float(self.params.curve_high)),
            "link_mode" => Some(ParameterValue::Int(self.params.link_mode)),
            "transient_guard" => Some(ParameterValue::Bool(self.params.transient_guard)),
            _ => None,
        }
    }

    fn current_values(&self) -> ParameterSet {
        let mut values = ParameterSet::new();
        values.insert(
            ParameterId::from("enabled"),
            ParameterValue::Bool(self.params.enabled),
        );
        values.insert(
            ParameterId::from("threshold_db"),
            ParameterValue::Float(self.params.threshold_db),
        );
        values.insert(
            ParameterId::from("frequency_hz"),
            ParameterValue::Float(self.params.frequency_hz),
        );
        values.insert(
            ParameterId::from("strength"),
            ParameterValue::Float(self.params.strength),
        );
        values.insert(
            ParameterId::from("spectral_mode"),
            ParameterValue::Bool(self.params.spectral_mode),
        );
        values.insert(
            ParameterId::from("learn_noise"),
            ParameterValue::Bool(self.capture.is_active()),
        );
        values.insert(
            ParameterId::from("use_captured_profile"),
            ParameterValue::Bool(self.params.use_captured_profile),
        );
        values.insert(
            ParameterId::from("clear_profile"),
            ParameterValue::Bool(false),
        );
        values.insert(
            ParameterId::from("curve_low"),
            ParameterValue::Float(self.params.curve_low),
        );
        values.insert(
            ParameterId::from("curve_mid"),
            ParameterValue::Float(self.params.curve_mid),
        );
        values.insert(
            ParameterId::from("curve_high"),
            ParameterValue::Float(self.params.curve_high),
        );
        values.insert(
            ParameterId::from("link_mode"),
            ParameterValue::Int(self.params.link_mode),
        );
        values.insert(
            ParameterId::from("transient_guard"),
            ParameterValue::Bool(self.params.transient_guard),
        );
        values
    }

    fn apply_values(&mut self, values: ParameterSet) -> PluginResult<()> {
        if self.drain_remaining.is_some() {
            return Err("Reset spectral hiss before changing parameters after drain starts".into());
        }
        let mut next = self.params.clone();
        for (id, value) in &values {
            self.parametric_validate_parameter(id, value)?;
            param_bridge::set_parameter(HP, id, value, |i, v| match i {
                0 => next.enabled = v > 0.5,
                1 => next.threshold_db = v as f32,
                2 => next.frequency_hz = v as f32,
                3 => next.strength = v as f32,
                4 => next.spectral_mode = v > 0.5,
                // Triggers never fire from batch updates; use the named
                // setter to start/cancel capture or discard a profile.
                5 => {}
                6 => next.use_captured_profile = v > 0.5,
                7 => {}
                8 => next.curve_low = v as f32,
                9 => next.curve_mid = v as f32,
                10 => next.curve_high = v as f32,
                11 => next.link_mode = (v as i32).clamp(LINK_INDEPENDENT, LINK_LINKED),
                12 => next.transient_guard = v > 0.5,
                _ => {}
            })?;
        }
        self.params = Self::canonicalize_params(next, self.sample_rate, self.channels)?;
        self.reducer.set_enabled(self.params.enabled, false);
        self.spectral_reducer.set_enabled(self.params.enabled);
        self.refresh_backend_params();
        Ok(())
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
            .map_err(|error| format!("{id}: {error}"))?;
        if id.as_str() == "frequency_hz" {
            let frequency = value
                .as_float()
                .ok_or_else(|| "frequency_hz must be a float".to_string())?;
            let maximum = Self::maximum_frequency(self.sample_rate)?;
            if frequency > maximum {
                return Err(format!(
                    "frequency_hz must not exceed {maximum} Hz at sample rate {}",
                    self.sample_rate
                ));
            }
        }
        if id.as_str() == "spectral_mode" && self.initialized {
            let requested = value
                .as_bool()
                .ok_or_else(|| "spectral_mode must be a bool".to_string())?;
            if requested != self.params.spectral_mode {
                return Err(
                    "spectral_mode is structural and requires plugin reconstruction".to_string(),
                );
            }
        }
        Ok(())
    }

    fn parametric_set_parameter(
        &mut self,
        id: ParameterId,
        value: ParameterValue,
    ) -> PluginResult<()> {
        if self.drain_remaining.is_some() {
            return Err("Reset spectral hiss before changing parameters after drain starts".into());
        }
        self.parametric_validate_parameter(&id, &value)?;
        self.apply_parameter(id, value)
    }

    fn initialize(&mut self, sample_rate: u32) -> PluginResult<()> {
        self.params = Self::canonicalize_params(self.params.clone(), sample_rate, self.channels)?;
        self.sample_rate = sample_rate;
        self.reducer.initialize(sample_rate)?;
        self.spectral_reducer.set_enabled(self.params.enabled);
        self.spectral_reducer.initialize(sample_rate)?;
        self.reducer.set_params(
            self.params.frequency_hz,
            self.params.threshold_db,
            self.params.strength,
        );
        self.reducer.set_enabled(self.params.enabled, true);
        self.spectral_reducer.set_params(
            self.params.frequency_hz,
            self.params.threshold_db,
            self.params.strength,
        );
        self.refresh_backend_params();
        self.initialized = true;
        // A partial capture never survives reinitialization, but a stored
        // profile does: floors are broadband level references. Cross-rate
        // or cross-cutoff reuse shifts the measured band (~+1.8 dB for
        // white hiss from 48 to 96 kHz at fixed cutoff); the 6 dB
        // engagement margin absorbs this while depth shifts ~±2 dB, so
        // re-capture after rate/cutoff changes is guidance, not a hard
        // requirement (see README).
        self.capture.cancel();
        self.clear_drain();
        Ok(())
    }

    fn reset(&mut self) {
        self.reducer.reset();
        self.spectral_reducer.reset();
        // Reset discards a partial capture and preserves the stored profile
        // and settings, matching the denoiser capture contract.
        self.capture.cancel();
        self.clear_drain();
    }

    fn process_in_place(
        &mut self,
        buffer: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        let expected = context
            .num_frames
            .checked_mul(self.channels)
            .ok_or_else(|| "Frame/channel count overflow".to_string())?;
        if buffer.len() != expected {
            return Err(format!(
                "Buffer size mismatch: expected {}, got {}",
                expected,
                buffer.len()
            ));
        }

        if !self.initialized {
            return Err("plugin must be initialized before processing".to_string());
        }
        if context.sample_rate != self.sample_rate {
            return Err(format!(
                "sample rate mismatch: initialized at {}, context is {}",
                self.sample_rate, context.sample_rate
            ));
        }

        if context.num_frames > 0 && self.drain_remaining.is_some() {
            return Err("Reset spectral hiss before processing input after drain starts".into());
        }
        // Capture measures the input before either reducer runs, in both DSP
        // modes. Drain input never accumulates: capture is frozen there.
        if self.capture.is_active() && context.num_frames > 0 {
            self.capture.accumulate(buffer);
            // take_completed returns None without side effects while the
            // capture is incomplete, so no separate is_complete guard is
            // needed here.
            if let Some(summary) = self.capture.take_completed(&mut self.profile_floor_db) {
                self.has_profile = true;
                self.profile_sample_rate = summary.sample_rate;
                self.profile_cutoff_hz = summary.measurement_cutoff_hz;
                self.profile_frames = summary.frames_analyzed;
                self.refresh_backend_params();
            }
        }
        if self.params.spectral_mode {
            self.spectral_reducer.process(buffer);
            self.has_input |= context.num_frames > 0;
            self.source_phase = (self.source_phase + context.num_frames % DRAIN_HOP) % DRAIN_HOP;
        } else {
            // Keep filter and detector state warm while bypassed; HissReducer
            // owns the click-free wet/dry transition and reaches exact dry.
            self.reducer.process(buffer);
        }
        Ok(context.num_frames)
    }

    fn drain_output_frames_max(&self) -> usize {
        if self.params.spectral_mode {
            DRAIN_HOP
        } else {
            0
        }
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        if !(self.has_input && self.params.spectral_mode) {
            return std::num::NonZeroU64::new(1);
        }
        let hop = DRAIN_HOP;
        let remaining = self
            .drain_remaining
            .unwrap_or(2 * SPECTRAL_HISS_FFT_SIZE - hop + (hop - self.source_phase) % hop);
        // One call serves at most one canonical refill. A partially served
        // refill needs its own call even when fewer than one hop remains.
        let cached = self.drain_frames - self.drain_pos;
        let calls = usize::from(cached != 0) + remaining.saturating_sub(cached).div_ceil(hop);
        std::num::NonZeroU64::new(calls.max(1) as u64)
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<PluginDrainResult> {
        // Conventional IIR mode retains its legacy unknown-tail/no-drain path.
        if !self.params.spectral_mode {
            return Ok(PluginDrainResult::COMPLETE);
        }
        if !self.initialized || context.sample_rate != self.sample_rate {
            return Err("Spectral hiss drain requires its initialized sample rate".into());
        }
        if !self.has_input || self.drain_remaining == Some(0) {
            return Ok(PluginDrainResult::COMPLETE);
        }
        if output.is_empty() || !output.len().is_multiple_of(self.channels) {
            return Err("Spectral hiss drain requires a positive frame-aligned destination".into());
        }
        let remaining = self.drain_remaining.unwrap_or(
            2 * SPECTRAL_HISS_FFT_SIZE - DRAIN_HOP + (DRAIN_HOP - self.source_phase) % DRAIN_HOP,
        );
        if self.drain_pos == self.drain_frames {
            let frames = remaining.min(DRAIN_HOP);
            self.drain_cache[..frames * self.channels].fill(0.0);
            self.spectral_reducer
                .process(&mut self.drain_cache[..frames * self.channels]);
            self.drain_frames = frames;
            self.drain_pos = 0;
        }
        let frames = (output.len() / self.channels).min(self.drain_frames - self.drain_pos);
        let start = self.drain_pos * self.channels;
        output[..frames * self.channels]
            .copy_from_slice(&self.drain_cache[start..start + frames * self.channels]);
        self.drain_pos += frames;
        self.drain_remaining = Some(remaining - frames);
        Ok(PluginDrainResult {
            frames,
            complete: remaining == frames,
        })
    }

    fn tail_length(&self) -> TailLength {
        if self.params.spectral_mode {
            TailLength::Finite((2 * SPECTRAL_HISS_FFT_SIZE - 1) as u64)
        } else {
            TailLength::Unknown
        }
    }

    fn latency_samples(&self) -> usize {
        if self.params.spectral_mode {
            self.spectral_reducer.latency_samples()
        } else {
            self.reducer.latency_samples()
        }
    }
}
