//! Independent model-block and output-clock references for RNNoise framing.

// Rust guideline compliant 2026-02-21
use plugins_denoiser::rnnoise::RnnoiseBackend;

const FRAME: usize = 480;
const LATENCY: usize = 960;

fn source(channels: usize, opposite: bool) -> Vec<f32> {
    (0..(18 * FRAME + 137) * channels)
        .map(|i| {
            let sample = (((i / channels * 37 + 11) % 257) as f32 - 128.) / 512.;
            if opposite && i % channels == 1 {
                -sample
            } else {
                sample
            }
        })
        .collect()
}

// This reference has no streaming rings or arbitrary-block scheduler. Preserve
// every direct model result, including the nonempty first synthesis frame.
fn direct_model(input: &[f32], channels: usize) -> Vec<f32> {
    let mut states: Vec<_> = (0..channels)
        .map(|_| nnnoiseless::DenoiseState::new())
        .collect();
    let mut detector = nnnoiseless::DenoiseState::new();
    let mut result = vec![0.; FRAME * channels];
    let mut model_input = [0.; FRAME];
    let mut model_output = [0.; FRAME];
    for block in input.chunks_exact(FRAME * channels) {
        let mut gains = [1.; nnnoiseless::DENOISE_BAND_COUNT];
        if channels == 2 {
            // Fixtures have R=L or R=-L. The polarity-aware energy-normalized
            // common detector is exactly L in both cases, including silence.
            for i in 0..FRAME {
                assert_eq!(block[2 * i].abs(), block[2 * i + 1].abs());
                model_input[i] = block[2 * i] * 32768.;
            }
            gains = detector
                .process_frame_with_analysis(&mut model_output, &model_input)
                .band_gains;
        }
        let start = result.len();
        result.resize(start + FRAME * channels, 0.);
        for ch in 0..channels {
            for i in 0..FRAME {
                model_input[i] = block[i * channels + ch] * 32768.;
            }
            if channels == 1 {
                states[ch].process_frame(&mut model_output, &model_input);
            } else {
                states[ch].process_frame_with_band_gains(&mut model_output, &model_input, &gains);
            }
            for i in 0..FRAME {
                result[start + i * channels + ch] = model_output[i] / 32768.;
            }
        }
    }
    result.truncate(input.len());
    assert_eq!(result.len(), input.len());
    result
}

fn render(
    backend: &mut RnnoiseBackend,
    input: &[f32],
    channels: usize,
    blocks: &[usize],
    events: &[(usize, bool)],
) -> Vec<f32> {
    let mut output = input.to_vec();
    let mut position = 0;
    let mut event = 0;
    let mut call = 0;
    while position < input.len() / channels {
        if event + 1 < events.len() && events[event + 1].0 == position {
            event += 1;
        }
        let next_event = events.get(event + 1).map_or(usize::MAX, |e| e.0);
        let frames = blocks[call % blocks.len()]
            .min(input.len() / channels - position)
            .min(next_event - position);
        assert_eq!(
            backend.process(
                &mut output[position * channels..(position + frames) * channels],
                frames,
                channels,
                events[event].1,
            ),
            frames
        );
        position += frames;
        call += 1;
    }
    output
}

#[test]
fn complete_wet_waveform_matches_direct_model_with_one_frame_queue() {
    for (channels, opposite) in [(1, false), (2, false), (2, true)] {
        let input = source(channels, opposite);
        let expected = direct_model(&input, channels);
        assert!(
            expected[FRAME * channels..2 * FRAME * channels]
                .iter()
                .any(|sample| sample.abs() > 1e-5)
        );
        let mut backend = RnnoiseBackend::new();
        backend.initialize(48000, channels).unwrap();
        for blocks in [
            &[1][..],
            &[7],
            &[137],
            &[479],
            &[480],
            &[481],
            &[8193],
            &[1, 479, 17, 4096],
        ] {
            backend.reset();
            let actual = render(&mut backend, &input, channels, blocks, &[(0, false)]);
            assert_eq!(
                actual, expected,
                "channels={channels} opposite={opposite} blocks={blocks:?}"
            );
        }
    }
}

#[test]
fn output_clock_crossfades_blend_direct_wet_and_aligned_dry() {
    let events = [
        (0, false),
        (37, true),
        (479, false),
        (480, true),
        (481, false),
        (1001, true),
        (1203, false),
        (1921, true),
        (2600, false),
        (3109, true),
        (4801, false),
    ];
    for (channels, opposite) in [(1, false), (2, false), (2, true)] {
        let input = source(channels, opposite);
        let wet = direct_model(&input, channels);
        let mut expected = wet.clone();
        let mut initial_mix = 0.0_f64;
        for (event, &(start, bypass)) in events.iter().enumerate() {
            let end = events
                .get(event + 1)
                .map_or(input.len() / channels, |e| e.0);
            let target = f64::from(bypass);
            for frame in start..end {
                // Closed-form linear ramp, independent of production's repeated
                // f32 additions. Only their bounded rounding error is tolerated.
                let distance = (frame + 1 - start) as f64 / FRAME as f64;
                let mix = initial_mix + (target - initial_mix).clamp(-distance, distance);
                for ch in 0..channels {
                    let i = frame * channels + ch;
                    let dry = frame
                        .checked_sub(LATENCY)
                        .map_or(0., |n| input[n * channels + ch]);
                    expected[i] = (f64::from(wet[i]) * (1. - mix) + f64::from(dry) * mix) as f32;
                }
            }
            let distance = (end - start) as f64 / FRAME as f64;
            initial_mix += (target - initial_mix).clamp(-distance, distance);
        }
        let mut backend = RnnoiseBackend::new();
        backend.initialize(48000, channels).unwrap();
        let mut first = None;
        for blocks in [&[1][..], &[137], &[8193], &[479, 7, 481, 4096]] {
            backend.reset();
            let actual = render(&mut backend, &input, channels, blocks, &events);
            let error = actual
                .iter()
                .zip(&expected)
                .map(|(a, b)| (a - b).abs())
                .fold(0., f32::max);
            assert!(
                error < 1e-5,
                "channels={channels} opposite={opposite} blocks={blocks:?} error={error}"
            );
            if let Some(first) = &first {
                assert_eq!(&actual, first);
            } else {
                first = Some(actual);
            }
            assert_eq!(
                backend.analyzer_data().model_frames,
                (input.len() / channels / FRAME) as u64
            );
        }
    }
}

#[test]
fn empty_call_does_not_latch_initial_bypass_before_real_audio() {
    for channels in [1, 2] {
        for first_bypass in [false, true] {
            let input = source(channels, false);
            let mut actual = RnnoiseBackend::new();
            let mut fresh = RnnoiseBackend::new();
            actual.initialize(48000, channels).unwrap();
            fresh.initialize(48000, channels).unwrap();
            assert_eq!(actual.process(&mut [], 0, channels, !first_bypass), 0);
            // A second switch before the first wet frame makes the mistakenly
            // latched initial mix observable, even though startup itself is zero.
            let events = [(0, first_bypass), (137, !first_bypass)];
            let a = render(&mut actual, &input, channels, &[1, 137], &events);
            let b = render(&mut fresh, &input, channels, &[1, 137], &events);
            let error = a
                .iter()
                .zip(&b)
                .map(|(a, b)| (a - b).abs())
                .fold(0., f32::max);
            assert_eq!(error, 0., "channels={channels} first_bypass={first_bypass}");
        }
    }
}

#[test]
fn rejected_initialization_preserves_prepared_stream_and_model_history() {
    for channels in [1, 2] {
        let input = source(channels, false);
        let mut actual = RnnoiseBackend::new();
        let mut twin = RnnoiseBackend::new();
        assert_eq!(actual.latency_samples(), LATENCY);
        actual.initialize(48000, channels).unwrap();
        twin.initialize(48000, channels).unwrap();
        assert_eq!(
            render(&mut actual, &input, channels, &[137], &[(0, false)]),
            render(&mut twin, &input, channels, &[137], &[(0, false)])
        );
        for (rate, invalid_channels) in [(44100, channels), (48000, 0), (48000, 3)] {
            assert!(actual.initialize(rate, invalid_channels).is_err());
        }
        assert_eq!(
            render(&mut actual, &input, channels, &[479, 1, 8193], &[(0, true)]),
            render(&mut twin, &input, channels, &[479, 1, 8193], &[(0, true)])
        );
        assert_eq!(actual.analyzer_data(), twin.analyzer_data());
    }
}
