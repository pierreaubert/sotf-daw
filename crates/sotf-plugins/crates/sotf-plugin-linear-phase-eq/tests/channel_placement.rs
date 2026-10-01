//! Public-API behavior checks for per-band channel/Mid/Side routing (R1/A3).
//!
//! Exactness against the independent cascade reference lives in the unit
//! suite; these tests exercise the same behavior through the public
//! constructor/processing/response API only.

use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_linear_phase_eq::{
    BandConfig, LinearPhaseEqBandPlacement as Placement, LinearPhaseEqPlugin,
    LinearPhaseEqPluginParams,
};

fn placed_band(
    filter_type: &str,
    frequency: f64,
    gain_db: f64,
    placement: Placement,
) -> BandConfig {
    BandConfig {
        filter_type: filter_type.to_string(),
        frequency,
        q: 1.0,
        gain_db,
        active: true,
        placement: Some(placement),
    }
}

fn params_for(bands: Vec<BandConfig>) -> LinearPhaseEqPluginParams {
    let num_filters = bands.len();
    LinearPhaseEqPluginParams {
        num_filters,
        fir_length_index: 0,
        phase_mode_index: 0,
        auto_gain: false,
        mix: 1.0,
        filters: bands,
        stereo_pairs: None,
    }
}

fn sine(frames: usize, channels: usize, freq: f32, rate: u32, right_sign: f32) -> Vec<f32> {
    let mut buffer = vec![0.0; frames * channels];
    for frame in 0..frames {
        let t = frame as f32 / rate as f32;
        let sample = (2.0 * std::f32::consts::PI * freq * t).sin() * 0.5;
        buffer[frame * channels] = sample;
        if channels > 1 {
            buffer[frame * channels + 1] = sample * right_sign;
        }
    }
    buffer
}

fn stream_all(plugin: &mut LinearPhaseEqPlugin, input: &[f32], rate: u32) -> Vec<f32> {
    let channels = plugin.channels();
    let frames = input.len() / channels;
    let mut output = input.to_vec();
    let mut position = 0;
    while position < frames {
        let count = 256.min(frames - position);
        plugin
            .process_in_place(
                &mut output[position * channels..(position + count) * channels],
                &ProcessContext::new(rate, count),
            )
            .unwrap();
        position += count;
    }
    output
}

fn channel_rms(output: &[f32], channels: usize, channel: usize, from: usize, to: usize) -> f64 {
    let mut sum = 0.0;
    let mut count = 0;
    for frame in from..to {
        let sample = f64::from(output[frame * channels + channel]);
        sum += sample * sample;
        count += 1;
    }
    (sum / count as f64).sqrt()
}

#[test]
fn left_band_processes_left_only() {
    let rate = 48_000;
    let mut plugin = LinearPhaseEqPlugin::from_params(
        2,
        rate,
        params_for(vec![placed_band("Peak", 1000.0, 12.0, Placement::Left)]),
    )
    .unwrap();
    assert!(plugin.is_ordered_route());
    let input = sine(4096, 2, 1000.0, rate, 1.0);
    let output = stream_all(&mut plugin, &input, rate);
    let input_rms = channel_rms(&input, 2, 0, 1500, 4000);
    let left_rms = channel_rms(&output, 2, 0, 1500, 4000);
    let right_rms = channel_rms(&output, 2, 1, 1500, 4000);
    // +12 dB is ~4x; the FIR approximation holds well within a 3x floor.
    assert!(
        left_rms > input_rms * 3.0,
        "left channel must be boosted: {left_rms} vs {input_rms}"
    );
    // The right channel passes through an identity delay: same level.
    assert!(
        (right_rms - input_rms).abs() / input_rms < 0.02,
        "right channel must pass through: {right_rms} vs {input_rms}"
    );
}

#[test]
fn right_band_processes_right_only() {
    let rate = 48_000;
    let mut plugin = LinearPhaseEqPlugin::from_params(
        2,
        rate,
        params_for(vec![placed_band("Peak", 1000.0, 12.0, Placement::Right)]),
    )
    .unwrap();
    let input = sine(4096, 2, 1000.0, rate, 1.0);
    let output = stream_all(&mut plugin, &input, rate);
    let input_rms = channel_rms(&input, 2, 0, 1500, 4000);
    assert!(channel_rms(&output, 2, 1, 1500, 4000) > input_rms * 3.0);
    assert!((channel_rms(&output, 2, 0, 1500, 4000) - input_rms).abs() / input_rms < 0.02);
}

#[test]
fn mid_band_boosts_correlated_not_anticorrelated() {
    let rate = 48_000;
    // Correlated stereo lives entirely in Mid: fully boosted.
    let mut plugin = LinearPhaseEqPlugin::from_params(
        2,
        rate,
        params_for(vec![placed_band("Peak", 1000.0, 12.0, Placement::Mid)]),
    )
    .unwrap();
    let input = sine(4096, 2, 1000.0, rate, 1.0);
    let output = stream_all(&mut plugin, &input, rate);
    let input_rms = channel_rms(&input, 2, 0, 1500, 4000);
    assert!(channel_rms(&output, 2, 0, 1500, 4000) > input_rms * 3.0);
    assert!(channel_rms(&output, 2, 1, 1500, 4000) > input_rms * 3.0);

    // Anticorrelated stereo lives entirely in Side: untouched.
    let mut plugin = LinearPhaseEqPlugin::from_params(
        2,
        rate,
        params_for(vec![placed_band("Peak", 1000.0, 12.0, Placement::Mid)]),
    )
    .unwrap();
    let input = sine(4096, 2, 1000.0, rate, -1.0);
    let output = stream_all(&mut plugin, &input, rate);
    for channel in 0..2 {
        let rms = channel_rms(&output, 2, channel, 1500, 4000);
        assert!(
            (rms - input_rms).abs() / input_rms < 0.02,
            "anticorrelated channel {channel} must pass through: {rms} vs {input_rms}"
        );
    }
}

#[test]
fn side_band_boosts_anticorrelated_not_correlated() {
    let rate = 48_000;
    let mut plugin = LinearPhaseEqPlugin::from_params(
        2,
        rate,
        params_for(vec![placed_band("Peak", 1000.0, 12.0, Placement::Side)]),
    )
    .unwrap();
    let input = sine(4096, 2, 1000.0, rate, -1.0);
    let output = stream_all(&mut plugin, &input, rate);
    let input_rms = channel_rms(&input, 2, 0, 1500, 4000);
    assert!(channel_rms(&output, 2, 0, 1500, 4000) > input_rms * 3.0);
    assert!(channel_rms(&output, 2, 1, 1500, 4000) > input_rms * 3.0);

    let mut plugin = LinearPhaseEqPlugin::from_params(
        2,
        rate,
        params_for(vec![placed_band("Peak", 1000.0, 12.0, Placement::Side)]),
    )
    .unwrap();
    let input = sine(4096, 2, 1000.0, rate, 1.0);
    let output = stream_all(&mut plugin, &input, rate);
    for channel in 0..2 {
        let rms = channel_rms(&output, 2, channel, 1500, 4000);
        assert!(
            (rms - input_rms).abs() / input_rms < 0.02,
            "correlated channel {channel} must pass through: {rms} vs {input_rms}"
        );
    }
}

#[test]
fn stereo_pairs_required_errors_are_public() {
    let params = params_for(vec![placed_band("Peak", 1000.0, 6.0, Placement::Left)]);
    let Err(error) = LinearPhaseEqPlugin::from_params(4, 48_000, params) else {
        panic!("expected an explicit stereo_pairs error");
    };
    assert!(error.contains("explicit stereo_pairs"), "{error}");
}

#[test]
fn legacy_json_and_placement_schema_are_public() {
    let params: LinearPhaseEqPluginParams = serde_json::from_value(serde_json::json!({
        "num_filters": 1,
        "filters": [{"filter_type": "Peak", "frequency": 1000.0}],
    }))
    .unwrap();
    assert_eq!(params.stereo_pairs, None);
    let mut plugin = LinearPhaseEqPlugin::from_params(2, 48_000, params).unwrap();
    assert!(!plugin.is_ordered_route());
    assert!(
        plugin
            .parameters()
            .iter()
            .any(|p| p.id.as_str() == "band_0_placement")
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("band_0_placement")),
        Some(ParameterValue::Int(0))
    );
    assert!(
        plugin
            .set_parameter(
                ParameterId::from("band_0_placement"),
                ParameterValue::Int(4)
            )
            .is_err()
    );
}

#[test]
fn ordered_topology_is_visible_via_public_api() {
    let plugin = LinearPhaseEqPlugin::from_params(
        2,
        48_000,
        params_for(vec![
            placed_band("Peak", 1000.0, 6.0, Placement::Left),
            placed_band("Lowshelf", 300.0, -3.0, Placement::Mid),
        ]),
    )
    .unwrap();
    assert!(plugin.is_ordered_route());
    assert_eq!(plugin.stage_count(), 2);
    assert_eq!(plugin.stereo_pairs(), &[[0, 1]]);
    assert_eq!(plugin.band_placement(0), Some(Some(Placement::Left)));
    assert_eq!(plugin.band_placement(1), Some(Some(Placement::Mid)));
    assert_eq!(plugin.band_placement(2), None);
    assert_eq!(plugin.latency_samples(), 2 * (1024 / 2 + 32));
    assert_eq!(plugin.stage_fir(0).map(<[f32]>::len), Some(1024));
    assert_eq!(plugin.stage_fir(2), None);
    // The chart-facing response resolves per channel.
    let left = plugin.channel_complex_response(0, 1000.0).unwrap();
    let right = plugin.channel_complex_response(1, 1000.0).unwrap();
    assert!((left.re.hypot(left.im) - right.re.hypot(right.im)).abs() > 0.1);
    assert!(plugin.channel_complex_response(2, 1000.0).is_none());
}
