//! Compare full mix-automation waveforms at identical absolute sample positions.
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_crossfeed::{CrossfeedMode, CrossfeedPlugin, CrossfeedPluginParams};

fn render(mode: CrossfeedMode, partitions: &[usize], rate: u32) -> Vec<f32> {
    let mut plugin = CrossfeedPlugin::new(CrossfeedPluginParams {
        mode,
        mix: 0.35,
        autogain_enabled: false,
        ..Default::default()
    })
    .unwrap();
    plugin.initialize(f64::from(rate)).unwrap();
    let frames = 20000;
    let mut output: Vec<f32> = (0..frames)
        .flat_map(|frame| {
            let t = frame as f64 / f64::from(rate);
            [
                (0.3 * (std::f64::consts::TAU * 317.0 * t).sin()) as f32,
                (0.2 * (std::f64::consts::TAU * 1337.0 * t).cos()) as f32,
            ]
        })
        .collect();
    let events = [(0, 0.0), (2113, 0.9), (9001, 0.2), (16000, 1.0)];
    let mut position = 0;
    let mut event = 0;
    let mut block = 0;
    while position < frames {
        if event < events.len() && position == events[event].0 {
            plugin
                .set_parameter(
                    ParameterId::from("mix"),
                    ParameterValue::Float(events[event].1),
                )
                .unwrap();
            event += 1;
        }
        let boundary = if event < events.len() {
            events[event].0
        } else {
            frames
        };
        let count = partitions[block % partitions.len()].min(boundary - position);
        plugin
            .process_in_place(
                &mut output[position * 2..(position + count) * 2],
                &ProcessContext::new(rate, count),
            )
            .unwrap();
        position += count;
        block += 1;
    }
    output
}

#[test]
fn mix_automation_waveform_is_invariant_to_callback_partitioning() {
    for mode in [
        CrossfeedMode::Bauer,
        CrossfeedMode::Meier,
        CrossfeedMode::Mb,
        CrossfeedMode::Hrtf,
    ] {
        for rate in [44_100, 48_000, 96_000, 192_000] {
            let reference = render(mode, &[1], rate);
            assert!(reference.iter().any(|sample| sample.abs() > 0.1));
            for pattern in [&[1, 7, 31, 257, 1024][..], &[8192][..]] {
                let actual = render(mode, pattern, rate);
                let error = actual
                    .iter()
                    .zip(&reference)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0_f32, f32::max);
                assert!(
                    error < 3e-6,
                    "mode={mode:?}, rate={rate}, pattern={pattern:?}: waveform error={error}"
                );
            }
        }
    }
}
