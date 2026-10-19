//! Gain endpoint, first callback, zero-frame, and adapter arithmetic policies.

// Rust guideline compliant 2026-02-21
use sotf_host::parametric_plugin::{ParametricPlugin, ParametricPluginAdapter};
use sotf_host::plugin::{Plugin, PluginCompiledOp, ProcessContext};
use sotf_plugin_gain::GainPlugin;

#[test]
fn high_rate_gain_rises_reach_the_exact_requested_endpoint() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for channels in [1, 7] {
            for per_channel in [false, true] {
                let mut plugin = if per_channel {
                    GainPlugin::new_per_channel(vec![0.0; channels]).unwrap()
                } else {
                    GainPlugin::new(channels, 0.0)
                };
                plugin.plugin_initialize(f64::from(rate)).unwrap();
                if per_channel {
                    for channel in 0..channels {
                        plugin.set_channel_gain_db(channel, 20.0).unwrap();
                    }
                } else {
                    plugin.set_gain_db(20.0);
                }
                let frames = rate as usize / 5;
                let input = vec![1.0; frames * channels];
                let mut output = vec![0.0; input.len()];
                ParametricPlugin::process(
                    &mut plugin,
                    &input,
                    &mut output,
                    &ProcessContext::new(rate, frames),
                )
                .unwrap();
                let expected = 10.0 - 9.0 * (-1.0 / (0.010 * f64::from(rate))).exp();
                assert!(
                    (f64::from(output[0]) - expected).abs() < 1e-6,
                    "the first callback must advance one frame, not a whole block"
                );
                assert_eq!(
                    *output.last().unwrap(),
                    10.0,
                    "rate={rate}, channels={channels}, per_channel={per_channel}: f32 ramp stalled"
                );
            }
        }
    }
}

#[test]
fn zero_frame_callbacks_do_not_snap_a_nearly_settled_control() {
    let mut tested = GainPlugin::new(1, 0.0);
    let mut reference = GainPlugin::new(1, 0.0);
    for plugin in [&mut tested, &mut reference] {
        plugin.plugin_initialize(48_000.0).unwrap();
        plugin.set_gain_linear(0.999995);
    }
    ParametricPlugin::process(&mut tested, &[], &mut [], &ProcessContext::new(48_000, 0)).unwrap();
    tested.set_gain_linear(0.5);
    reference.set_gain_linear(0.5);
    let mut actual = [0.0];
    let mut expected = [0.0];
    ParametricPlugin::process(
        &mut tested,
        &[1.0],
        &mut actual,
        &ProcessContext::new(48_000, 1),
    )
    .unwrap();
    ParametricPlugin::process(
        &mut reference,
        &[1.0],
        &mut expected,
        &ProcessContext::new(48_000, 1),
    )
    .unwrap();
    assert_eq!(actual, expected, "no audio frames means no smoother steps");
}

fn assert_policy(output: &[f32]) {
    assert_eq!(&output[..3], &[20.0, -20.0, 1.0]);
    assert!(output[3].is_nan());
    assert_eq!(output[4], f32::INFINITY);
    assert_eq!(output[5], f32::NEG_INFINITY);
}

#[test]
fn raw_adapter_compiled_and_f64_fallback_share_unclipped_ieee_multiplication() {
    let input = [2.0, -2.0, 0.1, f32::NAN, f32::INFINITY, f32::NEG_INFINITY];
    let context = ProcessContext::new(48_000, input.len());
    let mut raw = GainPlugin::new(1, 20.0);
    raw.plugin_initialize(48_000.0).unwrap();
    let mut output = [0.0; 6];
    ParametricPlugin::process(&mut raw, &input, &mut output, &context).unwrap();
    assert_policy(&output);
    let mut adapter = ParametricPluginAdapter::new(GainPlugin::new(1, 20.0));
    adapter.initialize(48_000.0).unwrap();
    adapter.process(&input, &mut output, &context).unwrap();
    assert_policy(&output);
    adapter
        .process_compiled_f32(PluginCompiledOp::ApplyGain, &input, &mut output, &context)
        .unwrap()
        .unwrap();
    assert_policy(&output);
    let input64 = input.map(f64::from);
    let mut output64 = [0.0; 6];
    adapter
        .process_f64(&input64, &mut output64, &context)
        .unwrap();
    assert_policy(&output64.map(|x| x as f32));
}
