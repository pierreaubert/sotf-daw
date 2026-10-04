//! Reset restarts the existing callback-counted AutoGain measurement schedule.
// Rust guideline compliant 2026-02-21
use sotf_host::plugin::PluginCompiledOp;
use sotf_host::{ParametricPlugin, ProcessContext};
use sotf_plugin_eq::{EqPlugin, EqPluginParams};

fn plugin(rate: u32) -> EqPlugin {
    let params: EqPluginParams = serde_json::from_value(serde_json::json!({
        "filters": [{"filter_type": "peak", "freq": 1000.0, "q": 1.0, "db_gain": 9.0}],
        "auto_gain": {"enabled": true, "smoothing_ms": 100.0}
    }))
    .unwrap();
    let mut plugin = EqPlugin::from_params(2, rate, params).unwrap();
    plugin.plugin_initialize(f64::from(rate)).unwrap();
    plugin
}

fn render(
    plugin: &mut EqPlugin,
    input: &[f32],
    rate: u32,
    block: usize,
    compiled: bool,
) -> Vec<f32> {
    let mut output = vec![0.0; input.len()];
    for (source, destination) in input.chunks(block * 2).zip(output.chunks_mut(block * 2)) {
        let context = ProcessContext::new(rate, source.len() / 2);
        let frames = if compiled {
            plugin
                .process_compiled_f32(
                    PluginCompiledOp::EqBiquadBank,
                    source,
                    destination,
                    &context,
                )
                .expect("the compiled operation must be exercised")
                .unwrap()
        } else {
            plugin.process(source, destination, &context).unwrap()
        };
        assert_eq!(frames, context.num_frames);
    }
    output
}

#[test]
fn warmed_reset_matches_fresh_audio_at_nonzero_measurement_callback_phases() {
    for rate in [48_000, 96_000] {
        let input: Vec<_> = (0..rate as usize * 6)
            .flat_map(|n| {
                let x = (0.02 * (std::f64::consts::TAU * 1000.0 * n as f64 / f64::from(rate)).sin())
                    as f32;
                [x, -0.5 * x]
            })
            .collect();
        for block in [137, 512] {
            for compiled in [false, true] {
                let expected = render(&mut plugin(rate), &input, rate, block, compiled);
                for prior_calls in [1, 7, 9] {
                    let mut processor = plugin(rate);
                    // Neither initialization nor a multiple-of-ten callback count
                    // may accidentally stand in for resetting the measurement phase.
                    render(
                        &mut processor,
                        &input[..prior_calls * block * 2],
                        rate,
                        block,
                        compiled,
                    );
                    processor.plugin_reset();
                    let actual = render(&mut processor, &input, rate, block, compiled);
                    let first_difference = expected.iter().zip(&actual).position(|(a, b)| a != b);
                    let max_error = expected
                        .iter()
                        .zip(&actual)
                        .map(|(a, b)| (a - b).abs())
                        .fold(0.0_f32, f32::max);
                    assert!(
                        first_difference.is_none(),
                        "rate={rate}, block={block}, compiled={compiled}, prior_calls={prior_calls}, first={first_difference:?}, max_error={max_error}"
                    );
                }
            }
        }
    }
}
