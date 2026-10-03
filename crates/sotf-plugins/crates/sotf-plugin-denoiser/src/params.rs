//! Classical spectral denoiser parameter definitions.

use serde::{Deserialize, Serialize};
use sotf_host::param_specs::ParamSpec;
use sotf_host::plugin_layout::*;
use sotf_host::plugin_params::PluginParamDef;

mod consts;
mod d;
#[cfg(test)]
mod tests;

pub use consts::*;

use d::d_attack_ms;
use d::d_audition_residual;
use d::d_clear_profile;
use d::d_curve_high;
use d::d_curve_low;
use d::d_curve_mid;
use d::d_dd_alpha;
use d::d_dd_enabled;
use d::d_floor_db;
use d::d_formant_preservation;
use d::d_formant_strength;
use d::d_learn_noise;
use d::d_low_latency;
use d::d_mcra_alpha_p;
use d::d_mcra_alpha_s;
use d::d_mcra_delta;
use d::d_mcra_l;
use d::d_multi_resolution;
use d::d_polyphonic_detection;
use d::d_psychoacoustic_masking;
use d::d_reduction_db;
use d::d_release_ms;
use d::d_smoothing;
use d::d_spatial_strength;
use d::d_spectral_smoothing_enabled;
use d::d_spectral_sub_alpha;
use d::d_spectral_sub_beta;
use d::d_spectral_sub_enabled;
use d::d_temporal_smoothing_enabled;
use d::d_transparency;
use d::d_use_captured_profile;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Params {
    #[serde(default = "d_reduction_db")]
    pub reduction_db: f64,
    #[serde(default = "d_floor_db")]
    pub floor_db: f64,
    #[serde(default = "d_smoothing")]
    pub smoothing: f64,
    #[serde(default = "d_attack_ms")]
    pub attack_ms: f64,
    #[serde(default = "d_release_ms")]
    pub release_ms: f64,
    #[serde(default = "d_low_latency")]
    pub low_latency: bool,
    #[serde(default = "d_polyphonic_detection")]
    pub polyphonic_detection: bool,
    #[serde(default = "d_mcra_alpha_s")]
    pub mcra_alpha_s: f64,
    #[serde(default = "d_mcra_alpha_p")]
    pub mcra_alpha_p: f64,
    #[serde(default = "d_mcra_l")]
    pub mcra_l: usize,
    #[serde(default = "d_mcra_delta")]
    pub mcra_delta: f64,
    #[serde(default = "d_transparency")]
    pub transparency: f64,
    #[serde(default = "d_dd_enabled")]
    pub dd_enabled: bool,
    #[serde(default = "d_dd_alpha")]
    pub dd_alpha: f64,
    #[serde(default = "d_psychoacoustic_masking")]
    pub psychoacoustic_masking: bool,
    #[serde(default = "d_spectral_smoothing_enabled")]
    pub spectral_smoothing_enabled: bool,
    #[serde(default = "d_temporal_smoothing_enabled")]
    pub temporal_smoothing_enabled: bool,
    #[serde(default = "d_spectral_sub_enabled")]
    pub spectral_sub_enabled: bool,
    #[serde(default = "d_spectral_sub_alpha")]
    pub spectral_sub_alpha: f64,
    #[serde(default = "d_spectral_sub_beta")]
    pub spectral_sub_beta: f64,
    #[serde(default = "d_learn_noise")]
    pub learn_noise: bool,
    #[serde(default = "d_use_captured_profile")]
    pub use_captured_profile: bool,
    #[serde(default = "d_clear_profile")]
    pub clear_profile: bool,
    #[serde(default = "d_formant_preservation")]
    pub formant_preservation: bool,
    #[serde(default = "d_formant_strength")]
    pub formant_strength: f64,
    #[serde(default = "d_multi_resolution")]
    pub multi_resolution: bool,
    #[serde(default)]
    pub harmonic_percussive: bool,
    #[serde(default)]
    pub spatial_denoise: bool,
    #[serde(default = "d_spatial_strength")]
    pub spatial_strength: f64,
    #[serde(default = "d_curve_low")]
    pub curve_low: f64,
    #[serde(default = "d_curve_mid")]
    pub curve_mid: f64,
    #[serde(default = "d_curve_high")]
    pub curve_high: f64,
    #[serde(default = "d_audition_residual")]
    pub audition_residual: bool,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            reduction_db: d_reduction_db(),
            floor_db: d_floor_db(),
            smoothing: d_smoothing(),
            attack_ms: d_attack_ms(),
            release_ms: d_release_ms(),
            low_latency: d_low_latency(),
            polyphonic_detection: d_polyphonic_detection(),
            mcra_alpha_s: d_mcra_alpha_s(),
            mcra_alpha_p: d_mcra_alpha_p(),
            mcra_l: d_mcra_l(),
            mcra_delta: d_mcra_delta(),
            transparency: d_transparency(),
            dd_enabled: d_dd_enabled(),
            dd_alpha: d_dd_alpha(),
            psychoacoustic_masking: d_psychoacoustic_masking(),
            spectral_smoothing_enabled: d_spectral_smoothing_enabled(),
            temporal_smoothing_enabled: d_temporal_smoothing_enabled(),
            spectral_sub_enabled: d_spectral_sub_enabled(),
            spectral_sub_alpha: d_spectral_sub_alpha(),
            spectral_sub_beta: d_spectral_sub_beta(),
            learn_noise: d_learn_noise(),
            use_captured_profile: d_use_captured_profile(),
            clear_profile: d_clear_profile(),
            formant_preservation: d_formant_preservation(),
            formant_strength: d_formant_strength(),
            multi_resolution: d_multi_resolution(),
            harmonic_percussive: false,
            spatial_denoise: false,
            spatial_strength: d_spatial_strength(),
            curve_low: d_curve_low(),
            curve_mid: d_curve_mid(),
            curve_high: d_curve_high(),
            audition_residual: d_audition_residual(),
        }
    }
}

impl PluginParamDef for Params {
    const PARAMS: &'static [ParamSpec] = PARAMS;
    const LAYOUT: Option<&'static PluginLayout> = Some(&LAYOUT);
    const VERSION: u32 = 2;
    const PLUGIN_TYPE_KEY: &'static str = "denoiser";

    fn param_value(&self, index: usize) -> Option<f64> {
        match index {
            0 => Some(self.reduction_db),
            1 => Some(self.floor_db),
            2 => Some(self.smoothing),
            3 => Some(self.attack_ms),
            4 => Some(self.release_ms),
            5 => Some(if self.low_latency { 1.0 } else { 0.0 }),
            6 => Some(if self.polyphonic_detection { 1.0 } else { 0.0 }),
            7 => Some(self.mcra_alpha_s),
            8 => Some(self.mcra_alpha_p),
            9 => Some(self.mcra_l as f64),
            10 => Some(self.mcra_delta),
            11 => Some(self.transparency),
            12 => Some(if self.dd_enabled { 1.0 } else { 0.0 }),
            13 => Some(self.dd_alpha),
            14 => Some(if self.psychoacoustic_masking {
                1.0
            } else {
                0.0
            }),
            15 => Some(if self.spectral_smoothing_enabled {
                1.0
            } else {
                0.0
            }),
            16 => Some(if self.temporal_smoothing_enabled {
                1.0
            } else {
                0.0
            }),
            17 => Some(if self.spectral_sub_enabled { 1.0 } else { 0.0 }),
            18 => Some(self.spectral_sub_alpha),
            19 => Some(self.spectral_sub_beta),
            20 => Some(if self.learn_noise { 1.0 } else { 0.0 }),
            21 => Some(if self.use_captured_profile { 1.0 } else { 0.0 }),
            22 => Some(if self.clear_profile { 1.0 } else { 0.0 }),
            23 => Some(if self.formant_preservation { 1.0 } else { 0.0 }),
            24 => Some(self.formant_strength),
            25 => Some(if self.multi_resolution { 1.0 } else { 0.0 }),
            26 => Some(if self.harmonic_percussive { 1.0 } else { 0.0 }),
            27 => Some(if self.spatial_denoise { 1.0 } else { 0.0 }),
            28 => Some(self.spatial_strength),
            29 => Some(self.curve_low),
            30 => Some(self.curve_mid),
            31 => Some(self.curve_high),
            32 => Some(if self.audition_residual { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    fn set_param_value(&mut self, index: usize, value: f64) {
        match index {
            0 => self.reduction_db = PARAMS[0].clamp_f64(value),
            1 => self.floor_db = PARAMS[1].clamp_f64(value),
            2 => self.smoothing = PARAMS[2].clamp_f64(value),
            3 => self.attack_ms = PARAMS[3].clamp_f64(value),
            4 => self.release_ms = PARAMS[4].clamp_f64(value),
            5 => self.low_latency = PARAMS[5].clamp_f64(value) > 0.5,
            6 => self.polyphonic_detection = PARAMS[6].clamp_f64(value) > 0.5,
            7 => self.mcra_alpha_s = PARAMS[7].clamp_f64(value),
            8 => self.mcra_alpha_p = PARAMS[8].clamp_f64(value),
            9 => self.mcra_l = PARAMS[9].clamp_f64(value) as usize,
            10 => self.mcra_delta = PARAMS[10].clamp_f64(value),
            11 => self.transparency = PARAMS[11].clamp_f64(value),
            12 => self.dd_enabled = PARAMS[12].clamp_f64(value) > 0.5,
            13 => self.dd_alpha = PARAMS[13].clamp_f64(value),
            14 => self.psychoacoustic_masking = PARAMS[14].clamp_f64(value) > 0.5,
            15 => self.spectral_smoothing_enabled = PARAMS[15].clamp_f64(value) > 0.5,
            16 => self.temporal_smoothing_enabled = PARAMS[16].clamp_f64(value) > 0.5,
            17 => self.spectral_sub_enabled = PARAMS[17].clamp_f64(value) > 0.5,
            18 => self.spectral_sub_alpha = PARAMS[18].clamp_f64(value),
            19 => self.spectral_sub_beta = PARAMS[19].clamp_f64(value),
            20 => self.learn_noise = PARAMS[20].clamp_f64(value) > 0.5,
            21 => self.use_captured_profile = PARAMS[21].clamp_f64(value) > 0.5,
            22 => self.clear_profile = PARAMS[22].clamp_f64(value) > 0.5,
            23 => self.formant_preservation = PARAMS[23].clamp_f64(value) > 0.5,
            24 => self.formant_strength = PARAMS[24].clamp_f64(value),
            25 => self.multi_resolution = PARAMS[25].clamp_f64(value) > 0.5,
            26 => self.harmonic_percussive = PARAMS[26].clamp_f64(value) > 0.5,
            27 => self.spatial_denoise = PARAMS[27].clamp_f64(value) > 0.5,
            28 => self.spatial_strength = PARAMS[28].clamp_f64(value),
            29 => self.curve_low = PARAMS[29].clamp_f64(value),
            30 => self.curve_mid = PARAMS[30].clamp_f64(value),
            31 => self.curve_high = PARAMS[31].clamp_f64(value),
            32 => self.audition_residual = PARAMS[32].clamp_f64(value) > 0.5,
            _ => {}
        }
    }
}
