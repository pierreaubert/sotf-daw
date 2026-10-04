//! AutoGain settings must affect real EQ audio in both public processing routes.
// Rust guideline compliant 2026-02-21
use sotf_host::plugin::PluginCompiledOp;
use sotf_host::{ParametricPlugin, ProcessContext};
use sotf_plugin_eq::{EqPlugin, EqPluginParams};

fn plugin(rate: u32, gain_db: f64, smoothing_ms: f32, enabled: bool) -> EqPlugin {
    let params: EqPluginParams = serde_json::from_value(serde_json::json!({
        "filters": [{"filter_type": "peak", "freq": 1000.0, "q": 1.0, "db_gain": gain_db}],
        "auto_gain": {"enabled": enabled, "smoothing_ms": smoothing_ms, "max_gain_db": 12.0}
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
                .expect("fixture must actually dispatch the compiled biquad operation")
                .unwrap()
        } else {
            plugin.process(source, destination, &context).unwrap()
        };
        assert_eq!(frames, context.num_frames);
    }
    output
}

fn energy(samples: &[f32]) -> f64 {
    samples.iter().map(|&x| f64::from(x).powi(2)).sum()
}

#[test]
fn json_smoothing_changes_eq_audio_and_compiled_route_matches_ordinary() {
    // The meter observes every tenth callback. Compare only identical schedules;
    // six seconds also supplies more than 400 ms of measured input at either size.
    for rate in [48_000, 96_000] {
        let input: Vec<_> = (0..rate as usize * 6)
            .flat_map(|n| {
                let x = (0.02 * (std::f64::consts::TAU * 1000.0 * n as f64 / f64::from(rate)).sin())
                    as f32;
                [x, -0.5 * x]
            })
            .collect();
        for block in [137, 512] {
            for gain_db in [-9.0, 9.0] {
                let mut audio = Vec::new();
                for smoothing in [25.0, 1000.0] {
                    let mut ordinary = plugin(rate, gain_db, smoothing, true);
                    let mut compiled = plugin(rate, gain_db, smoothing, true);
                    let first = render(&mut ordinary, &input, rate, block, false);
                    assert!(
                        first == render(&mut compiled, &input, rate, block, true),
                        "compiled waveform differs"
                    );
                    audio.push(first);
                }
                let fast = energy(&audio[0]);
                let slow = energy(&audio[1]);
                // A center-frequency boost needs attenuation; a cut needs gain.
                // Shorter smoothing must move farther toward either correction.
                let signed_change = (slow / fast).ln() * gain_db.signum();
                assert!(
                    signed_change > 0.001,
                    "rate={rate}, block={block}, EQ={gain_db}, fast={fast}, slow={slow}"
                );
                let dry_fast = render(
                    &mut plugin(rate, gain_db, 25.0, false),
                    &input,
                    rate,
                    block,
                    false,
                );
                let dry_slow = render(
                    &mut plugin(rate, gain_db, 1000.0, false),
                    &input,
                    rate,
                    block,
                    true,
                );
                assert_eq!(
                    dry_fast, dry_slow,
                    "disabled AutoGain must ignore smoothing"
                );
                assert!((energy(&audio[0]) / energy(&dry_fast)).ln().abs() > 0.1);
            }
        }
    }
}
