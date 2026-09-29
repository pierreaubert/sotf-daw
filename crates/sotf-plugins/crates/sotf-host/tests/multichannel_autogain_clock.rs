// Rust guideline compliant 2026-02-21
use sotf_host::auto_gain::{AutoGain, AutoGainLoudnessType, AutoGainParams};
use sotf_host::multichannel_auto_gain::MultichannelAutoGain;
use sotf_host::speaker_config::{SpeakerConfig, get_speaker_config};

fn params() -> AutoGainParams {
    AutoGainParams {
        enabled: true,
        loudness_type: AutoGainLoudnessType::Momentary,
        max_gain_db: 12.0,
        smoothing_ms: 100.0,
    }
}

fn source(frames: usize, channels: usize, offset: usize) -> Vec<f32> {
    (offset..offset + frames)
        .flat_map(|frame| {
            (0..channels).map(move |ch| {
                let level = [0.03, 0.1, 0.017][frame / 7937 % 3];
                let t = frame as f64 / 48000.0;
                (level * (std::f64::consts::TAU * (997.0 + ch as f64 * 151.0) * t).sin()) as f32
            })
        })
        .collect()
}

// Independent ordered fold, using per-speaker contributions to each frame.
// Speaker order intentionally matches the public compatibility contract.
fn fold(output: &[f32], channels: usize, config: &SpeakerConfig) -> Vec<f32> {
    output
        .chunks_exact(channels)
        .flat_map(|frame| {
            if channels == 1 {
                return [frame[0], frame[0]];
            }
            if channels == 2 {
                return [frame[0], frame[1]];
            }
            config.speakers.iter().fold([0.0, 0.0], |mut lr, sp| {
                if !sp.is_lfe && sp.channel < channels {
                    let x = frame[sp.channel];
                    if sp.azimuth > 10.0 {
                        lr[0] += x;
                    } else if sp.azimuth < -10.0 {
                        lr[1] += x;
                    } else {
                        let center = x * std::f32::consts::FRAC_1_SQRT_2;
                        lr[0] += center;
                        lr[1] += center;
                    }
                }
                lr
            })
        })
        .collect()
}

fn compare_legacy_schedule(blocks: &[usize]) {
    for (layout, channels) in [
        ("1.0", 1),
        ("2.0", 2),
        ("5.1", 6),
        ("7.1.4", 12),
        ("5.1", 2),
    ] {
        let config = get_speaker_config(layout).unwrap();
        let mut actual = MultichannelAutoGain::new(48000, params()).unwrap();
        let mut expected = AutoGain::new(2, 48000, params()).unwrap();
        let mut offset = 0;
        for &frames in blocks.iter().cycle().take(90) {
            let input = source(frames, 2, offset);
            let mut output = source(frames, channels, offset);
            let mut reference = output.clone();
            expected.measure_input(&input).unwrap();
            expected
                .measure_output(&fold(&reference, channels, config))
                .unwrap();
            for frame in reference.chunks_exact_mut(channels) {
                let gain = expected.next_gain_linear();
                for sample in frame {
                    *sample *= gain;
                }
            }
            actual.measure_input(&input).unwrap();
            actual
                .measure_and_apply(&mut output, frames, channels, config)
                .unwrap();
            assert_eq!(
                output, reference,
                "legacy {layout}/{channels}, offset {offset}"
            );
            let a = actual.data();
            let b = expected.get_data();
            assert_eq!(a.gain_db.to_bits(), b.gain_db.to_bits());
            assert_eq!(a.input_lufs.to_bits(), b.input_lufs.to_bits());
            assert_eq!(a.output_lufs.to_bits(), b.output_lufs.to_bits());
            offset += frames;
        }
    }
}

#[test]
fn legacy_normal_callbacks_match_original_scalar_schedule_exactly() {
    compare_legacy_schedule(&[1, 137, 4096, 8192, 17]);
}

#[test]
fn legacy_oversized_callbacks_retain_one_refresh_and_all_frames() {
    compare_legacy_schedule(&[8193, 32769, 17]);
}

fn reference_paired(
    sample_rate: u32,
    input: &[f32],
    raw: &[f32],
    channels: usize,
    config: &SpeakerConfig,
) -> Vec<f32> {
    let interval = (sample_rate / 10).max(1) as usize;
    let mut scalar = AutoGain::new(2, sample_rate, params()).unwrap();
    let mut result = raw.to_vec();
    // This independent schedule applies the preceding target first. Measurement
    // happens only after completing each fixed interval, never at host boundaries.
    for (block, measured) in result
        .chunks_mut(interval * channels)
        .zip(raw.chunks(interval * channels))
        .zip(input.chunks(interval * 2))
    {
        let (block, raw_block) = block;
        for frame in block.chunks_exact_mut(channels) {
            let gain = scalar.next_gain_linear();
            for sample in frame {
                *sample *= gain;
            }
        }
        scalar.ingest_input(measured).unwrap();
        scalar
            .ingest_output(&fold(raw_block, channels, config))
            .unwrap();
        if block.len() == interval * channels {
            scalar.refresh_input_measurement();
            scalar.refresh_output_measurement();
        }
    }
    result
}

fn apply_paired(
    meter: &mut MultichannelAutoGain,
    input: &[f32],
    raw: &[f32],
    channels: usize,
    config: &SpeakerConfig,
    sizes: &[usize],
) -> Vec<f32> {
    let mut output = raw.to_vec();
    let total = input.len() / 2;
    let mut position = 0;
    for &size in sizes.iter().cycle() {
        if position == total {
            break;
        }
        let frames = size.min(total - position);
        meter
            .measure_aligned_and_apply(
                &input[position * 2..(position + frames) * 2],
                &mut output[position * channels..(position + frames) * channels],
                frames,
                channels,
                config,
            )
            .unwrap();
        position += frames;
    }
    output
}

#[test]
fn paired_clock_matches_independent_fixed_boundary_scalar_schedule() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        let total = rate as usize + 17;
        let input = source(total, 2, 0);
        for (layout, channels) in [
            ("1.0", 1),
            ("2.0", 2),
            ("5.1", 6),
            ("7.1.4", 12),
            ("5.1", 2),
        ] {
            let config = get_speaker_config(layout).unwrap();
            let raw = source(total, channels, 37);
            let expected = reference_paired(rate, &input, &raw, channels, config);
            for sizes in [&[1, 17, 137, 512][..], &[8193, 32769][..]] {
                let mut actual = MultichannelAutoGain::new(rate, params()).unwrap();
                let output = apply_paired(&mut actual, &input, &raw, channels, config, sizes);
                assert_eq!(
                    output, expected,
                    "rate {rate}, {layout}/{channels}, {sizes:?}"
                );
            }
        }
    }
}

#[test]
fn paired_future_samples_cannot_change_an_earlier_output_prefix() {
    let config = get_speaker_config("5.1").unwrap();
    let mut a = MultichannelAutoGain::new(48000, params()).unwrap();
    let mut b = MultichannelAutoGain::new(48000, params()).unwrap();
    let warm_input = source(48_017, 2, 0);
    let warm_output = source(48_017, 6, 37);
    apply_paired(&mut a, &warm_input, &warm_output, 6, config, &[137]);
    apply_paired(&mut b, &warm_input, &warm_output, 6, config, &[137]);
    let input = source(32769, 2, 48_017);
    let raw = source(32769, 6, 48_054);
    let mut different_input = input.clone();
    let mut different_raw = raw.clone();
    different_input[8193 * 2..]
        .iter_mut()
        .for_each(|v| *v *= 10.0);
    different_raw[8193 * 6..]
        .iter_mut()
        .for_each(|v| *v *= 0.01);
    let original = apply_paired(&mut a, &input, &raw, 6, config, &[32769]);
    let changed = apply_paired(
        &mut b,
        &different_input,
        &different_raw,
        6,
        config,
        &[32769],
    );
    assert_eq!(original[..8193 * 6], changed[..8193 * 6]);
    assert_ne!(original[8193 * 6..], changed[8193 * 6..]);
}

#[test]
fn disabled_interval_preserves_paused_clock_and_existing_scalar_toggle_behavior() {
    let config = get_speaker_config("2.0").unwrap();
    let mut a = MultichannelAutoGain::new(48000, params()).unwrap();
    let mut b = MultichannelAutoGain::new(48000, params()).unwrap();
    let input = source(48_137, 2, 0);
    let raw: Vec<_> = input.iter().map(|x| x * 0.25).collect();
    assert_eq!(
        apply_paired(&mut a, &input, &raw, 2, config, &[137]),
        apply_paired(&mut b, &input, &raw, 2, config, &[8193])
    );
    a.set_enabled(false);
    b.set_enabled(false);
    let before = a.data();
    assert_eq!(apply_paired(&mut a, &input, &raw, 2, config, &[32769]), raw);
    assert_eq!(a.data().input_lufs.to_bits(), before.input_lufs.to_bits());
    assert_eq!(a.data().output_lufs.to_bits(), before.output_lufs.to_bits());
    a.set_enabled(true);
    a.set_enabled(true); // Same-value snapshot must not restart the interval.
    b.set_enabled(true);
    assert_eq!(
        apply_paired(&mut a, &input, &raw, 2, config, &[8193]),
        apply_paired(&mut b, &input, &raw, 2, config, &[17, 137])
    );
}

#[test]
fn paired_invalid_shapes_and_zero_calls_do_not_advance_state_or_output() {
    let config = get_speaker_config("2.0").unwrap();
    let mut a = MultichannelAutoGain::new(48000, params()).unwrap();
    let mut b = MultichannelAutoGain::new(48000, params()).unwrap();
    let input = source(24_799, 2, 0);
    let raw: Vec<_> = input.iter().map(|v| v * 0.25).collect();
    apply_paired(&mut a, &input, &raw, 2, config, &[137]);
    apply_paired(&mut b, &input, &raw, 2, config, &[137]);
    let mut sentinel = [123.0; 4];
    for (invalid, frames, channels) in [
        (&[0.0; 3][..], 2, 2),
        (&[0.0; 4][..], 3, 2),
        (&[][..], usize::MAX, 2),
    ] {
        assert!(
            a.measure_aligned_and_apply(invalid, &mut sentinel, frames, channels, config)
                .is_err()
        );
        assert_eq!(sentinel, [123.0; 4]);
    }
    a.measure_aligned_and_apply(&[], &mut [], 0, 2, config)
        .unwrap();
    assert_eq!(
        apply_paired(&mut a, &input, &raw, 2, config, &[17]),
        apply_paired(&mut b, &input, &raw, 2, config, &[8193])
    );
}

#[test]
fn paired_reset_restarts_clock_and_rate_change_preserves_existing_gain_history() {
    let config = get_speaker_config("2.0").unwrap();
    let mut warm = MultichannelAutoGain::new(48000, params()).unwrap();
    let input = source(48_137, 2, 0);
    let raw: Vec<_> = input.iter().map(|v| v * 0.25).collect();
    apply_paired(&mut warm, &input, &raw, 2, config, &[137]);
    let previous_gain = warm.data().gain_db;
    assert!(previous_gain > 1.0);
    warm.set_sample_rate(96_000).unwrap();
    assert_eq!(warm.data().gain_db, previous_gain);
    warm.reset();
    let mut fresh = MultichannelAutoGain::new(96_000, params()).unwrap();
    assert_eq!(
        apply_paired(&mut warm, &input, &raw, 2, config, &[8193]),
        apply_paired(&mut fresh, &input, &raw, 2, config, &[137])
    );
}

#[test]
fn paired_peaks_cover_whole_intervals_and_ignore_unpublished_future_fragments() {
    let rate = 192_000;
    let interval = 19_200;
    let cfg = get_speaker_config("5.1").unwrap();
    let frames = interval * 2 + 17;
    let mut input = vec![0.0; frames * 2];
    let mut raw = vec![0.0; frames * 6];
    // Early and late markers straddle both the 8192 scratch cut and callbacks.
    input[2] = -0.75;
    input[(interval + 8193) * 2 + 1] = 0.625;
    input[interval * 4] = 8.0; // Not published: the third interval is incomplete.
    raw[6] = 0.5;
    raw[8193 * 6 + 2] = -0.25; // Center is split into the measured pair.
    raw[(interval + 7) * 6 + 1] = -0.375;
    raw[(interval + 2) * 6 + 3] = 100.0; // LFE excluded from meter peaks.
    raw[interval * 2 * 6] = 4.0;
    for sizes in [&[1, 137, 8193][..], &[32769][..]] {
        let mut meter = MultichannelAutoGain::new(rate, params()).unwrap();
        apply_paired(
            &mut meter,
            &input[..interval * 2],
            &raw[..interval * 6],
            6,
            cfg,
            sizes,
        );
        assert_eq!(meter.data().input_peak, 0.75);
        assert_eq!(meter.data().output_peak, 0.5);
        apply_paired(
            &mut meter,
            &input[interval * 2..],
            &raw[interval * 6..],
            6,
            cfg,
            sizes,
        );
        assert_eq!(meter.data().input_peak, 0.625);
        assert_eq!(meter.data().output_peak, 0.375);
        meter.reset();
        assert_eq!(meter.data().input_peak, 0.0);
        assert_eq!(meter.data().output_peak, 0.0);
        apply_paired(&mut meter, &input[..34], &raw[..102], 6, cfg, sizes);
        assert_eq!(meter.data().input_peak, 0.0);
        assert_eq!(meter.data().output_peak, 0.0);
    }
}

#[test]
fn legacy_oversized_output_peak_covers_early_scratch_spans() {
    let cfg = get_speaker_config("5.1").unwrap();
    let mut meter = MultichannelAutoGain::new(48000, params()).unwrap();
    let input = source(32769, 2, 0);
    let mut output = vec![0.0; 32769 * 6];
    output[6] = -0.75;
    output[30_000 * 6] = 0.125;
    output[8193 * 6 + 3] = 100.0; // LFE is not measured.
    meter.measure_input(&input).unwrap();
    let legacy_input_peak = meter.data().input_peak;
    let mut reference = AutoGain::new(2, 48000, params()).unwrap();
    reference.measure_input(&input).unwrap();
    assert_eq!(legacy_input_peak, reference.get_data().input_peak);
    meter.measure_and_apply(&mut output, 32769, 6, cfg).unwrap();
    assert_eq!(meter.data().input_peak, legacy_input_peak);
    assert_eq!(meter.data().output_peak, 0.75);
}

#[test]
fn paired_settled_gain_matches_independent_amplitude_ratio_in_both_directions() {
    let cfg = get_speaker_config("2.0").unwrap();
    for rate in [48_000, 192_000] {
        for raw_gain in [0.5, 2.0] {
            let total = rate as usize * 4 + 17;
            let input: Vec<_> = (0..total)
                .flat_map(|n| {
                    let x = (std::f64::consts::TAU * 997.0 * n as f64 / f64::from(rate)).sin()
                        as f32
                        * 0.01;
                    [x, x * 0.5]
                })
                .collect();
            let raw: Vec<_> = input.iter().map(|v| v * raw_gain).collect();
            let mut meter = MultichannelAutoGain::new(rate, params()).unwrap();
            let output = apply_paired(&mut meter, &input, &raw, 2, cfg, &[8193, 137, 32769]);
            let expected_db = -20.0 * f64::from(raw_gain).log10();
            assert!((f64::from(meter.data().gain_db) - expected_db).abs() < 0.005);
            let start = (total - rate as usize / 10) * 2;
            let energy =
                |samples: &[f32]| samples.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>();
            let measured_db = 10.0 * (energy(&output[start..]) / energy(&input[start..])).log10();
            assert!(
                measured_db.abs() < 0.005,
                "rate={rate}, raw={raw_gain}, error={measured_db} dB"
            );
        }
    }
}

#[test]
fn fold_uses_exact_center_boundaries_and_ignores_lfe_and_missing_channels() {
    use sotf_host::speaker_config::SpeakerPosition;
    const SPEAKERS: [SpeakerPosition; 6] = [
        SpeakerPosition {
            label: "C",
            name: "At +10",
            azimuth: 10.0,
            elevation: 0.0,
            channel: 0,
            is_lfe: false,
        },
        SpeakerPosition {
            label: "C",
            name: "At -10",
            azimuth: -10.0,
            elevation: 0.0,
            channel: 1,
            is_lfe: false,
        },
        SpeakerPosition {
            label: "L",
            name: "Left",
            azimuth: 10.1,
            elevation: 0.0,
            channel: 2,
            is_lfe: false,
        },
        SpeakerPosition {
            label: "R",
            name: "Right",
            azimuth: -10.1,
            elevation: 0.0,
            channel: 3,
            is_lfe: false,
        },
        SpeakerPosition {
            label: "LFE",
            name: "LFE",
            azimuth: 0.0,
            elevation: 0.0,
            channel: 4,
            is_lfe: true,
        },
        SpeakerPosition {
            label: "L",
            name: "Missing",
            azimuth: 30.0,
            elevation: 0.0,
            channel: 99,
            is_lfe: false,
        },
    ];
    let cfg = SpeakerConfig {
        id: "boundary",
        name: "Boundary",
        description: "Independent fold fixture",
        total_channels: 6,
        speakers: &SPEAKERS,
        meter_groups: &[],
    };
    let left =
        (0.01 * std::f32::consts::FRAC_1_SQRT_2 + 0.02 * std::f32::consts::FRAC_1_SQRT_2) + 0.03;
    let right =
        (0.01 * std::f32::consts::FRAC_1_SQRT_2 + 0.02 * std::f32::consts::FRAC_1_SQRT_2) + 0.04;
    let frames = 48_000;
    let input: Vec<_> = std::iter::repeat_n([left, right], frames)
        .flatten()
        .collect();
    let raw: Vec<_> = std::iter::repeat_n([0.01, 0.02, 0.03, 0.04, 100.0, 999.0], frames)
        .flatten()
        .collect();
    let mut meter = MultichannelAutoGain::new(48000, params()).unwrap();
    let output = apply_paired(&mut meter, &input, &raw, 6, &cfg, &[8193, 32769, 137]);
    assert_eq!(output, raw);
    assert_eq!(meter.data().gain_db, 0.0);
    assert_eq!(meter.data().input_peak, f64::from(right));
    assert_eq!(meter.data().output_peak, f64::from(right));
}
