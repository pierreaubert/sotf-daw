//! Verifies surround layout admission through the format bridge.

// Rust guideline compliant 2026-10-09

use plugins_bridge::create_plugin;
use serde_json::json;
use sotf_host::ProcessContext;
use sotf_host::speaker_config::{ChannelLayout, get_speaker_config};

#[test]
fn bridge_routes_surround_input_and_checks_host_width() {
    let layout = ChannelLayout::from_speaker_config(get_speaker_config("5.0").unwrap()).unwrap();
    let config = json!({"input_layout": layout, "speaker_config": "7.1.4", "fft_size": 512});
    let mut plugin = create_plugin("Upmixer", 5, 48_000.0, &config.to_string()).unwrap();
    plugin.initialize(48_000.0).unwrap();
    assert_eq!(plugin.input_channels(), 5);
    assert_eq!(plugin.output_channels(), 12);
    let frames = 1024;
    let mut input = vec![0.0; frames * 5];
    input[2] = 0.75;
    let mut output = vec![0.0; frames * 12];
    plugin
        .process(&input, &mut output, &ProcessContext::new(48_000, frames))
        .unwrap();
    assert_eq!(output[plugin.latency_samples() * 12 + 2], 0.75);
    assert!(create_plugin("Upmixer", 6, 48_000.0, &config.to_string()).is_err());
    assert!(create_plugin("Upmixer", 5, 48_000.0, "{}").is_err());
}
