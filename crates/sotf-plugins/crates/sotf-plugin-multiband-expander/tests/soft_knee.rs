//! Independent steady gain and state-boundary checks for the centered knee.
// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_multiband_expander::{MultibandExpanderPlugin, MultibandExpanderPluginParams};

const N: usize = 1024;
const BIN: usize = 37;
const FRAMES: usize = 64 * N;
// Existing f32 fast log/power paths have independently measured error <=0.01512 dB.
// This new full-curve tolerance does not change any existing 0.01 dB fixture.
const GAIN_TOLERANCE_DB: f64 = 0.03;

fn params(spectral: bool) -> MultibandExpanderPluginParams {
    MultibandExpanderPluginParams {
        num_bands: 1,
        processing_mode: if spectral { "spectral" } else { "time_domain" }.into(),
        threshold_db: -24.0,
        ratio: 4.0,
        knee_db: 12.0,
        range_db: 80.0,
        attack_ms: 1.0,
        release_ms: 10.0,
        hold_ms: 0.0,
        hysteresis_db: 0.0,
        mix: 1.0,
        auto_makeup: Some(false),
        measured_auto_makeup: Some(false),
        sidechain_hpf_hz: Some(0.0),
        ..Default::default()
    }
}

fn gain(level: f64, threshold: f64, ratio: f64, knee: f64, range: f64) -> f64 {
    // Integrate the attenuation-slope ramp from -(R-1) at the lower edge
    // to zero at the upper edge, with zero attenuation at the upper edge.
    // No production attenuation, FFT, fast log or fast power is used.
    let upper = threshold + knee / 2.0;
    let attenuation = if knee < 0.1 || level <= threshold - knee / 2.0 {
        ((threshold - level) * (ratio - 1.0)).max(0.0)
    } else if level >= upper {
        0.0
    } else {
        (upper - level).powi(2) * (ratio - 1.0) / (2.0 * knee)
    };
    10.0f64.powf(-attenuation.min(range) / 20.0)
}

fn hann_gain(level: f64, threshold: f64, ratio: f64, knee: f64, range: f64) -> f64 {
    // Analysis Hann gives normalized magnitudes A,A/2,A/2. Synthesis Hann
    // and four-overlap reconstruction weight their fundamental gains 2/3,1/3.
    (2.0 * gain(level, threshold, ratio, knee, range)
        + gain(level - 20.0 * 2.0f64.log10(), threshold, ratio, knee, range))
        / 3.0
}

fn signal(spectral: bool, channels: usize, frames: usize, prefix: f64, level: f64) -> Vec<f32> {
    let period: Vec<f32> = (0..N)
        .map(|i| {
            if spectral {
                (std::f64::consts::TAU * BIN as f64 * i as f64 / N as f64).cos() as f32
            } else {
                1.0
            }
        })
        .collect();
    let amplitude = 10.0f64.powf(level / 20.0) as f32;
    let prefix = 10.0f64.powf(prefix / 20.0) as f32;
    (0..frames * channels)
        .map(|sample| {
            let frame = sample / channels;
            let sign = if sample.is_multiple_of(channels) {
                1.0
            } else {
                -1.0
            };
            sign * period[frame % N] * if frame < 8 * N { prefix } else { amplitude }
        })
        .collect()
}

fn process(
    plugin: &mut MultibandExpanderPlugin,
    buffer: &mut [f32],
    channels: usize,
    rate: u32,
    pattern: &[usize],
) {
    let mut offset = 0;
    let mut block = 0;
    while offset < buffer.len() / channels {
        let frames = pattern[block % pattern.len()].min(buffer.len() / channels - offset);
        plugin
            .process_in_place(
                &mut buffer[offset * channels..(offset + frames) * channels],
                &ProcessContext::new(rate, frames),
            )
            .unwrap();
        offset += frames;
        block += 1;
    }
}

fn render(
    settings: MultibandExpanderPluginParams,
    rate: u32,
    channels: usize,
    prefix: f64,
    level: f64,
    pattern: &[usize],
) -> Vec<f32> {
    let spectral = settings.processing_mode == "spectral";
    let mut plugin = MultibandExpanderPlugin::with_params(channels, settings);
    plugin.initialize(f64::from(rate)).unwrap();
    let mut output = signal(spectral, channels, FRAMES, prefix, level);
    process(&mut plugin, &mut output, channels, rate, pattern);
    output
}

fn measured_gain_db(
    output: &[f32],
    channels: usize,
    channel: usize,
    level: f64,
    spectral: bool,
) -> f64 {
    let amplitude = f64::from(10.0f64.powf(level / 20.0) as f32);
    let sign = if channel == 0 { 1.0 } else { -1.0 };
    let projection = output[48 * N * channels..]
        .chunks_exact(channels)
        .enumerate()
        .map(|(i, frame)| {
            let carrier = if spectral {
                (std::f64::consts::TAU * BIN as f64 * i as f64 / N as f64).cos()
            } else {
                1.0
            };
            f64::from(frame[channel]) * carrier * sign
        })
        .sum::<f64>()
        / (16 * N) as f64;
    20.0 * (projection * if spectral { 2.0 } else { 1.0 } / amplitude).log10()
}

#[test]
fn spectral_coherent_tone_matches_independent_hann_knee_without_center_jump() {
    let mut around_center = Vec::new();
    for level in [
        -31.0, -30.0, -27.0, -24.01, -24.0, -23.99, -21.0, -18.0, -17.99, -17.97, -17.0,
    ] {
        let expected = 20.0 * hann_gain(level, -24.0, 4.0, 12.0, 80.0).log10();
        for prefix in [-6.0, -50.0] {
            let output = render(params(true), 48_000, 1, prefix, level, &[137, 1, 511]);
            let actual = measured_gain_db(&output, 1, 0, level, true);
            assert!(
                (actual - expected).abs() < GAIN_TOLERANCE_DB,
                "level={level}, prefix={prefix}, actual={actual}, expected={expected}"
            );
            if prefix == -50.0 && [-24.01, -23.99].contains(&level) {
                assert_eq!(output, render(params(true), 48_000, 1, prefix, level, &[1]));
                around_center.push(actual);
            }
        }
    }
    assert!((around_center[1] - around_center[0]).abs() < 0.1);
}

#[test]
fn time_domain_dc_matches_independent_full_curve_and_hard_knee_boundary() {
    for rate in [44_100, 48_000, 96_000] {
        for channels in [1, 2] {
            for linked in [false, true] {
                for knee in [0.0, 0.099, 0.1, 12.0] {
                    for prefix in [-6.0, -50.0] {
                        for level in [
                            -31.0, -30.0, -27.0, -24.05, -24.01, -24.0, -23.99, -23.95, -21.0,
                            -18.0, -17.0,
                        ] {
                            let mut settings = params(false);
                            settings.link_channels = linked;
                            settings.knee_db = knee;
                            let output =
                                render(settings, rate, channels, prefix, level, &[137, 1, 511]);
                            let actual_level =
                                20.0 * f64::from(10.0f64.powf(level / 20.0) as f32).log10();
                            let expected = 20.0
                                * gain(actual_level, -24.0, 4.0, f64::from(knee), 80.0).log10();
                            for channel in 0..channels {
                                let actual =
                                    measured_gain_db(&output, channels, channel, level, false);
                                assert!(
                                    (actual - expected).abs() < GAIN_TOLERANCE_DB,
                                    "rate={rate}, ch={channels}/{channel}, link={linked}, knee={knee}, prefix={prefix}, level={level}, actual={actual}, expected={expected}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn hysteresis_keeps_distinct_histories_between_the_knee_edge_and_close_boundary() {
    // With K=12, the unity edge is -18 and its 4 dB hysteresis close edge is -22.
    // At -20 an open history stays open, while a closed history follows the knee.
    for spectral in [false, true] {
        for rate in [44_100, 48_000, 96_000] {
            for hold in [0.0, 7.0] {
                for level in [-23.0, -20.0, -17.0] {
                    for prefix in [-6.0, -50.0] {
                        let mut settings = params(spectral);
                        settings.hysteresis_db = 4.0;
                        settings.hold_ms = hold;
                        let output = render(settings, rate, 2, prefix, level, &[1, 255, 513]);
                        let bin_gain = |db: f64| {
                            let edge = if prefix == -6.0 { -22.0 } else { -18.0 };
                            if db >= edge {
                                1.0
                            } else {
                                gain(db, -24.0, 4.0, 12.0, 80.0)
                            }
                        };
                        let expected = if spectral {
                            (2.0 * bin_gain(level) + bin_gain(level - 20.0 * 2.0f64.log10())) / 3.0
                        } else {
                            bin_gain(level)
                        };
                        for channel in 0..2 {
                            let actual = measured_gain_db(&output, 2, channel, level, spectral);
                            assert!(
                                (actual - 20.0 * expected.log10()).abs() < GAIN_TOLERANCE_DB,
                                "spectral={spectral}, rate={rate}, hold={hold}, level={level}, prefix={prefix}, actual={actual}, expected={}",
                                20.0 * expected.log10()
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn hold_keeps_its_existing_sample_and_hop_duration() {
    for rate in [44_100, 48_000, 96_000] {
        for knee in [0.0, 12.0] {
            for hold_ms in [0.0, 3.125] {
                let mut settings = params(false);
                settings.knee_db = knee;
                settings.hold_ms = hold_ms;
                settings.attack_ms = 0.1;
                let mut plugin = MultibandExpanderPlugin::with_params(1, settings);
                plugin.initialize(f64::from(rate)).unwrap();
                // For a soft knee this is ABOVE its center, but below unity.
                let level = if knee == 0.0 { -27.0 } else { -21.0 };
                let mut output = signal(false, 1, 4096, level, level);
                process(&mut plugin, &mut output, 1, rate, &[1, 17, 513]);
                let hold_samples = (f64::from(hold_ms) * 0.001 * f64::from(rate)).round() as usize;
                // Existing semantics: the first below-edge sample enters Hold;
                // the following H samples consume the count, then Closing starts.
                assert!(
                    output[..=hold_samples]
                        .iter()
                        .all(|&sample| sample == output[0])
                );
                assert!(
                    output[hold_samples + 1] < output[0] * 0.99,
                    "rate={rate}, knee={knee}, hold={hold_ms}"
                );
            }

            let mut settings = params(true);
            settings.knee_db = knee;
            settings.hold_ms = 50.0;
            let mut reference_settings = settings.clone();
            reference_settings.ratio = 1.0;
            let actual = render(settings, rate, 1, -50.0, -50.0, &[17, 1, 511]);
            let reference = render(reference_settings, rate, 1, -50.0, -50.0, &[8193]);
            let hold_hops = (0.050 * f64::from(rate) / 256.0).round() as usize;
            // The first hop enters Hold; hop H+2 closes. Its window origin is
            // (H-2)*256 because the first origin is -768. Hann synthesis is
            // exactly zero at that origin, so an audible change starts later.
            let first_changed_window = N + (hold_hops - 2) * 256;
            assert_eq!(
                &actual[..=first_changed_window],
                &reference[..=first_changed_window]
            );
            assert!(
                actual[first_changed_window + 1..first_changed_window + 256]
                    .iter()
                    .zip(&reference[first_changed_window + 1..first_changed_window + 256])
                    .any(|(&a, &b)| (a - b).abs() > 1e-5),
                "rate={rate}, knee={knee}"
            );
        }
    }
}

#[test]
fn band_overrides_and_live_threshold_knee_changes_keep_the_input_clock() {
    for spectral in [false, true] {
        for rate in [48_000, 96_000] {
            for channels in [1, 2] {
                for overrides in [false, true] {
                    let mut baseline = None;
                    for pattern in [&[1][..], &[137, 1, 511][..], &[8193][..]] {
                        let mut settings = params(spectral);
                        let band = if spectral && overrides { 1 } else { 0 };
                        if overrides {
                            settings.num_bands = if spectral { 3 } else { 1 };
                            // Keep bin37 and both Hann neighbors in band1 at both rates.
                            settings.crossover_frequencies = vec![200.0, 5000.0, 8000.0, 12000.0];
                            settings
                                .bands
                                .resize_with(settings.num_bands, Default::default);
                            settings.threshold_db = -60.0;
                            settings.knee_db = 0.0;
                            settings.bands[band].threshold_db = Some(-24.0);
                            settings.bands[band].knee_db = Some(12.0);
                        }
                        let mut plugin = MultibandExpanderPlugin::with_params(channels, settings);
                        plugin.initialize(f64::from(rate)).unwrap();
                        let mut output = signal(spectral, channels, FRAMES, -50.0, -29.0);
                        let change = 24 * N * channels;
                        process(&mut plugin, &mut output[..change], channels, rate, pattern);
                        for (field, value) in [("threshold", -30.0), ("knee", 6.0)] {
                            let id = if overrides {
                                format!("band_{band}_{field}")
                            } else {
                                field.into()
                            };
                            plugin
                                .set_parameter(ParameterId::from(id), ParameterValue::Float(value))
                                .unwrap();
                        }
                        process(&mut plugin, &mut output[change..], channels, rate, pattern);
                        let expected = if spectral {
                            hann_gain(-29.0, -30.0, 4.0, 6.0, 80.0)
                        } else {
                            gain(-29.0, -30.0, 4.0, 6.0, 80.0)
                        };
                        for channel in 0..channels {
                            let actual =
                                measured_gain_db(&output, channels, channel, -29.0, spectral);
                            assert!(
                                (actual - 20.0 * expected.log10()).abs() < GAIN_TOLERANCE_DB,
                                "spectral={spectral}, rate={rate}, channels={channels}, overrides={overrides}, actual={actual}"
                            );
                        }
                        if let Some(expected) = &baseline {
                            assert_eq!(&output, expected);
                        } else {
                            baseline = Some(output);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn neutral_controls_and_sub_point_one_knees_preserve_their_waveforms() {
    for spectral in [false, true] {
        for variant in 0..5 {
            let mut settings = params(spectral);
            match variant {
                0 => settings.ratio = 1.0,
                1 => settings.range_db = 0.0,
                2 => settings.mix = 0.0,
                3 => {
                    settings.bands.resize_with(1, Default::default);
                    settings.bands[0].active = false;
                }
                _ => {
                    settings.bands.resize_with(1, Default::default);
                    settings.bands[0].bypass = true;
                }
            }
            let source = signal(spectral, 2, FRAMES, -6.0, -21.0);
            let output = render(settings, 48_000, 2, -6.0, -21.0, &[137, 1, 511]);
            let delay = if spectral { N * 2 } else { 0 };
            for (i, &actual) in output.iter().enumerate() {
                let expected = i.checked_sub(delay).map_or(0.0, |index| source[index]);
                assert!(
                    (actual - expected).abs() < 2e-6,
                    "spectral={spectral}, variant={variant}, sample={i}"
                );
            }
        }
        for linked in [false, true] {
            let mut settings = params(spectral);
            settings.link_channels = linked;
            settings.knee_db = 0.0;
            settings.hysteresis_db = 4.0;
            settings.hold_ms = 7.0;
            let hard = render(settings.clone(), 48_000, 2, -6.0, -31.0, &[137, 1, 511]);
            settings.knee_db = 0.099;
            assert_eq!(hard, render(settings, 48_000, 2, -6.0, -31.0, &[1]));
        }
    }
}
