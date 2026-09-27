use super::PluginFuzzer;
use rand::RngExt;
use rand::rngs::StdRng;
use sotf_plugins::{
    AnalogEqPlugin, AnalogEqPluginParams, ParametricInPlacePluginAdapter, Plugin,
};
use sotf_plugins::plugin_analog_common::MODEL_NAMES;

pub(super) struct AnalogEqFuzzer;

impl PluginFuzzer for AnalogEqFuzzer {
    fn create_plugin(&self, channels: usize, rng: &mut StdRng) -> (Box<dyn Plugin>, String) {
        let model_id = rng.random_range(0..MODEL_NAMES.len() as u32);
        let params = AnalogEqPluginParams {
            low_freq: rng.random_range(20.0..500.0),
            low_gain: rng.random_range(-24.0..24.0),
            mid1_freq: rng.random_range(100.0..5000.0),
            mid1_gain: rng.random_range(-24.0..24.0),
            mid1_q: rng.random_range(0.1..10.0),
            mid2_freq: rng.random_range(500.0..12000.0),
            mid2_gain: rng.random_range(-24.0..24.0),
            mid2_q: rng.random_range(0.1..10.0),
            high_freq: rng.random_range(2000.0..20000.0),
            high_gain: rng.random_range(-24.0..24.0),
            analog_model: MODEL_NAMES[model_id as usize].to_string(),
            analog_drive: rng.random_range(-60.0..36.0),
            analog_color: rng.random_range(0.0..1.0),
            analog_character: rng.random_range(0.0..1.0),
            analog_trim: rng.random_range(-24.0..24.0),
        };
        let desc = format!(
            "model={} drive={:.1}dB color={:.2}",
            params.analog_model, params.analog_drive, params.analog_color
        );
        let plugin = AnalogEqPlugin::try_from_params(channels, params)
            .expect("in-range analog EQ params must validate");
        (Box::new(ParametricInPlacePluginAdapter::new(plugin)), desc)
    }
}
