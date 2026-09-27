use super::PluginFuzzer;
use rand::RngExt;
use rand::rngs::StdRng;
use sotf_plugins::{
    AnalogCompressorPlugin, AnalogCompressorPluginParams, ParametricInPlacePluginAdapter, Plugin,
};
use sotf_plugins::plugin_analog_common::MODEL_NAMES;

pub(super) struct AnalogCompressorFuzzer;

impl PluginFuzzer for AnalogCompressorFuzzer {
    fn create_plugin(&self, channels: usize, rng: &mut StdRng) -> (Box<dyn Plugin>, String) {
        let model_id = rng.random_range(0..MODEL_NAMES.len() as u32);
        let params = AnalogCompressorPluginParams {
            threshold: rng.random_range(-60.0..0.0),
            ratio: rng.random_range(1.0..20.0),
            attack: rng.random_range(0.1..100.0),
            release: rng.random_range(10.0..1000.0),
            knee: rng.random_range(0.0..20.0),
            makeup: rng.random_range(-24.0..24.0),
            mix: rng.random_range(0.0..1.0),
            auto_makeup: rng.random_bool(0.5),
            analog_model: MODEL_NAMES[model_id as usize].to_string(),
            analog_drive: rng.random_range(-60.0..36.0),
            analog_color: rng.random_range(0.0..1.0),
            analog_character: rng.random_range(0.0..1.0),
            analog_trim: rng.random_range(-24.0..24.0),
        };
        let desc = format!(
            "threshold={:.1}dB ratio={:.1} model={} drive={:.1}dB color={:.2}",
            params.threshold,
            params.ratio,
            params.analog_model,
            params.analog_drive,
            params.analog_color
        );
        let plugin = AnalogCompressorPlugin::try_from_params(channels, params)
            .expect("in-range analog compressor params must validate");
        (Box::new(ParametricInPlacePluginAdapter::new(plugin)), desc)
    }
}
