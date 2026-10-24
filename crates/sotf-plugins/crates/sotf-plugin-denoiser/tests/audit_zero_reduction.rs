//! Zero requested attenuation must retain the complete latency-aligned signal.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_denoiser::{DenoiserData, DenoiserPlugin, DenoiserPluginParams};

#[cfg(feature = "qa")]
#[global_allocator]
static ALLOCATOR: sotf_host::CountingAlloc = sotf_host::CountingAlloc;

fn processor(rate: u32, channels: usize, low: bool, optional: bool) -> DenoiserPlugin {
    let mut plugin = DenoiserPlugin::from_params(
        channels,
        DenoiserPluginParams {
            reduction_db: 0.0,
            transparency: 0.0,
            low_latency: low,
            multi_resolution: optional,
            polyphonic_detection: optional,
            psychoacoustic_masking: optional,
            formant_preservation: optional,
            spectral_sub_enabled: optional,
            harmonic_percussive: optional,
            spatial_denoise: optional,
            curve_low: if optional { 0.4 } else { 1.0 },
            curve_mid: if optional { 0.8 } else { 1.0 },
            curve_high: if optional { 0.6 } else { 1.0 },
            ..Default::default()
        },
    );
    plugin.initialize(f64::from(rate)).unwrap();
    plugin
}

fn source(frames: usize, channels: usize) -> Vec<f32> {
    let mut input: Vec<_> = (0..frames * channels)
        .map(|i| {
            let frame = (i / channels) as f64;
            let channel = (i % channels) as f64;
            (0.17 * (0.719 * frame + 0.2 * channel).cos() + 0.03 * (0.123 * frame + channel).sin())
                as f32
        })
        .collect();
    input[0] = 0.125;
    *input.last_mut().unwrap() = -0.25;
    input
}

fn render(
    plugin: &mut DenoiserPlugin,
    input: &[f32],
    channels: usize,
    rate: u32,
    parts: &[usize],
) -> Vec<f32> {
    let mut output = input.to_vec();
    let mut position = 0;
    let mut call = 0;
    while position < input.len() / channels {
        let frames = parts[call % parts.len()].min(input.len() / channels - position);
        plugin
            .process_in_place(
                &mut output[position * channels..(position + frames) * channels],
                &ProcessContext::new(rate, frames),
            )
            .unwrap();
        position += frames;
        call += 1;
    }
    let mut tail = vec![1234.0; plugin.drain_output_frames_max() * channels];
    for _ in 0..32 {
        let status = plugin
            .drain(&mut tail, &ProcessContext::new(rate, 0))
            .unwrap();
        output.extend_from_slice(&tail[..status.frames * channels]);
        if status.complete {
            return output;
        }
    }
    panic!("zero-reduction drain did not finish");
}

fn error(output: &[f32], input: &[f32], channels: usize, latency: usize) -> f64 {
    output
        .iter()
        .enumerate()
        .map(|(i, &actual)| {
            let expected = i
                .checked_sub(latency * channels)
                .and_then(|j| input.get(j))
                .copied()
                .unwrap_or(0.0);
            (f64::from(actual) - f64::from(expected)).abs()
        })
        .fold(0.0, f64::max)
}

#[test]
fn zero_reduction_is_an_independent_unity_transfer_in_every_analysis_mode() {
    let mut cases = 0;
    let mut failures = 0;
    let mut maximum_error = 0.0_f64;
    for rate in [44_100_u32, 48_000, 96_000, 192_000] {
        for low in [false, true] {
            for channels in [1, 2, 6] {
                for optional in [false, true] {
                    let mut plugin = processor(rate, channels, low, optional);
                    let latency = plugin.latency_samples();
                    let frames =
                        (8 * latency + 73).max((f64::from(rate) * 0.35).ceil() as usize + 73);
                    let input = source(frames, channels);
                    let output = render(&mut plugin, &input, channels, rate, &[257, 4096, 13]);
                    let expected_support =
                        2 * latency + ((frames - 1) / (latency / 2)) * (latency / 2);
                    assert_eq!(output.len(), expected_support * channels);
                    let observed = error(&output, &input, channels, latency);
                    maximum_error = maximum_error.max(observed);
                    failures += usize::from(observed > 2e-6);
                    plugin.reset();
                    assert_eq!(
                        render(&mut plugin, &input, channels, rate, &[1, 31, 257]),
                        output
                    );
                    println!(
                        "zero_reduction_unity rate={rate} low={low} channels={channels} optional={optional} maximum_error={observed:e}"
                    );
                    cases += 1;
                }
            }
        }
    }
    println!(
        "zero_reduction_summary cases={cases} failures={failures} maximum_error={maximum_error:e}"
    );
    assert_eq!(cases, 48);
    assert_eq!(
        failures, 0,
        "zero attenuation must retain the full signal at the reported delay"
    );
}

#[test]
fn a_published_captured_profile_cannot_override_zero_attenuation() {
    let mut cases = 0;
    for low in [false, true] {
        let rate = 48_000;
        let channels = 2;
        let mut plugin = processor(rate, channels, low, true);
        plugin
            .parametric_set_parameter(ParameterId::from("learn_noise"), ParameterValue::Bool(true))
            .unwrap();
        let mut noise: Vec<f32> = (0..60_000 * channels)
            .map(|i| ((i * 73 % 257) as f32 - 128.0) / 1024.0)
            .collect();
        for block in noise.chunks_mut(257 * channels) {
            plugin
                .process_in_place(block, &ProcessContext::new(rate, block.len() / channels))
                .unwrap();
        }
        let data = plugin
            .get_data()
            .unwrap()
            .downcast::<DenoiserData>()
            .unwrap();
        assert!(data.has_captured_profile);
        plugin
            .parametric_set_parameter(
                ParameterId::from("use_captured_profile"),
                ParameterValue::Bool(true),
            )
            .unwrap();
        plugin.reset();
        let latency = plugin.latency_samples();
        let input = source(8 * latency + 73, channels);
        let output = render(&mut plugin, &input, channels, rate, &[1, 257, 4096]);
        let observed = error(&output, &input, channels, latency);
        println!("zero_reduction_profile low={low} maximum_error={observed:e}");
        assert!(
            observed < 2e-6,
            "a selected noise profile may not turn zero dB into attenuation"
        );
        cases += 1;
    }
    assert_eq!(cases, 2);
}

#[cfg(feature = "qa")]
#[test]
fn zero_depth_cold_callbacks_live_transitions_and_reset_have_no_heap_work() {
    for low in [false, true] {
        for optional in [false, true] {
            let mut plugin = processor(48_000, 2, low, optional);
            let frames = 4096;
            let mut block = source(frames, 2);
            let context = ProcessContext::new(48_000, frames);
            let reduction = ParameterId::from("reduction_db");
            sotf_host::test_utils::assert_no_allocs_or_deallocs(
                "zero attenuation cold process/reset/transition",
                || {
                    plugin.process_in_place(&mut block, &context).unwrap();
                    plugin
                        .parametric_set_parameter(reduction.clone(), ParameterValue::Float(10.0))
                        .unwrap();
                    plugin.process_in_place(&mut block, &context).unwrap();
                    plugin
                        .parametric_set_parameter(reduction.clone(), ParameterValue::Float(0.0))
                        .unwrap();
                    plugin.process_in_place(&mut block, &context).unwrap();
                    plugin.reset();
                    plugin.process_in_place(&mut block, &context).unwrap();
                },
            );
        }
    }
}
