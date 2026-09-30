use super::PluginFuzzer;
use rand::RngExt;
use rand::rngs::StdRng;
use sotf_plugins::{BandSplitPlugin, BandSplitPluginParams, BandSplitRecombinationMode, Plugin};

pub(super) struct BandSplitFuzzer;

impl PluginFuzzer for BandSplitFuzzer {
    fn create_plugin(&self, channels: usize, rng: &mut StdRng) -> (Box<dyn Plugin>, String) {
        let frequency = rng.random_range(100.0..5000.0);
        let crossover_type = if rng.random_bool(0.5) { "LR24" } else { "LR48" };

        let params = BandSplitPluginParams {
            frequencies: vec![],
            explicit_frequencies: None,
            frequency_2: None,
            frequency_3: None,
            num_bands: 2,
            frequency,
            crossover_type: crossover_type.to_string(),
            recombination_mode: BandSplitRecombinationMode::LegacyCascade,
        };

        let plugin = BandSplitPlugin::from_params(channels, &params)
            .expect("Failed to create BandSplitPlugin");

        let desc = format!("freq={:.0}Hz type={}", frequency, crossover_type);

        (Box::new(plugin), desc)
    }
}
