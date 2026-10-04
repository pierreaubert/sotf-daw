//! Real crossfeed audio must honor AutoGain smoothing at a fixed meter schedule.
// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_crossfeed::{CrossfeedMode, CrossfeedPlugin, CrossfeedPluginParams};

fn plugin(rate: u32, target: f32, smoothing: f32, enabled: bool) -> CrossfeedPlugin {
    let mut plugin = CrossfeedPlugin::new(CrossfeedPluginParams {
        mode: CrossfeedMode::Bauer,
        mix: 1.0,
        autogain_enabled: enabled,
        autogain_target_lufs: target,
        autogain_max_gain_db: 12.0,
        ..Default::default()
    })
    .unwrap();
    plugin.initialize(f64::from(rate)).unwrap();
    let id = ParameterId::from("autogain_smoothing_ms");
    plugin
        .parametric_set_parameter(id.clone(), ParameterValue::Float(smoothing))
        .unwrap();
    assert_eq!(
        plugin.parametric_get_parameter(&id),
        Some(ParameterValue::Float(smoothing))
    );
    plugin
}

fn render(plugin: &mut CrossfeedPlugin, input: &[f32], rate: u32, block: usize) -> Vec<f32> {
    let mut output = input.to_vec();
    for chunk in output.chunks_mut(block * 2) {
        let frames = chunk.len() / 2;
        assert_eq!(
            plugin
                .process_in_place(chunk, &ProcessContext::new(rate, frames))
                .unwrap(),
            frames
        );
    }
    output
}

fn energy(samples: &[f32]) -> f64 {
    samples.iter().map(|&x| f64::from(x).powi(2)).sum()
}

#[test]
fn realtime_smoothing_changes_crossfeed_gain_in_both_directions_and_survives_reset() {
    for rate in [48_000, 96_000] {
        let input: Vec<_> = (0..rate as usize * 2)
            .flat_map(|n| {
                let t = n as f64 / f64::from(rate);
                [
                    (0.025 * (std::f64::consts::TAU * 431.0 * t).sin()) as f32,
                    (0.01 * (std::f64::consts::TAU * 911.0 * t).sin()) as f32,
                ]
            })
            .collect();
        for block in [137, 512] {
            for target in [-40.0, -12.0] {
                let mut audio = Vec::new();
                for smoothing in [25.0, 1000.0] {
                    let mut processor = plugin(rate, target, smoothing, true);
                    let first = render(&mut processor, &input, rate, block);
                    processor.reset();
                    assert!(
                        first == render(&mut processor, &input, rate, block),
                        "reset changed audio"
                    );
                    audio.push(first);
                }
                let fast = energy(&audio[0]);
                let slow = energy(&audio[1]);
                let direction = if target == -12.0 { 1.0 } else { -1.0 };
                assert!(
                    direction * (fast / slow).ln() > 0.01,
                    "rate={rate}, block={block}, target={target}, fast={fast}, slow={slow}"
                );
                let raw = render(&mut plugin(rate, target, 25.0, false), &input, rate, block);
                assert_eq!(
                    raw,
                    render(
                        &mut plugin(rate, target, 1000.0, false),
                        &input,
                        rate,
                        block
                    )
                );
                assert!(direction * (fast / energy(&raw)).ln() > 0.1);
                assert!(
                    raw.iter().zip(&input).any(|(a, b)| (a - b).abs() > 1e-4),
                    "crossfeed must be nonneutral"
                );
            }
        }
    }
}
