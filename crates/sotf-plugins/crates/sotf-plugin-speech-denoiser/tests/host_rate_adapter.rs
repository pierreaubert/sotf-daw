//! Host-rate speech adapter: exact bypass, framing, and drain contracts.

use sotf_host::{CountingAlloc, ParametricInPlacePlugin, ProcessContext, assert_no_allocs};
use sotf_plugin_speech_denoiser::{
    SpeechDenoiserData, SpeechDenoiserPlugin, SpeechDenoiserPluginParams,
};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

const VALIDATOR_RATES: [f64; 13] = [
    8_000.0, 22_050.0, 44_100.0, 48_000.0, 88_200.0, 96_000.0, 192_000.0,
    384_000.0, 768_000.0, 1_234.5678, 12_345.678, 45_678.901, 123_456.78,
];

fn strength_zero(rate: f64, channels: usize, enabled: bool) -> SpeechDenoiserPlugin {
    let mut plugin = SpeechDenoiserPlugin::from_params(
        channels,
        SpeechDenoiserPluginParams {
            enabled,
            strength: 0.0,
            ..SpeechDenoiserPluginParams::default()
        },
    );
    plugin.initialize(rate).unwrap();
    plugin
}

fn disabled(rate: f64, channels: usize) -> SpeechDenoiserPlugin {
    strength_zero(rate, channels, false)
}

fn model_frames(plugin: &SpeechDenoiserPlugin) -> u64 {
    plugin
        .get_data()
        .unwrap()
        .downcast::<SpeechDenoiserData>()
        .unwrap()
        .model_frames
}

fn process_stream(plugin: &mut SpeechDenoiserPlugin, rate: f64, source: &[f32]) -> Vec<f32> {
    let mut output = Vec::with_capacity(source.len());
    for chunk in source.chunks(137) {
        let mut block = chunk.to_vec();
        assert_eq!(
            plugin.process_in_place(&mut block, &ProcessContext::new(rate, chunk.len())).unwrap(),
            chunk.len()
        );
        output.extend_from_slice(&block);
    }
    output
}

fn render(
    plugin: &mut SpeechDenoiserPlugin,
    rate: f64,
    source: &[f32],
    channels: usize,
    partitions: &[usize],
    min_model_frames_before_drain: Option<u64>,
) -> Vec<f32> {
    let mut output = Vec::new();
    let mut offset = 0;
    let mut call = 0;
    while offset < source.len() / channels {
        let frames = partitions[call % partitions.len()].min(source.len() / channels - offset);
        let mut block = source[offset * channels..(offset + frames) * channels].to_vec();
        assert_eq!(
            plugin.process_in_place(&mut block, &ProcessContext::new(rate, frames)).unwrap(),
            frames,
        );
        output.extend_from_slice(&block);
        offset += frames;
        call += 1;
    }
    if let Some(min_frames) = min_model_frames_before_drain {
        assert!(
            model_frames(plugin) >= min_frames,
            "{rate} Hz processed fewer than {min_frames} RNNoise frames before drain"
        );
    }
    // A broken drain must fail this test instead of hanging the remote QA job.
    for call in 0..100_000 {
        let mut tail = vec![0.0; 64 * channels];
        let result = plugin.drain(&mut tail, &ProcessContext::new(rate, 0)).unwrap();
        assert!(result.frames <= 64, "drain over-reported output on call {call}");
        output.extend_from_slice(&tail[..result.frames * channels]);
        if result.complete {
            return output;
        }
    }
    panic!("speech adapter drain did not complete within 100,000 calls");
}

#[test]
fn all_validator_rates_preserve_full_band_at_zero_strength_and_exact_host_latency() {
    for rate in VALIDATOR_RATES {
        for channels in [1, 2] {
            for enabled in [false, true] {
                let mut plugin = strength_zero(rate, channels, enabled);
                let latency = plugin.latency_samples();
                let frames = latency + 257;
                let mut source = vec![0.0; frames * channels];
                for frame in 0..frames {
                    for channel in 0..channels {
                        // An alternating high-band component must survive the
                        // strength-zero path even above RNNoise's 24 kHz Nyquist.
                        source[frame * channels + channel] =
                            if frame % 2 == 0 { 0.5 } else { -0.5 };
                    }
                }
                source[17 * channels] = 1.0;
                let output = render(&mut plugin, rate, &source, channels, &[1, 17, 63, 137], None);
                if enabled && rate != 48_000.0 {
                    assert!(output.len() > (frames + latency) * channels);
                } else {
                    assert_eq!(output.len(), (frames + latency) * channels);
                }
                assert!(output[..latency * channels].iter().all(|&x| x == 0.0));
                assert_eq!(
                    &output[latency * channels..(latency + frames) * channels],
                    source.as_slice()
                );
                assert!(
                    output[(latency + frames) * channels..].iter().all(|&sample| sample == 0.0),
                    "{rate} Hz zero-strength tail contains audio"
                );
            }
        }
    }
}

#[test]
fn adapted_output_is_independent_of_host_callback_partition() {
    for rate in VALIDATOR_RATES {
        // Three model-frame durations exercise wet processing even at 768 kHz.
        let frames = 4096.max((rate * 0.03).ceil() as usize);
        let source: Vec<f32> = (0..frames)
            .map(|frame| ((frame as f64 * 440.0 * std::f64::consts::TAU / rate).sin() * 0.5) as f32)
            .collect();
        let mut small = disabled(rate, 1);
        let mut large = disabled(rate, 1);
        let a = render(&mut small, rate, &source, 1, &[1, 7, 65], None);
        let b = render(&mut large, rate, &source, 1, &[511], None);
        assert_eq!(a, b, "{rate} Hz disabled dry path changed with callback partition");
        let mut wet_small = SpeechDenoiserPlugin::new(1);
        wet_small.initialize(rate).unwrap();
        let mut wet_large = SpeechDenoiserPlugin::new(1);
        wet_large.initialize(rate).unwrap();
        let a = render(&mut wet_small, rate, &source, 1, &[1, 7, 65], Some(2));
        let b = render(&mut wet_large, rate, &source, 1, &[511], Some(2));
        assert_eq!(a, b, "{rate} Hz enabled wet path changed with callback partition");
    }
}

#[test]
fn adapted_process_has_no_heap_allocation_after_preparation() {
    for rate in VALIDATOR_RATES {
        let mut plugin = disabled(rate, 2);
        let mut block = [0.25_f32; 128];
        let dry_label = format!("{rate} Hz speech adapter disabled process");
        assert_no_allocs(&dry_label, || {
            plugin.process_in_place(&mut block, &ProcessContext::new(rate, 64)).unwrap();
        });
        let mut enabled = SpeechDenoiserPlugin::new(2);
        enabled.initialize(rate).unwrap();
        let mut first = [0.25_f32; 128];
        let first_label = format!("{rate} Hz speech adapter first enabled process");
        assert_no_allocs(&first_label, || {
            enabled
                .process_in_place(&mut first, &ProcessContext::new(rate, 64))
                .unwrap();
        });
        let subsequent_label = format!("{rate} Hz speech adapter subsequent enabled process");
        assert_no_allocs(&subsequent_label, || {
            let callbacks = ((rate * 0.03).ceil() as usize).div_ceil(64).max(1);
            for _ in 0..callbacks {
                enabled
                    .process_in_place(&mut block, &ProcessContext::new(rate, 64))
                    .unwrap();
            }
        });
        assert!(model_frames(&enabled) >= 2, "{rate} Hz missed RNNoise processing");
    }
}

#[test]
fn failed_rate_reinitialization_retains_active_backend_and_adapter() {
    for rate in VALIDATOR_RATES {
        for enabled in [false, true] {
            let mut plugin = if enabled {
                let mut plugin = SpeechDenoiserPlugin::new(1);
                plugin.initialize(rate).unwrap();
                plugin
            } else {
                disabled(rate, 1)
            };
            let mut reference = if enabled {
                let mut reference = SpeechDenoiserPlugin::new(1);
                reference.initialize(rate).unwrap();
                reference
            } else {
                disabled(rate, 1)
            };
            let frames = plugin.latency_samples() + 512.max((rate * 0.03).ceil() as usize);
            let input: Vec<f32> = (0..frames)
                .map(|frame| ((frame as f64 * 440.0 * std::f64::consts::TAU / rate).sin() * 0.2) as f32)
                .collect();
            let first = process_stream(&mut plugin, rate, &input);
            let expected_first = process_stream(&mut reference, rate, &input);
            assert_eq!(first, expected_first, "{rate} Hz enabled={enabled} prefix");
            if enabled {
                assert!(model_frames(&plugin) >= 2, "{rate} Hz prefix missed RNNoise processing");
            }
            for invalid_rate in [f64::NAN, 0.0, f64::MIN_POSITIVE, f64::MAX] {
                assert!(
                    plugin.initialize(invalid_rate).is_err(),
                    "{rate} Hz enabled={enabled} accepted invalid rate {invalid_rate}"
                );
            }
            let next = process_stream(&mut plugin, rate, &input);
            let expected_next = process_stream(&mut reference, rate, &input);
            assert_eq!(next, expected_next, "{rate} Hz enabled={enabled} retained state");
            if enabled {
                assert!(model_frames(&plugin) >= 4, "{rate} Hz suffix missed RNNoise processing");
            }
        }
    }
}

#[test]
fn enabled_adapted_stream_emits_bounded_tail_and_then_stays_complete() {
    for rate in VALIDATOR_RATES {
        let mut plugin = SpeechDenoiserPlugin::new(1);
        plugin.initialize(rate).unwrap();
        let mut continued = SpeechDenoiserPlugin::new(1);
        continued.initialize(rate).unwrap();
        let frames = 257.max((rate * 0.03).ceil() as usize);
        let source: Vec<f32> = (0..frames)
            .map(|frame| if frame == 17 { 0.5 } else { 0.0 })
            .collect();
        let output = render(&mut plugin, rate, &source, 1, &[1, 17, 63, 137], Some(2));
        if rate == 48_000.0 {
            assert_eq!(
                output.len(),
                source.len() + plugin.latency_samples(),
                "{rate} Hz native backend tail length"
            );
        } else {
            assert!(
                output.len() > source.len() + plugin.latency_samples(),
                "{rate} Hz adapted backend tail length"
            );
        }
        assert!(output.iter().all(|value| value.is_finite()), "{rate} Hz nonfinite output");
        let mut offset = 0;
        let mut call = 0;
        let partitions = [1, 17, 63, 137];
        while offset < source.len() {
            let count = partitions[call % partitions.len()].min(source.len() - offset);
            let mut block = source[offset..offset + count].to_vec();
            continued
                .process_in_place(&mut block, &ProcessContext::new(rate, count))
                .unwrap();
            assert_eq!(block, output[offset..offset + count], "{rate} Hz input prefix");
            offset += count;
            call += 1;
        }
        // Drain must match the prefix of an independently prepared instance
        // that continues to receive real zero-valued host input. This catches
        // lost queued converter output at the process-to-drain transition.
        let mut continued_tail = Vec::new();
        let tail_frames = output.len() - source.len();
        while continued_tail.len() < tail_frames {
            let count = (tail_frames - continued_tail.len()).min(64);
            let mut zeros = vec![0.0; count];
            continued
                .process_in_place(&mut zeros, &ProcessContext::new(rate, count))
                .unwrap();
            continued_tail.extend_from_slice(&zeros);
        }
        assert_eq!(continued_tail, output[source.len()..], "{rate} Hz drain continuation");
        let mut tail = [f32::NAN; 64];
        let done = plugin.drain(&mut tail, &ProcessContext::new(rate, 0)).unwrap();
        assert_eq!(done.frames, 0, "{rate} Hz extra drain frames");
        assert!(done.complete, "{rate} Hz drain did not complete");
    }
}
