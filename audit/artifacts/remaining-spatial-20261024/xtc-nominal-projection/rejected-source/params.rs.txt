//! Canonical XTC parameter schema.
//!
//! `Params` is an alias of the runtime/factory configuration, so serde,
//! factory construction, UI metadata, presets, and DSP dispatch cannot drift.

use sotf_host::plugin_layout::PluginLayout;
use sotf_host::plugin_params::PluginParamDef;

pub(crate) mod consts;
#[cfg(test)]
mod tests;

pub use consts::*;
pub type Params = crate::config::XtcPluginParams;

impl PluginParamDef for crate::config::XtcPluginParams {
    const PARAMS: &'static [sotf_host::param_specs::ParamSpec] = PARAMS;
    const LAYOUT: Option<&'static PluginLayout> = Some(&LAYOUT);
    const VERSION: u32 = 2;
    const PLUGIN_TYPE_KEY: &'static str = "xtc";

    fn param_value(&self, index: usize) -> Option<f64> {
        match index {
            0 => Some(self.distance_m as f64),
            1 => Some(self.speaker_angle_deg as f64),
            2 => Some(self.head_radius_m as f64),
            3 => Some(self.head_offset_x as f64),
            4 => Some(self.head_offset_z as f64),
            5 => Some(self.head_yaw_deg as f64),
            6 => Some(self.head_tracking_smooth_s as f64),
            7 => Some(self.beta_base as f64),
            8 => Some(self.beta_low_freq_boost as f64),
            9 => Some(self.beta_high_freq_boost as f64),
            10 => Some(self.head_shadow_cutoff_hz as f64),
            11 => Some(self.head_shadow_slope_db_per_octave as f64),
            12 => Some(self.max_gain_db as f64),
            13 => Some(self.spectral_normalization as u8 as f64),
            14 => Some(self.pinna_model_enabled as u8 as f64),
            15 => Some(self.room_reflections_enabled as u8 as f64),
            16 => None, // room_ir_file (FilePath — handled separately)
            17 => Some(self.room_width_m as f64),
            18 => Some(self.room_depth_m as f64),
            19 => Some(self.wall_absorption as f64),
            20 => Some(self.reflection_beta_boost as f64),
            21 => Some(self.bypass_xtc_filters as u8 as f64),
            22 => Some(self.bypass_spectral_normalization as u8 as f64),
            23 => Some(self.bypass_neumann_refinement as u8 as f64),
            24 => Some(self.auto_gain_enabled as u8 as f64),
            25 => Some(self.auto_gain_max_db as f64),
            26 => Some(self.auto_gain_smoothing_ms as f64),
            27 => Some(self.head_model as f64),
            _ => None,
        }
    }

    fn set_param_value(&mut self, index: usize, value: f64) {
        // Values are clamped against PARAMS so the raw indexed path enforces
        // the same ranges as the named path (param_bridge::set_parameter).
        // Index 16 (room_ir_file) is a FilePath param handled separately and
        // is intentionally not settable here — see param_value above.
        match index {
            0 => self.distance_m = PARAMS[0].clamp_f64(value) as f32,
            1 => self.speaker_angle_deg = PARAMS[1].clamp_f64(value) as f32,
            2 => self.head_radius_m = PARAMS[2].clamp_f64(value) as f32,
            3 => self.head_offset_x = PARAMS[3].clamp_f64(value) as f32,
            4 => self.head_offset_z = PARAMS[4].clamp_f64(value) as f32,
            5 => self.head_yaw_deg = PARAMS[5].clamp_f64(value) as f32,
            6 => self.head_tracking_smooth_s = PARAMS[6].clamp_f64(value) as f32,
            7 => self.beta_base = PARAMS[7].clamp_f64(value) as f32,
            8 => self.beta_low_freq_boost = PARAMS[8].clamp_f64(value) as f32,
            9 => self.beta_high_freq_boost = PARAMS[9].clamp_f64(value) as f32,
            10 => self.head_shadow_cutoff_hz = PARAMS[10].clamp_f64(value) as f32,
            11 => self.head_shadow_slope_db_per_octave = PARAMS[11].clamp_f64(value) as f32,
            12 => self.max_gain_db = PARAMS[12].clamp_f64(value) as f32,
            13 => self.spectral_normalization = PARAMS[13].clamp_f64(value) > 0.5,
            14 => self.pinna_model_enabled = PARAMS[14].clamp_f64(value) > 0.5,
            15 => self.room_reflections_enabled = PARAMS[15].clamp_f64(value) > 0.5,
            16 => {} // room_ir_file (FilePath — handled separately)
            17 => self.room_width_m = PARAMS[17].clamp_f64(value) as f32,
            18 => self.room_depth_m = PARAMS[18].clamp_f64(value) as f32,
            19 => self.wall_absorption = PARAMS[19].clamp_f64(value) as f32,
            20 => self.reflection_beta_boost = PARAMS[20].clamp_f64(value) as f32,
            21 => self.bypass_xtc_filters = PARAMS[21].clamp_f64(value) > 0.5,
            22 => self.bypass_spectral_normalization = PARAMS[22].clamp_f64(value) > 0.5,
            23 => self.bypass_neumann_refinement = PARAMS[23].clamp_f64(value) > 0.5,
            24 => self.auto_gain_enabled = PARAMS[24].clamp_f64(value) > 0.5,
            25 => self.auto_gain_max_db = PARAMS[25].clamp_f64(value) as f32,
            26 => self.auto_gain_smoothing_ms = PARAMS[26].clamp_f64(value) as f32,
            27 => self.head_model = PARAMS[27].clamp_f64(value) as usize,
            _ => {}
        }
    }
}
