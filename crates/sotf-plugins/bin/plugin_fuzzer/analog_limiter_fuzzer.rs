use super::PluginFuzzer;
use rand::RngExt;
use rand::rngs::StdRng;
use sotf_plugins::plugin_analog_common::MODEL_NAMES;
use sotf_plugins::{
    AnalogLimiterPlugin, AnalogLimiterPluginParams, ParametricInPlacePluginAdapter, Plugin,
};

pub(super) struct AnalogLimiterFuzzer;

impl PluginFuzzer for AnalogLimiterFuzzer {
    fn create_plugin(&self, channels: usize, rng: &mut StdRng) -> (Box<dyn Plugin>, String) {
        let model_id = rng.random_range(0..MODEL_NAMES.len() as u32);
        let params = AnalogLimiterPluginParams {
            threshold: rng.random_range(-20.0..0.0),
            release: rng.random_range(10.0..1000.0),
            lookahead: rng.random_range(0.0..20.0),
            soft: rng.random_bool(0.5),
            true_peak: rng.random_bool(0.5),
            mix: rng.random_range(0.0..1.0),
            analog_model: MODEL_NAMES[model_id as usize].to_string(),
            analog_drive: rng.random_range(-60.0..36.0),
            analog_color: rng.random_range(0.0..1.0),
            analog_character: rng.random_range(0.0..1.0),
            analog_trim: rng.random_range(-24.0..24.0),
        };
        let desc = format!(
            "threshold={:.1}dB model={} drive={:.1}dB color={:.2}",
            params.threshold, params.analog_model, params.analog_drive, params.analog_color
        );
        let plugin = AnalogLimiterPlugin::try_from_params(channels, params)
            .expect("in-range analog limiter params must validate");
        (Box::new(ParametricInPlacePluginAdapter::new(plugin)), desc)
    }
}
