//! Verifies explicit surround layouts through the SOTF plugin factory.

// Rust guideline compliant 2026-10-09

use serde_json::json;
use sotf_host::ProcessContext;
use sotf_host::speaker_config::{ChannelLayout, get_speaker_config};
use sotf_plugins::factory::create_plugin;

#[test]
fn factory_routes_surround_bed_and_rejects_unmatched_width() {
    let layout = ChannelLayout::from_speaker_config(get_speaker_config("5.1").unwrap()).unwrap();
    let config = json!({"input_layout": layout, "speaker_config": "7.1.4", "fft_size": 512});
    let mut plugin = create_plugin("upmixer", &config, 6, 48_000).unwrap();
    plugin.initialize(48_000.0).unwrap();
    assert_eq!(plugin.input_channels(), 6);
    assert_eq!(plugin.output_channels(), 12);
    let frames = 1024;
    let mut input = vec![0.0; frames * 6];
    input[3] = 0.375;
    let mut output = vec![0.0; frames * 12];
    plugin
        .process(&input, &mut output, &ProcessContext::new(48_000, frames))
        .unwrap();
    assert_eq!(output[plugin.latency_samples() * 12 + 3], 0.375);
    assert!(create_plugin("upmixer", &config, 5, 48_000).is_err());
    assert!(create_plugin("upmixer", &json!({}), 6, 48_000).is_err());
}
