//! Check HR enable transitions in sample time, including tiny high-rate callbacks.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterValue, Plugin, ProcessContext};
use sotf_plugin_upmixer::{UpmixerPlugin, UpmixerPluginParams};

fn render(rate: u32, fft_size: usize, pattern: &[usize]) -> (Vec<f32>, f32) {
    render_events(rate, fft_size, pattern, 16_384, &[])
}

fn render_events(
    rate: u32,
    fft_size: usize,
    pattern: &[usize],
    frames: usize,
    events: &[(usize, bool)],
) -> (Vec<f32>, f32) {
    let params: UpmixerPluginParams = serde_json::from_value(serde_json::json!({
        "fft_size": fft_size, "speaker_config": "2.0",
        "enable_hr_direct": false, "auto_gain_enabled": false,
        "bypass_decorrelation": true, "enable_subharmonic_synth": false,
        "hr_sharpen": 2.0
    }))
    .unwrap();
    let mut processor = UpmixerPlugin::from_params(params);
    processor.initialize(f64::from(rate)).unwrap();
    processor
        .set_parameter("enable_hr_direct".into(), ParameterValue::Bool(true))
        .unwrap();
    let input: Vec<f32> = (0..frames)
        .flat_map(|i| {
            let envelope = if i % 1024 < 64 { 0.1 } else { 0.002 };
            [
                envelope * (i as f32 * 0.37).sin(),
                envelope * (i as f32 * 0.61).cos(),
            ]
        })
        .collect();
    let mut output = vec![0.0; frames * 2];
    let mut position = 0;
    let mut event = 0;
    for &requested in pattern.iter().cycle() {
        if event < events.len() && position == events[event].0 {
            processor
                .set_parameter(
                    "enable_hr_direct".into(),
                    ParameterValue::Bool(events[event].1),
                )
                .unwrap();
            event += 1;
        }
        let boundary = events.get(event).map_or(frames, |event| event.0);
        let count = requested.min(boundary - position);
        if count == 0 {
            break;
        }
        processor
            .process(
                &input[position * 2..(position + count) * 2],
                &mut output[position * 2..(position + count) * 2],
                &ProcessContext::new(rate, count),
            )
            .unwrap();
        position += count;
    }
    (output, processor.diagnostics().hr_direct_envelope)
}

#[test]
fn hr_attack_progresses_with_single_frame_high_rate_callbacks() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        let (_, envelope) = render(rate, 512, &[1]);
        let expected = 1.0 - (-16_384.0 / (f64::from(rate) * 0.2025)).exp();
        assert!(
            (f64::from(envelope) - expected).abs() < 2e-4,
            "rate={rate}: HR attack stalled or has incorrect sample-time response: {envelope} vs {expected}"
        );
    }
}

#[test]
fn hr_transition_audio_is_invariant_to_callback_partitioning() {
    for rate in [48_000, 96_000] {
        for fft_size in [512, 2048, 4096] {
            let (reference, _) = render(rate, fft_size, &[1]);
            assert!(reference.iter().any(|x| x.abs() > 0.001));
            for pattern in [&[8192][..], &[7, 31, 257, 1024][..]] {
                let (actual, _) = render(rate, fft_size, pattern);
                let error = actual
                    .iter()
                    .zip(&reference)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0_f32, f32::max);
                assert!(
                    error < 2e-6,
                    "rate={rate}, fft={fft_size}, pattern={pattern:?}: waveform error={error}"
                );
            }
        }
    }
}

#[test]
fn hr_release_to_zero_and_reenable_preserve_the_source_timeline() {
    let events = [(2501, false), (200_003, true)];
    for fft_size in [512, 2048, 4096] {
        let (reference, reference_envelope) =
            render_events(48_000, fft_size, &[1], 220_000, &events);
        for pattern in [&[8192][..], &[7, 31, 257, 1024][..]] {
            let (actual, envelope) = render_events(48_000, fft_size, pattern, 220_000, &events);
            assert_eq!(envelope, reference_envelope);
            let error = actual
                .iter()
                .zip(&reference)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            let worst = actual
                .iter()
                .zip(&reference)
                .enumerate()
                .max_by(|(_, (a, b)), (_, (c, d))| (*a - *b).abs().total_cmp(&(*c - *d).abs()))
                .unwrap()
                .0
                / 2;
            assert!(
                error < 2e-6,
                "fft={fft_size}, pattern={pattern:?}: release/reenable error={error} at frame={worst}"
            );
        }
    }
}
