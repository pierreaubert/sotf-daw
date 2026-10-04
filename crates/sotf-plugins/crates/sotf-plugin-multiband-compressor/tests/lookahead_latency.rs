//! Independent dry-path delay and public latency regression.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParametricInPlacePluginAdapter, Plugin, ProcessContext};
use sotf_plugin_multiband_compressor::{
    MultibandCompressorPlugin, MultibandCompressorPluginParams,
};

fn verify_dry_delay(plugin: &mut dyn Plugin, rate: u32, channels: usize, delay: usize) {
    // Dyadic values make the expected pure-delay waveform exact in f32.
    let input: Vec<f32> = (0..(delay + 37) * channels)
        .map(|i| ((i * 7 % 23) as f32 - 11.0) / 64.0)
        .collect();
    let mut padded = input.clone();
    padded.resize(input.len() + delay * channels, 0.0);
    let mut expected = vec![0.0; delay * channels];
    expected.extend_from_slice(&input);
    for partitions in [&[1, 17, 64, 257][..], &[8193][..]] {
        plugin.reset();
        let mut output = vec![0.0; padded.len()];
        let total_frames = padded.len() / channels;
        let mut start = 0;
        let mut block = 0;
        while start < total_frames {
            let frames = partitions[block % partitions.len()].min(total_frames - start);
            let span = start * channels..(start + frames) * channels;
            assert_eq!(
                plugin
                    .process(
                        &padded[span.clone()],
                        &mut output[span],
                        &ProcessContext::new(rate, frames)
                    )
                    .unwrap(),
                frames,
            );
            start += frames;
            block += 1;
        }
        assert_eq!(
            output, expected,
            "rate={rate}, channels={channels}, delay={delay}, partitions={partitions:?}"
        );
        assert_eq!(
            plugin.latency_samples(),
            delay,
            "reported latency must match the emitted dry waveform at {rate} Hz"
        );
    }
}

#[test]
fn reported_lookahead_matches_dry_audio_at_every_supported_test_rate() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for channels in [1, 2, 6] {
            for bands in [1, 3] {
                for lookahead_ms in [0.0_f32, 0.001, 0.01, 1.0, 20.0] {
                    let params = MultibandCompressorPluginParams {
                        num_bands: bands,
                        per_band_lookahead_ms: lookahead_ms,
                        mix: 0.0,
                        ..Default::default()
                    };
                    let mut plugin = ParametricInPlacePluginAdapter::new(
                        MultibandCompressorPlugin::from_params(channels, params),
                    );
                    Plugin::initialize(&mut plugin, f64::from(rate)).unwrap();
                    // A positive active delay is at least one sample; zero disables it.
                    let delay = if lookahead_ms == 0.0 {
                        0
                    } else {
                        ((f64::from(lookahead_ms) * f64::from(rate) / 1000.0).round() as usize)
                            .max(1)
                    };
                    verify_dry_delay(&mut plugin, rate, channels, delay);
                }
            }
        }
    }
}
