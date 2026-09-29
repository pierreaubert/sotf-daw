//! AutoGain measurement and compensation must follow the audio sample clock.
// Rust guideline compliant 2026-02-21
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_xtc::{XtcData, XtcPlugin, XtcPluginParams};

fn stepped_program(rate: u32) -> Vec<f32> {
    (0..rate as usize * 6)
        .flat_map(|n| {
            let t = n as f64 / f64::from(rate);
            let envelope = if t < 2.0 {
                0.01
            } else if t < 4.0 {
                0.05
            } else {
                0.02
            };
            [
                (envelope
                    * ((std::f64::consts::TAU * 431.0 * t).sin()
                        + 0.3 * (std::f64::consts::TAU * 3821.0 * t).sin())) as f32,
                (envelope
                    * (0.7 * (std::f64::consts::TAU * 911.0 * t).sin()
                        - 0.2 * (std::f64::consts::TAU * 3821.0 * t).cos())) as f32,
            ]
        })
        .collect()
}

fn render(
    input: &[f32],
    rate: u32,
    fft_size: usize,
    auto_gain: bool,
    neutral: bool,
    pattern: &[usize],
) -> (Vec<f32>, f64, f64) {
    let mut plugin = XtcPlugin::new(
        XtcPluginParams {
            fft_size,
            auto_gain_enabled: auto_gain,
            bypass_xtc_filters: neutral,
            ..Default::default()
        },
        rate,
    )
    .unwrap();
    plugin.initialize(rate).unwrap();
    let mut output = vec![0.0; input.len()];
    let mut cursor = 0;
    for &size in pattern.iter().cycle() {
        let frames = size.min(input.len() / 2 - cursor);
        if frames == 0 {
            break;
        }
        assert_eq!(
            plugin
                .process(
                    &input[cursor * 2..(cursor + frames) * 2],
                    &mut output[cursor * 2..(cursor + frames) * 2],
                    &ProcessContext::new(rate, frames),
                )
                .unwrap(),
            frames
        );
        cursor += frames;
    }
    let data = plugin.get_data().unwrap();
    let data = data.downcast_ref::<XtcData>().unwrap();
    (
        output,
        data.auto_gain.input_lufs,
        data.auto_gain.output_lufs,
    )
}

#[test]
fn autogain_preserves_neutral_delayed_program_through_level_steps() {
    for rate in [44100, 48000, 96000, 192000] {
        let source = stepped_program(rate);
        for n in [128, 2048, 16384] {
            for auto_gain in [false, true] {
                let (output, _, _) = render(&source, rate, n, auto_gain, true, &[512]);
                for (i, &actual) in output.iter().enumerate() {
                    let expected = i.checked_sub(n * 2).map_or(0.0, |j| source[j]);
                    assert!(
                        (actual - expected).abs() < 1.5e-6,
                        "AG={auto_gain}, sample={i}, actual={actual}, expected={expected}"
                    );
                }
            }
        }
    }
}

#[test]
fn autogain_waveform_and_measurements_ignore_callback_partition() {
    for rate in [44100, 48000, 96000, 192000] {
        let source = stepped_program(rate);
        for n in [128, 2048, 16384] {
            for auto_gain in [false, true] {
                let (reference, input_lufs, output_lufs) =
                    render(&source, rate, n, auto_gain, false, &[1]);
                for pattern in [&[512][..], &[8193, 137][..]] {
                    let (actual, actual_input, actual_output) =
                        render(&source, rate, n, auto_gain, false, pattern);
                    let error = reference
                        .iter()
                        .zip(&actual)
                        .map(|(a, b)| (a - b).abs())
                        .fold(0.0_f32, f32::max);
                    assert!(
                        error < 1.5e-6,
                        "AG={auto_gain}, pattern={pattern:?}, peak error={error}"
                    );
                    if auto_gain {
                        assert!(actual_input.is_finite() && actual_output.is_finite());
                        assert!((actual_input - input_lufs).abs() < 1e-9);
                        assert!((actual_output - output_lufs).abs() < 1e-9);
                    }
                }
            }
        }
    }
}
