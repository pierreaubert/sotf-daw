//! Public small-transform initialization, layout and stream regressions.
// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterValue, Plugin, ProcessContext};
use sotf_plugin_upmixer::{UpmixerPlugin, UpmixerPluginParams};

fn prepared(
    n: usize,
    layout: &str,
    rate: u32,
    mode: usize,
    bypass: bool,
    hr: bool,
) -> UpmixerPlugin {
    prepared_with_auto_gain(n, layout, rate, mode, bypass, hr, false)
}

fn prepared_with_auto_gain(
    n: usize,
    layout: &str,
    rate: u32,
    mode: usize,
    bypass: bool,
    hr: bool,
    auto_gain: bool,
) -> UpmixerPlugin {
    let params: UpmixerPluginParams = serde_json::from_value(serde_json::json!({
        "fft_size": n, "speaker_config": layout, "decorrelation_mode": mode,
        "bypass_decorrelation": bypass, "enable_hr_direct": hr,
        "auto_gain_enabled": auto_gain,
    }))
    .unwrap();
    let mut plugin = UpmixerPlugin::from_params(params);
    plugin.initialize(f64::from(rate)).unwrap();
    plugin
}

#[test]
fn fft_factory_normalizes_minimum_and_preserves_round_up_rules() {
    for (configured, expected) in [(0, 2), (1, 2), (2, 2), (3, 4)] {
        let params: UpmixerPluginParams =
            serde_json::from_value(serde_json::json!({ "fft_size": configured })).unwrap();
        let plugin = UpmixerPlugin::from_params(params);
        let diagnostics = plugin.diagnostics();
        assert_eq!(diagnostics.fft_size, expected, "configured={configured}");
        assert_eq!(
            diagnostics.hop_size,
            expected / 2,
            "configured={configured}"
        );
    }

    let params: UpmixerPluginParams = serde_json::from_value(serde_json::json!({
        "fft_size": 1,
        "low_latency": true
    }))
    .unwrap();
    let plugin = UpmixerPlugin::from_params(params);
    let diagnostics = plugin.diagnostics();
    assert_eq!(diagnostics.fft_size, 1024);
    assert_eq!(diagnostics.hop_size, 512);
}

#[test]
fn direct_constructor_rejects_fft_sizes_below_two() {
    for n in [0, 1] {
        assert!(
            std::panic::catch_unwind(|| {
                UpmixerPlugin::new(
                    n, "5.1", 1.0, 0.5, 1.0, 120.0, 0.5, 180.0, 1.0, 1.0, false, 0.0,
                )
            })
            .is_err(),
            "constructor accepted FFT size {n}"
        );
    }
}

fn source(frames: usize) -> Vec<f32> {
    (0..frames)
        .flat_map(|n| {
            let impulse = if n == 0 { 0.01 } else { 0.0 };
            [
                impulse + 0.003 * (n as f32 * 0.19).sin(),
                0.002 * (n as f32 * 0.11).cos(),
            ]
        })
        .collect()
}

fn render(plugin: &mut UpmixerPlugin, input: &[f32], rate: u32) -> Vec<f32> {
    let channels = plugin.output_channels();
    let mut output = vec![f32::NAN; input.len() / 2 * channels];
    let mut offset = 0;
    for size in [1, 17, 137, 8_193].into_iter().cycle() {
        let frames = size.min(input.len() / 2 - offset);
        if frames == 0 {
            break;
        }
        assert_eq!(
            plugin
                .process(
                    &input[offset * 2..(offset + frames) * 2],
                    &mut output[offset * channels..(offset + frames) * channels],
                    &ProcessContext::new(rate, frames)
                )
                .unwrap(),
            frames
        );
        offset += frames;
    }
    assert!(output.iter().all(|sample| sample.is_finite()));
    output
}

fn check_small(n: usize) {
    for rate in [44_100, 48_000, 96_000] {
        for (layout, channels) in [("5.1", 6), ("7.1.4", 12)] {
            for (mode, bypass) in [(0, false), (0, true), (1, false)] {
                for hr in [false, true] {
                    let mut plugin = prepared(n, layout, rate, mode, bypass, hr);
                    assert_eq!(plugin.output_channels(), channels);
                    assert_eq!(plugin.latency_samples(), n.max(512));
                    let input = source(8_193 + 137 + n);
                    let first = render(&mut plugin, &input, rate);
                    plugin.reset();
                    let reset = render(&mut plugin, &input, rate);
                    let mut fresh = prepared(n, layout, rate, mode, bypass, hr);
                    let expected = render(&mut fresh, &input, rate);
                    assert_eq!(first, expected, "deterministic preparation");
                    assert_eq!(
                        reset, expected,
                        "reset at n={n}, rate={rate}, layout={layout}, mode={mode}, hr={hr}"
                    );
                }
            }
        }
    }
}

#[test]
fn fft64_surround_initializes_and_processes_all_supported_routes() {
    check_small(64);
}

#[test]
fn fft128_surround_initializes_and_processes_all_supported_routes() {
    check_small(128);
}

#[test]
fn fft_sizes_two_through_thirty_two_process_and_reset_with_hr_and_autogain() {
    for n in [2, 4, 8, 16, 32] {
        for rate in [44_100, 96_000] {
            for (layout, channels) in [("5.1", 6), ("7.1.4", 12)] {
                for hr in [false, true] {
                    for auto_gain in [false, true] {
                        for mode in [0, 1] {
                            let mut plugin = prepared_with_auto_gain(
                                n, layout, rate, mode, false, hr, auto_gain,
                            );
                            assert_eq!(plugin.output_channels(), channels);
                            assert_eq!(plugin.latency_samples(), n.max(512));
                            assert_eq!(plugin.diagnostics().hop_size, n / 2);
                            // Cross both the 100 ms AutoGain refresh and two HR
                            // windows even at 96 kHz, where the interval is longest.
                            let frames = (rate / 10) as usize + 512 + n;
                            let input = source(frames);
                            let first = render(&mut plugin, &input, rate);
                            plugin.reset();
                            let reset = render(&mut plugin, &input, rate);
                            let mut fresh = prepared_with_auto_gain(
                                n, layout, rate, mode, false, hr, auto_gain,
                            );
                            let expected = render(&mut fresh, &input, rate);
                            assert_eq!(
                                first, expected,
                                "fresh at n={n}, rate={rate}, layout={layout}, mode={mode}, hr={hr}, auto_gain={auto_gain}"
                            );
                            assert_eq!(
                                reset, expected,
                                "reset at n={n}, rate={rate}, layout={layout}, mode={mode}, hr={hr}, auto_gain={auto_gain}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn changing_small_stereo_to_surround_prepares_the_new_width() {
    for (n, hr) in [(32, true), (64, false), (128, false)] {
        for layout in ["5.1", "7.1.4"] {
            let mut plugin = prepared(n, "2.0", 48_000, 0, false, hr);
            render(&mut plugin, &source(257), 48_000);
            let index = sotf_plugin_upmixer::params::SPEAKER_CONFIGS
                .iter()
                .position(|id| *id == layout)
                .unwrap();
            plugin
                .set_parameter("speaker_config".into(), ParameterValue::Int(index as i32))
                .unwrap();
            let mut fresh = prepared(n, layout, 48_000, 0, false, hr);
            assert_eq!(plugin.output_channels(), fresh.output_channels());
            assert_eq!(plugin.latency_samples(), n.max(512));
            let input = source(8_331);
            assert_eq!(
                render(&mut plugin, &input, 48_000),
                render(&mut fresh, &input, 48_000)
            );
        }
    }
}
