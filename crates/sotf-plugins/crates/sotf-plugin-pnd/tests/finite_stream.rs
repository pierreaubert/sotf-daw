//! Finite PND output follows the analytical neutral delay and exact support.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, Plugin, ProcessContext, TailLength};
use sotf_plugin_pnd::{PndPlugin, PndPluginParams};

const RATE: u32 = 48_000;
const N: usize = 2048;
const H: usize = 512;
const D: usize = 2047;

fn neutral(channels: usize, rate: u32) -> PndPlugin {
    let mut plugin = PndPlugin::from_params(
        channels,
        PndPluginParams {
            correction_strength: 0.0,
            ..Default::default()
        },
    )
    .unwrap();
    plugin.initialize(rate).unwrap();
    plugin
}

fn drain(plugin: &mut PndPlugin, rate: u32, capacities: &[usize]) -> Vec<f32> {
    let channels = plugin.output_channels();
    let mut result = Vec::new();
    for capacity in capacities.iter().cycle().take(5000) {
        let mut output = vec![123.0; capacity * channels];
        let drained = plugin
            .drain(&mut output, &ProcessContext::new(rate, 0))
            .unwrap();
        assert!(drained.frames <= *capacity);
        assert!(
            output[drained.frames * channels..]
                .iter()
                .all(|v| *v == 123.0)
        );
        result.extend_from_slice(&output[..drained.frames * channels]);
        if drained.complete {
            return result;
        }
        assert!(drained.frames > 0);
    }
    panic!("finite PND drain did not complete");
}

#[test]
fn final_marker_is_retained_after_eof() {
    let mut plugin = neutral(2, RATE);
    let mut input = vec![0.0; 34];
    input[32] = 0.75;
    input[33] = -0.25;
    let mut output = vec![0.0; input.len()];
    plugin
        .process(&input, &mut output, &ProcessContext::new(RATE, 17))
        .unwrap();
    output.extend(drain(&mut plugin, RATE, &[7, 512]));
    assert_eq!(output.len(), (D + N) * 2);
    assert!((output[(D + 16) * 2] - 0.75).abs() < 2e-5);
    assert!((output[(D + 16) * 2 + 1] + 0.25).abs() < 2e-5);
}

fn process(plugin: &mut PndPlugin, input: &[f32], rate: u32, blocks: &[usize]) -> Vec<f32> {
    let channels = plugin.input_channels();
    let mut output = vec![123.0; input.len()];
    let mut offset = 0;
    for size in blocks.iter().cycle() {
        let frames = (*size).min(input.len() / channels - offset);
        if frames == 0 {
            break;
        }
        let range = offset * channels..(offset + frames) * channels;
        assert_eq!(
            plugin
                .process(
                    &input[range.clone()],
                    &mut output[range],
                    &ProcessContext::new(rate, frames)
                )
                .unwrap(),
            frames
        );
        offset += frames;
    }
    output
}

fn check_neutral(
    frames: usize,
    channels: usize,
    rate: u32,
    blocks: &[usize],
    capacities: &[usize],
) -> (f64, f64) {
    let input: Vec<f32> = (0..frames)
        .flat_map(|frame| {
            (0..channels).map(move |ch| {
                if frame == 0 || frame == frames - 1 {
                    0.6 / (ch + 1) as f32
                } else {
                    (frame as f32 * (0.013 + 0.021 * ch as f32)).sin() * 0.1
                }
            })
        })
        .collect();
    let mut plugin = neutral(channels, rate);
    assert_eq!(plugin.latency_samples(), D);
    assert_eq!(plugin.tail_length(), TailLength::Finite(4094));
    let mut output = process(&mut plugin, &input, rate, blocks);
    output.extend(drain(&mut plugin, rate, capacities));
    let endpoint = D + ((frames - 1) / H) * H + N;
    assert_eq!(output.len(), endpoint * channels);
    // At zero correction strength ordinary zero-padding has the same fixed
    // unity ratio, independent of adaptive analysis. EOS must add no error.
    let mut ordinary = neutral(channels, rate);
    let mut padded = input.clone();
    padded.resize(endpoint * channels, 0.0);
    assert_eq!(output, process(&mut ordinary, &padded, rate, blocks));
    let mut signal_energy = 0.0_f64;
    let mut error_energy = 0.0_f64;
    let mut max_error = 0.0_f64;
    for (index, sample) in output.into_iter().enumerate() {
        let expected = index
            .checked_sub(D * channels)
            .and_then(|source| input.get(source))
            .copied()
            .unwrap_or(0.0);
        assert!(sample.is_finite());
        signal_energy += f64::from(expected).powi(2);
        error_energy += (f64::from(sample) - f64::from(expected)).powi(2);
        max_error = max_error.max((f64::from(sample) - f64::from(expected)).abs());
    }
    // Preserve the existing unity phase-vocoder contract: finite-precision
    // instantaneous-frequency/phase locking is not an exact identity filter.
    let snr = 10.0 * (signal_energy / error_energy.max(f64::MIN_POSITIVE)).log10();
    assert!(
        snr > 35.0,
        "frames={frames} channels={channels} rate={rate}: {snr} dB"
    );
    (max_error, snr)
}

#[test]
fn all_hop_phases_preserve_first_and_final_samples() {
    let (mut max_error, mut min_snr) = (0.0_f64, f64::INFINITY);
    for frames in 1..=H {
        let (error, snr) = check_neutral(frames, 1, RATE, &[1, 17, 257], &[1, 7, H, 8192]);
        max_error = max_error.max(error);
        min_snr = min_snr.min(snr);
    }
    println!("512 hop phases: max_error={max_error:e}, min_snr={min_snr:.6} dB");
    // Measured unchanged ordinary-padding baseline: 1.79e-7 / 132.8 dB.
    assert!(max_error < 5e-7 && min_snr > 125.0);
}

#[test]
fn fft_boundaries_rates_channels_and_capacities_match_neutral_oracle() {
    let (mut max_error, mut min_snr) = (0.0_f64, f64::INFINITY);
    for frames in [1, H - 1, H, H + 1, N - 1, N, N + 1] {
        for channels in [1, 2, 6] {
            for rate in [44_100, 48_000, 96_000] {
                for capacity in [1, 7, H, 8192] {
                    let (error, snr) = check_neutral(frames, channels, rate, &[4096], &[capacity]);
                    max_error = max_error.max(error);
                    min_snr = min_snr.min(snr);
                }
            }
        }
    }
    println!("252 boundary cases: max_error={max_error:e}, min_snr={min_snr:.6} dB");
    // The existing phase-locking baseline measures 2.49e-4 / 62.7 dB.
    // Keep modest f32 headroom as well as exact ordinary-padding parity.
    assert!(max_error < 5e-4 && min_snr > 60.0);
}

#[test]
fn rejected_calls_are_transactional_and_reset_rearms_eof() {
    assert!(PndPlugin::new(0).initialize(RATE).is_err());
    let mut raw = PndPlugin::new(2);
    assert_eq!(raw.tail_length(), TailLength::Unknown);
    assert!(
        raw.drain(&mut [123.0; 2], &ProcessContext::new(RATE, 0))
            .is_err()
    );
    let mut plugin = neutral(2, RATE);
    assert!(
        plugin
            .drain(&mut [], &ProcessContext::new(RATE, 0))
            .unwrap()
            .complete
    );
    let input = vec![0.3; 1030];
    let mut twin = neutral(2, RATE);
    assert_eq!(
        process(&mut plugin, &input, RATE, &[17]),
        process(&mut twin, &input, RATE, &[17])
    );
    for after_eof in [false, true] {
        if after_eof {
            let mut actual = [0.0; 14];
            let mut expected = [0.0; 14];
            plugin
                .drain(&mut actual, &ProcessContext::new(RATE, 0))
                .unwrap();
            twin.drain(&mut expected, &ProcessContext::new(RATE, 0))
                .unwrap();
            assert_eq!(actual, expected);
        }
        let mut canary = [123.0; 15];
        assert!(
            plugin
                .drain(&mut canary, &ProcessContext::new(RATE, 0))
                .is_err()
        );
        assert!(
            plugin
                .drain(&mut canary[..14], &ProcessContext::new(1, 0))
                .is_err()
        );
        assert!(
            plugin
                .drain(&mut [], &ProcessContext::new(RATE, 0))
                .is_err()
        );
        assert_eq!(canary, [123.0; 15]);
        assert!(plugin.initialize(0).is_err());
        assert_eq!(
            plugin
                .process(&[], &mut [], &ProcessContext::new(RATE, 0))
                .unwrap(),
            0
        );
    }
    let snapshot = plugin.parameters();
    for parameter in &snapshot {
        plugin
            .set_parameter(
                parameter.id.clone(),
                plugin.get_parameter(&parameter.id).unwrap(),
            )
            .unwrap();
    }
    for (id, value) in [
        ("correction_strength", ParameterValue::Float(1.0)),
        ("reference_frequency_hz", ParameterValue::Float(440.0)),
        ("missing", ParameterValue::Float(0.0)),
        ("correction_strength", ParameterValue::Bool(false)),
    ] {
        assert!(plugin.set_parameter(ParameterId::from(id), value).is_err());
    }
    let mut untouched = [123.0; 2];
    assert!(
        plugin
            .process(&[0.0; 2], &mut untouched, &ProcessContext::new(RATE, 1))
            .is_err()
    );
    assert_eq!(untouched, [123.0; 2]);
    assert_eq!(
        drain(&mut plugin, RATE, &[1, 7, H]),
        drain(&mut twin, RATE, &[H])
    );
    assert!(
        plugin
            .drain(&mut [], &ProcessContext::new(RATE, 0))
            .unwrap()
            .complete
    );
    for reinitialize in [false, true] {
        if reinitialize {
            plugin.initialize(RATE).unwrap();
        } else {
            plugin.reset();
        }
        let mut fresh = neutral(2, RATE);
        assert_eq!(
            process(&mut plugin, &input, RATE, &[257]),
            process(&mut fresh, &input, RATE, &[257])
        );
        assert_eq!(
            drain(&mut plugin, RATE, &[H]),
            drain(&mut fresh, RATE, &[H])
        );
    }
}

#[test]
fn full_capacity_call_bound_tracks_remaining_work_and_completion() {
    let mut plugin = neutral(1, RATE);
    assert_eq!(plugin.drain_call_bound().unwrap().get(), 1);
    for frames in [1, H - 1, H, H + 1] {
        plugin.reset();
        process(&mut plugin, &vec![0.1; frames], RATE, &[17]);
        let promised = plugin.drain_call_bound().unwrap().get();
        let mut calls = 0;
        loop {
            let before = plugin.drain_call_bound().unwrap().get();
            let result = plugin
                .drain(&mut [0.0; H], &ProcessContext::new(RATE, 0))
                .unwrap();
            calls += 1;
            assert_eq!(before, (promised - calls + 1).max(1));
            if result.complete {
                break;
            }
        }
        assert_eq!(calls, promised);
        assert_eq!(plugin.drain_call_bound().unwrap().get(), 1);
    }
}
