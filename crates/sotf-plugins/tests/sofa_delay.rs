//! Independent physical SOFA delay checks, including rate conversion (AUD109/AUD114).
// Rust guideline compliant 2026-02-21
use sofa_reader::{SofaFile, SofaWriter};
use sotf_host::sofa::load_sofa;
use sotf_host::{ParameterId, ParameterValue, Plugin, ProcessContext};
use sotf_plugin_binaural::{BinauralDecoderPlugin, RoomModel};
use sotf_plugin_xtc::{XtcPlugin, XtcPluginParams};
use std::path::Path;

const RATE: u32 = 48_000;
const FFT: usize = 128;
const IR: usize = 16;
const AMPLITUDES: [[f32; 2]; 3] = [[1.0, 0.2], [0.2, 1.0], [0.6, 0.4]];
const PER_MEASUREMENT: [[usize; 2]; 3] = [[0, 3], [2, 7], [1, 5]];

struct Fixture {
    length: usize,
    shifts: [[usize; 2]; 3],
    delay: Option<(Vec<&'static str>, Vec<f64>)>,
    rate_dimensions: Vec<&'static str>,
    rates: Vec<f64>,
    rate_attribute: bool,
    ir_dimensions: Vec<&'static str>,
    ir_samples: Option<Vec<f64>>,
    coordinate_type: Option<&'static str>,
    positions: [f64; 9],
}

impl Default for Fixture {
    fn default() -> Self {
        Self {
            length: IR,
            shifts: [[0; 2]; 3],
            delay: None,
            rate_dimensions: vec!["I"],
            rates: vec![f64::from(RATE)],
            rate_attribute: false,
            ir_dimensions: vec!["M", "R", "N"],
            ir_samples: None,
            coordinate_type: Some("spherical"),
            positions: [30.0, 0.0, 2.0, -30.0, 0.0, 2.0, 0.0, 0.0, 2.0],
        }
    }
}

fn write_fixture(path: &Path, fixture: &Fixture) {
    let mut writer = SofaWriter::new();
    for (key, value) in [
        ("Conventions", "SOFA"),
        ("Version", "1.0"),
        ("SOFAConventions", "SimpleFreeFieldHRIR"),
        ("SOFAConventionsVersion", "1.0"),
        ("DataType", "FIR"),
    ] {
        writer.add_attribute_str(key, value);
    }
    for (name, length) in [
        ("I", 1),
        ("M", 3),
        ("R", 2),
        ("N", fixture.length),
        ("C", 3),
    ] {
        writer.add_dimension(name, length);
    }
    if fixture.rate_attribute {
        writer.add_attribute_f64("Data.SamplingRate", fixture.rates[0]);
    } else {
        writer.add_variable_f64("Data.SamplingRate", &fixture.rate_dimensions);
        writer
            .write_f64("Data.SamplingRate", &fixture.rates)
            .unwrap();
    }
    writer.add_variable_f64("SourcePosition", &["M", "C"]);
    writer
        .write_f64("SourcePosition", &fixture.positions)
        .unwrap();
    if let Some(coordinates) = fixture.coordinate_type {
        writer
            .add_variable_attribute_str("SourcePosition", "Type", coordinates)
            .unwrap();
    }
    let samples = if let Some(samples) = &fixture.ir_samples {
        assert_eq!(samples.len(), 3 * 2 * fixture.length);
        samples.clone()
    } else {
        let mut samples = vec![0.0; 3 * 2 * fixture.length];
        for (m, ears) in fixture.shifts.iter().enumerate() {
            for (ear, &shift) in ears.iter().enumerate() {
                samples[(m * 2 + ear) * fixture.length + shift] = f64::from(AMPLITUDES[m][ear]);
            }
        }
        samples
    };
    writer.add_variable_f64("Data.IR", &fixture.ir_dimensions);
    writer.write_f64("Data.IR", &samples).unwrap();
    if let Some((dimensions, values)) = &fixture.delay {
        writer.add_variable_f64("Data.Delay", dimensions);
        writer.write_f64("Data.Delay", values).unwrap();
    }
    writer.finish(path).unwrap();
}

fn write_file(
    path: &Path,
    delays: Option<(&'static str, &[[usize; 2]; 3])>,
    shift: &[[usize; 2]; 3],
) {
    let delay = delays.map(|(layout, delay)| {
        let values = delay[..if layout == "I" { 1 } else { 3 }]
            .iter()
            .flat_map(|row| row.map(|x| x as f64))
            .collect();
        (vec![layout, "R"], values)
    });
    write_fixture(
        path,
        &Fixture {
            shifts: *shift,
            delay,
            ..Default::default()
        },
    );
}

fn write_valid_fractional_fixture(path: &Path, sample_rate: u32) {
    write_fixture(
        path,
        &Fixture {
            shifts: PER_MEASUREMENT,
            delay: Some((vec!["I", "R"], vec![0.25, 1.5])),
            rates: vec![f64::from(sample_rate)],
            ..Default::default()
        },
    );
}

fn max_error(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max)
}

fn render(plugin: &mut dyn Plugin, channels: usize, selected: usize) -> Vec<f32> {
    render_at(plugin, channels, selected, RATE)
}

fn render_at(plugin: &mut dyn Plugin, channels: usize, selected: usize, rate: u32) -> Vec<f32> {
    plugin.initialize(rate).unwrap();
    render_current_pattern(plugin, channels, selected, rate, &[1, 17, 127])
}

fn render_current_pattern(
    plugin: &mut dyn Plugin,
    channels: usize,
    selected: usize,
    rate: u32,
    partitions: &[usize],
) -> Vec<f32> {
    assert!(!partitions.is_empty());
    let frames = 73;
    let mut source = vec![0.0; frames * channels];
    source[selected] = 0.03125;
    source[(frames - 1) * channels + selected] = -0.015625;
    let mut output = Vec::new();
    let mut cursor = 0;
    for &size in partitions.iter().cycle() {
        let n = size.min(frames - cursor);
        if n == 0 {
            break;
        }
        let mut chunk = vec![0.0; 2 * n];
        assert_eq!(
            plugin
                .process(
                    &source[cursor * channels..(cursor + n) * channels],
                    &mut chunk,
                    &ProcessContext::new(rate, n)
                )
                .unwrap(),
            n
        );
        output.extend(chunk);
        cursor += n;
    }
    let capacity = plugin.drain_output_frames_max().max(1);
    let context = ProcessContext::new(rate, capacity);
    plugin.begin_drain(&context).unwrap();
    let mut chunk = vec![0.0; capacity * 2];
    for _ in 0..1000 {
        let result = plugin.drain(&mut chunk, &context).unwrap();
        output.extend_from_slice(&chunk[..result.frames * 2]);
        if result.complete {
            return output;
        }
    }
    panic!("finite probe did not drain");
}

fn binaural(path: &Path, selected: usize) -> Vec<f32> {
    // Three-channel auto-layout is 2.1; use the mono layout for the center HRIR.
    let (channels, source_channel) = if selected == 2 { (1, 0) } else { (2, selected) };
    let mut plugin = binaural_plugin(path, channels);
    render(&mut plugin, channels, source_channel)
}

fn binaural_plugin(path: &Path, channels: usize) -> BinauralDecoderPlugin {
    binaural_plugin_with_fft(path, channels, FFT)
}

fn binaural_plugin_with_fft(
    path: &Path,
    channels: usize,
    fft_size: usize,
) -> BinauralDecoderPlugin {
    BinauralDecoderPlugin::new(
        channels,
        fft_size,
        Some(path.to_owned()),
        0.0,
        0.0,
        false,
        120.0,
        2.0,
        0.0,
        RoomModel {
            max_order: 0,
            ..Default::default()
        },
    )
}

fn xtc(path: &Path, selected: usize) -> Vec<f32> {
    let mut plugin = xtc_plugin(path).unwrap();
    render(&mut plugin, 2, selected)
}

fn xtc_plugin(path: &Path) -> Result<XtcPlugin, String> {
    XtcPlugin::new(
        XtcPluginParams {
            fft_size: FFT,
            hrtf_file: Some(path.to_string_lossy().into_owned()),
            source_mode: "hrtf_file".to_owned(),
            auto_gain_enabled: false,
            spectral_normalization: false,
            bypass_spectral_normalization: true,
            ..Default::default()
        },
        RATE,
    )
}

fn peak_in(output: &[f32], ear: usize, start: usize, end: usize) -> usize {
    (start..end)
        .max_by(|&a, &b| {
            output[a * 2 + ear]
                .abs()
                .total_cmp(&output[b * 2 + ear].abs())
        })
        .unwrap()
}

fn independent_fractional_weights(fraction: f64) -> [f32; 65] {
    let mut weights = [0.0_f64; 65];
    let mut sum = 0.0;
    for (tap, weight) in weights.iter_mut().enumerate() {
        let distance = (tap as f64 - 32.0) - fraction;
        let x = std::f64::consts::PI * distance;
        let sinc = if x.abs() < 1.0e-8 {
            1.0 - x * x / 6.0
        } else {
            x.sin() / x
        };
        let hann = 0.5 - 0.5 * (std::f64::consts::TAU * tap as f64 / 64.0).cos();
        *weight = sinc * hann;
        sum += *weight;
    }
    std::array::from_fn(|tap| (weights[tap] / sum) as f32)
}

fn manually_materialized_fractional_irs() -> (usize, Vec<f64>) {
    let delays: [f64; 2] = [0.25, 1.5];
    let common_offset = 32;
    let expanded_length = IR + 65;
    let mut samples = vec![0.0_f32; 3 * 2 * expanded_length];
    for (measurement, (amplitudes, source_shifts)) in
        AMPLITUDES.iter().zip(PER_MEASUREMENT.iter()).enumerate()
    {
        for (ear, &delay) in delays.iter().enumerate() {
            let whole = delay.floor() as usize + common_offset;
            let prefix = whole - 32;
            let fraction = delay.fract();
            let weights = independent_fractional_weights(fraction);
            let start = (measurement * 2 + ear) * expanded_length + prefix;
            let source = source_shifts[ear];
            let amplitude = amplitudes[ear];
            for (tap, weight) in weights.iter().enumerate() {
                samples[start + source + tap] += amplitude * weight;
            }
        }
    }
    (
        expanded_length,
        samples.into_iter().map(f64::from).collect(),
    )
}

#[test]
fn binaural_integer_metadata_matches_manual_ir_shift_and_exact_onsets() {
    let dir = tempfile::tempdir().unwrap();
    let zero = [[0; 2]; 3];
    for (layout, delay) in [("I", [[0, 3]; 3]), ("M", PER_MEASUREMENT)] {
        let metadata = dir.path().join("metadata.sofa");
        let shifted = dir.path().join("shifted.sofa");
        write_file(&metadata, Some((layout, &delay)), &zero);
        write_file(&shifted, Some((layout, &zero)), &delay);
        for (selected, ears) in delay.iter().enumerate() {
            let actual = binaural(&metadata, selected);
            let expected = binaural(&shifted, selected);
            let error = max_error(&actual, &expected);
            assert!(
                error < 2e-7,
                "layout={layout} source={selected} error={error}"
            );
            for (ear, &delay) in ears.iter().enumerate() {
                assert_eq!(peak_in(&actual, ear, FFT, FFT + 16), FFT + delay);
                assert_eq!(peak_in(&actual, ear, FFT + 72, FFT + 88), FFT + 72 + delay);
            }
        }
    }
}

#[test]
fn xtc_integer_metadata_matches_manually_shifted_plant() {
    let dir = tempfile::tempdir().unwrap();
    let zero = [[0; 2]; 3];
    for (layout, delay) in [("I", [[0, 3]; 3]), ("M", PER_MEASUREMENT)] {
        let metadata = dir.path().join("metadata.sofa");
        let shifted = dir.path().join("shifted.sofa");
        write_file(&metadata, Some((layout, &delay)), &zero);
        write_file(&shifted, Some((layout, &zero)), &delay);
        for selected in 0..2 {
            let error = max_error(&xtc(&metadata, selected), &xtc(&shifted, selected));
            assert!(
                error < 2e-7,
                "layout={layout} source={selected} error={error}"
            );
        }
    }
}

fn dft(samples: &[f32], bin: usize, size: usize) -> (f64, f64) {
    samples
        .iter()
        .enumerate()
        .fold((0.0, 0.0), |(re, im), (n, &sample)| {
            let angle = -std::f64::consts::TAU * bin as f64 * n as f64 / size as f64;
            (
                re + f64::from(sample) * angle.cos(),
                im + f64::from(sample) * angle.sin(),
            )
        })
}

fn dtft(samples: &[f32], cycles_per_sample: f64) -> (f64, f64) {
    samples
        .iter()
        .enumerate()
        .fold((0.0, 0.0), |(re, im), (index, &sample)| {
            let angle = -std::f64::consts::TAU * cycles_per_sample * index as f64;
            (
                re + f64::from(sample) * angle.cos(),
                im + f64::from(sample) * angle.sin(),
            )
        })
}

#[test]
fn signed_fractional_delays_match_independent_full_response_oracle() {
    type DelayFixtureCase = (
        &'static str,
        Vec<&'static str>,
        Vec<f64>,
        [[f64; 2]; 3],
        usize,
    );

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fractional.sofa");
    let shifts = [[1, 5], [3, 7], [0, 9]];
    let cases: [DelayFixtureCase; 2] = [
        (
            "I",
            vec!["I", "R"],
            vec![-1.25, 2.5],
            [[-1.25, 2.5]; 3],
            34_usize,
        ),
        (
            "M",
            vec!["M", "R"],
            vec![-0.001, 0.999, -2.0, 1.25, 0.5, -1.5],
            [[-0.001, 0.999], [-2.0, 1.25], [0.5, -1.5]],
            34_usize,
        ),
    ];

    for (layout, dimensions, values, delays, expected_offset) in cases {
        write_fixture(
            &path,
            &Fixture {
                length: 12,
                shifts,
                delay: Some((dimensions, values)),
                ..Default::default()
            },
        );
        let loaded = load_sofa(&path).unwrap();
        assert!(loaded.delay_applied);
        assert_eq!(loaded.delay_rebase_samples, expected_offset, "{layout}");
        assert_eq!(
            loaded.delay_rebase_seconds,
            expected_offset as f64 / f64::from(RATE),
            "{layout}"
        );

        let expected_length = delays
            .iter()
            .flatten()
            .map(|&delay| {
                let integer_delay = delay.floor();
                let prefix = if delay.fract() != 0.0 {
                    (integer_delay + expected_offset as f64 - 32.0) as usize
                } else {
                    (integer_delay + expected_offset as f64) as usize
                };
                prefix + 12 + if delay.fract() != 0.0 { 64 } else { 0 }
            })
            .max()
            .unwrap();
        assert_eq!(loaded.data.ir_length, expected_length, "{layout}");

        for measurement in 0..3 {
            let ir = loaded.data.get_hrtf_slices(measurement).unwrap();
            for ear in 0..2 {
                let response = if ear == 0 { ir.1 } else { ir.2 };
                let delay = delays[measurement][ear];
                let input_position = shifts[measurement][ear];
                let amplitude = f64::from(AMPLITUDES[measurement][ear]);
                let effective_delay = input_position as f64 + delay + expected_offset as f64;

                let dc = dtft(response, 0.0);
                assert!(
                    (dc.0 - amplitude).abs() < 2.0e-7,
                    "{layout} M{measurement} E{ear} DC gain={dc:?}"
                );
                assert!(
                    dc.1.abs() < 2.0e-7,
                    "{layout} M{measurement} E{ear} DC phase={dc:?}"
                );

                let mut previous_phase = 0.0;
                for step in 1..=180 {
                    let frequency = step as f64 * 0.0025;
                    let actual = dtft(response, frequency);
                    let magnitude = actual.0.hypot(actual.1);
                    let magnitude_error_db = 20.0 * (magnitude / amplitude).log10();
                    assert!(
                        magnitude_error_db.abs() <= 0.03,
                        "{layout} M{measurement} E{ear} f={frequency}: magnitude error={magnitude_error_db} dB"
                    );

                    let raw_phase = actual.1.atan2(actual.0);
                    let unwrapped_phase = raw_phase
                        + std::f64::consts::TAU
                            * ((previous_phase - raw_phase) / std::f64::consts::TAU).round();
                    previous_phase = unwrapped_phase;
                    let expected_phase = -std::f64::consts::TAU * frequency * effective_delay;
                    let phase_delay_error = (unwrapped_phase - expected_phase).abs()
                        / (std::f64::consts::TAU * frequency);
                    assert!(
                        phase_delay_error <= 0.001,
                        "{layout} M{measurement} E{ear} f={frequency}: phase-delay error={phase_delay_error} samples"
                    );
                }

                if delay.fract() == 0.0 {
                    let exact_position = effective_delay as usize;
                    assert_eq!(response[exact_position], AMPLITUDES[measurement][ear]);
                }
            }
        }
    }
}

#[test]
fn subnormal_and_adjacent_to_integer_delays_remain_finite_and_accurate() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("boundary-fractions.sofa");
    let smallest_positive = f64::from_bits(1);
    let just_below_one = f64::from_bits(1.0_f64.to_bits() - 1);
    let cases = [
        ("subnormal", [smallest_positive, -smallest_positive]),
        ("near-one", [just_below_one, -just_below_one]),
    ];
    let shifts = [[1, 5], [3, 7], [0, 9]];

    for (label, delays) in cases {
        write_fixture(
            &path,
            &Fixture {
                length: 12,
                shifts,
                delay: Some((vec!["I", "R"], delays.to_vec())),
                ..Default::default()
            },
        );
        let loaded = load_sofa(&path).unwrap();
        assert_eq!(loaded.delay_rebase_samples, 33, "{label}");
        assert!(
            loaded
                .data
                .impulse_responses
                .iter()
                .all(|sample| sample.is_finite()),
            "{label}: materialized IR contains a non-finite coefficient"
        );

        let expected_length = delays
            .iter()
            .map(|&delay| {
                let prefix = (delay.floor() + 33.0 - 32.0) as usize;
                prefix + 12 + 64
            })
            .max()
            .unwrap();
        assert_eq!(loaded.data.ir_length, expected_length, "{label}");

        for (measurement, amplitudes) in AMPLITUDES.iter().enumerate() {
            let ir = loaded.data.get_hrtf_slices(measurement).unwrap();
            for (ear, &delay) in delays.iter().enumerate() {
                let response = if ear == 0 { ir.1 } else { ir.2 };
                let amplitude = f64::from(amplitudes[ear]);
                let effective_delay = shifts[measurement][ear] as f64
                    + delay
                    + f64::from(loaded.delay_rebase_samples as u32);
                let dc = dtft(response, 0.0);
                assert!((dc.0 - amplitude).abs() < 2.0e-7, "{label} DC={dc:?}");
                assert!(dc.1.abs() < 2.0e-7, "{label} DC phase={dc:?}");

                let mut previous_phase = 0.0;
                for step in 1..=180 {
                    let frequency = step as f64 * 0.0025;
                    let actual = dtft(response, frequency);
                    let magnitude = actual.0.hypot(actual.1);
                    let magnitude_error_db = 20.0 * (magnitude / amplitude).log10();
                    assert!(
                        magnitude_error_db.abs() <= 0.03,
                        "{label} M{measurement} E{ear} f={frequency}: magnitude error={magnitude_error_db} dB"
                    );
                    let phase = actual.1.atan2(actual.0);
                    let unwrapped_phase = phase
                        + std::f64::consts::TAU
                            * ((previous_phase - phase) / std::f64::consts::TAU).round();
                    previous_phase = unwrapped_phase;
                    let ideal_phase = -std::f64::consts::TAU * frequency * effective_delay;
                    let phase_delay_error =
                        (unwrapped_phase - ideal_phase).abs() / (std::f64::consts::TAU * frequency);
                    assert!(
                        phase_delay_error <= 0.001,
                        "{label} M{measurement} E{ear} f={frequency}: phase-delay error={phase_delay_error} samples"
                    );
                }
            }
        }
    }
}

#[test]
fn negative_integer_and_large_common_fractional_offsets_are_checked_before_expansion() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("negative.sofa");
    write_fixture(
        &path,
        &Fixture {
            delay: Some((vec!["I", "R"], vec![-2.0, 1.0])),
            ..Default::default()
        },
    );
    let loaded = load_sofa(&path).unwrap();
    assert_eq!(loaded.delay_rebase_samples, 2);
    assert_eq!(loaded.data.ir_length, IR + 3);
    for (measurement, amplitudes) in AMPLITUDES.iter().enumerate() {
        let ir = loaded.data.get_hrtf_slices(measurement).unwrap();
        assert_eq!(ir.1[0], amplitudes[0]);
        assert_eq!(ir.2[3], amplitudes[1]);
    }

    let large_common = dir.path().join("large-common.sofa");
    let large_negative = -1_000_000_000_000.0_f64;
    write_fixture(
        &large_common,
        &Fixture {
            delay: Some((vec!["I", "R"], vec![large_negative, large_negative + 0.25])),
            ..Default::default()
        },
    );
    let loaded = load_sofa(&large_common).unwrap();
    assert_eq!(loaded.delay_rebase_samples, 1_000_000_000_032);
    assert_eq!(loaded.data.ir_length, IR + 64);
    assert_eq!(loaded.data.impulse_responses.len(), 3 * 2 * (IR + 64));

    let unrepresentable = dir.path().join("unrepresentable-negative.sofa");
    write_fixture(
        &unrepresentable,
        &Fixture {
            delay: Some((vec!["I", "R"], vec![-f64::MAX, -f64::MAX])),
            ..Default::default()
        },
    );
    let error = load_sofa(&unrepresentable).unwrap_err();
    assert!(error.contains("addressable storage"), "{error}");
}

#[test]
fn spatial_consumers_report_source_time_rebase_and_keep_active_value_on_failed_reload() {
    let dir = tempfile::tempdir().unwrap();
    let binaural_path = dir.path().join("binaural-rebase.sofa");
    write_fixture(
        &binaural_path,
        &Fixture {
            rates: vec![44_100.0],
            delay: Some((vec!["I", "R"], vec![0.25, 2.5])),
            ..Default::default()
        },
    );

    let mut binaural_plugin = binaural_plugin(&binaural_path, 2);
    assert_eq!(binaural_plugin.sofa_delay_rebase_seconds(), None);
    binaural_plugin.initialize(48_000.0).unwrap();
    let expected_binaural_seconds = 32.0 / 44_100.0;
    assert!(
        (binaural_plugin.sofa_delay_rebase_seconds().unwrap() - expected_binaural_seconds).abs()
            < 1.0e-15
    );
    let replacement_binaural_path = dir.path().join("binaural-replacement.sofa");
    write_fixture(
        &replacement_binaural_path,
        &Fixture {
            delay: Some((vec!["I", "R"], vec![-2.0, 1.0])),
            ..Default::default()
        },
    );
    binaural_plugin
        .set_parameter(
            ParameterId::from("hrtf_file"),
            ParameterValue::String(replacement_binaural_path.to_string_lossy().into_owned()),
        )
        .unwrap();
    let expected_replacement_seconds = 2.0 / f64::from(RATE);
    assert_eq!(
        binaural_plugin.sofa_delay_rebase_seconds(),
        Some(expected_replacement_seconds)
    );
    write_fixture(
        &replacement_binaural_path,
        &Fixture {
            delay: Some((vec!["R"], vec![0.0; 2])),
            ..Default::default()
        },
    );
    assert!(binaural_plugin.initialize(48_000.0).is_err());
    assert_eq!(
        binaural_plugin.sofa_delay_rebase_seconds(),
        Some(expected_replacement_seconds)
    );

    let xtc_path = dir.path().join("xtc-rebase.sofa");
    write_fixture(
        &xtc_path,
        &Fixture {
            delay: Some((vec!["I", "R"], vec![-1.25, 2.5])),
            ..Default::default()
        },
    );
    let mut xtc_plugin = xtc_plugin(&xtc_path).unwrap();
    let expected_xtc_seconds = 34.0 / f64::from(RATE);
    assert_eq!(
        xtc_plugin.sofa_delay_rebase_seconds(),
        Some(expected_xtc_seconds)
    );
    write_fixture(
        &xtc_path,
        &Fixture {
            delay: Some((vec!["I", "R"], vec![-2.0, 1.0])),
            ..Default::default()
        },
    );
    xtc_plugin.initialize(RATE).unwrap();
    let expected_xtc_replacement_seconds = 2.0 / f64::from(RATE);
    assert_eq!(
        xtc_plugin.sofa_delay_rebase_seconds(),
        Some(expected_xtc_replacement_seconds)
    );
    write_fixture(
        &xtc_path,
        &Fixture {
            delay: Some((vec!["R"], vec![0.0; 2])),
            ..Default::default()
        },
    );
    assert!(xtc_plugin.initialize(RATE).is_err());
    assert_eq!(
        xtc_plugin.sofa_delay_rebase_seconds(),
        Some(expected_xtc_replacement_seconds)
    );

    // A failed runtime replacement is rejected before changing the active
    // Binaural state or its SOFA provenance.
    let missing = dir.path().join("missing.sofa");
    assert!(
        binaural_plugin
            .set_parameter(
                ParameterId::from("hrtf_file"),
                ParameterValue::String(missing.to_string_lossy().into_owned()),
            )
            .is_err()
    );
    assert_eq!(
        binaural_plugin.sofa_delay_rebase_seconds(),
        Some(expected_replacement_seconds)
    );
}

#[test]
fn fractional_support_is_counted_at_binaural_and_xtc_fft_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fractional-support.sofa");
    for (delay, should_fit) in [(49.5, true), (50.5, false)] {
        write_fixture(
            &path,
            &Fixture {
                delay: Some((vec!["I", "R"], vec![delay, delay])),
                ..Default::default()
            },
        );
        let mut plugin = binaural_plugin(&path, 2);
        let result = plugin.initialize(RATE);
        assert_eq!(
            result.is_ok(),
            should_fit,
            "Binaural delay={delay}: {result:?}"
        );
        if !should_fit {
            assert!(
                result
                    .unwrap_err()
                    .contains("linear convolution capacity 97")
            );
        }
    }

    for (delay, should_fit) in [(81.5, true), (82.5, false)] {
        write_fixture(
            &path,
            &Fixture {
                shifts: [[15, 15]; 3],
                delay: Some((vec!["I", "R"], vec![delay, delay])),
                ..Default::default()
            },
        );
        let result = xtc_plugin(&path);
        assert_eq!(result.is_ok(), should_fit, "XTC delay={delay}");
        if !should_fit {
            let error = result.err().expect("over-capacity XTC plant is rejected");
            assert!(error.contains("nonzero support beyond FFT size 128"));
        }
    }
}

#[test]
fn canonical_samples_obey_independent_dft_delay_and_exact_padding() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("delayed.sofa");
    for (layout, delays) in [("I", [[0, 3]; 3]), ("M", PER_MEASUREMENT)] {
        write_file(&path, Some((layout, &delays)), &[[0; 2]; 3]);
        let loaded = load_sofa(&path).unwrap();
        assert!(loaded.delay_applied);
        assert_eq!(loaded.delay_rebase_samples, 0);
        assert_eq!(loaded.delay_rebase_seconds, 0.0);
        assert_eq!(
            loaded.data.ir_length,
            IR + delays.iter().flatten().max().unwrap()
        );
        for (measurement, ears) in delays.iter().enumerate() {
            let ir = loaded.data.get_hrtf_slices(measurement).unwrap();
            for (ear, samples) in [ir.1, ir.2].into_iter().enumerate() {
                let delay = ears[ear];
                for (n, &sample) in samples.iter().enumerate() {
                    assert_eq!(
                        sample,
                        if n == delay {
                            AMPLITUDES[measurement][ear]
                        } else {
                            0.0
                        }
                    );
                }
                for bin in [0, 1, 7, 31, 32] {
                    let actual = dft(samples, bin, 64);
                    let phase = -std::f64::consts::TAU * bin as f64 * delay as f64 / 64.0;
                    let gain = f64::from(AMPLITUDES[measurement][ear]);
                    assert!((actual.0 - gain * phase.cos()).abs() < 1e-14);
                    assert!((actual.1 - gain * phase.sin()).abs() < 1e-14);
                }
            }
        }
    }
}

#[test]
fn absent_zero_delay_coordinates_and_rate_representations_preserve_legacy_loader() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.sofa");
    for coordinates in [None, Some("spherical"), Some("cartesian")] {
        for delay in [
            None,
            Some((vec!["I", "R"], vec![0.0, -0.0])),
            Some((vec!["M", "R"], vec![0.0; 6])),
        ] {
            for mode in 0..4 {
                let mut fixture = Fixture {
                    coordinate_type: coordinates,
                    delay: delay.clone(),
                    ..Default::default()
                };
                match mode {
                    1 => fixture.rate_dimensions.clear(),
                    2 => {
                        fixture.rate_dimensions = vec!["M"];
                        fixture.rates = vec![f64::from(RATE); 3];
                    }
                    3 => fixture.rate_attribute = true,
                    _ => {}
                }
                write_fixture(&path, &fixture);
                let old = SofaFile::load(&path).unwrap();
                let loaded = load_sofa(&path).unwrap();
                assert!(!loaded.delay_applied);
                assert_eq!(loaded.delay_rebase_samples, 0);
                assert_eq!(loaded.delay_rebase_seconds, 0.0);
                let new = loaded.data;
                assert_eq!(new.ir_length, old.ir_length);
                assert_eq!(new.impulse_responses, old.impulse_responses);
                assert_eq!(new.sample_rate, old.sample_rate);
                assert_eq!(new.data_sample_rate, old.data_sample_rate);
                assert_eq!(new.convention, old.convention);
                for (a, b) in new.positions.iter().zip(old.positions) {
                    assert_eq!(
                        (a.azimuth, a.elevation, a.distance),
                        (b.azimuth, b.elevation, b.distance)
                    );
                }
            }
        }
    }
    let absent = dir.path().join("absent.sofa");
    let zero = dir.path().join("zero.sofa");
    write_file(&absent, None, &[[0; 2]; 3]);
    write_file(&zero, Some(("M", &[[0; 2]; 3])), &[[0; 2]; 3]);
    for selected in 0..3 {
        assert_eq!(binaural(&absent, selected), binaural(&zero, selected));
    }
    for selected in 0..2 {
        assert_eq!(xtc(&absent, selected), xtc(&zero, selected));
    }
}

#[test]
fn unrepresentable_delays_and_malformed_rates_shapes_fail_before_large_allocation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("invalid.sofa");
    for (delay, expected) in [
        (f64::NAN, "finite"),
        (f64::INFINITY, "finite"),
        (usize::MAX as f64, "addressable"),
        (1_000_000_000.0, "limit is 268435456 bytes"),
    ] {
        write_fixture(
            &path,
            &Fixture {
                delay: Some((vec!["I", "R"], vec![0.0, delay])),
                ..Default::default()
            },
        );
        let error = load_sofa(&path).unwrap_err();
        assert!(error.contains(expected), "delay={delay}: {error}");
    }
    for (dimensions, values) in [
        (vec!["R", "M"], vec![0.0; 6]),
        (vec!["R"], vec![0.0; 2]),
        (vec!["I", "C"], vec![0.0; 3]),
    ] {
        write_fixture(
            &path,
            &Fixture {
                delay: Some((dimensions, values)),
                ..Default::default()
            },
        );
        assert!(load_sofa(&path).unwrap_err().contains("Data.Delay shape"));
    }
    for rates in [
        vec![48_000.0, 44_100.0, 48_000.0],
        vec![0.0; 3],
        vec![-1.0; 3],
        vec![f64::NAN; 3],
        vec![f64::INFINITY; 3],
        vec![f64::MAX; 3],
        vec![f64::MIN_POSITIVE; 3],
    ] {
        write_fixture(
            &path,
            &Fixture {
                rate_dimensions: vec!["M"],
                rates,
                ..Default::default()
            },
        );
        assert!(load_sofa(&path).unwrap_err().contains("SamplingRate"));
    }
    write_fixture(
        &path,
        &Fixture {
            rate_dimensions: vec!["I", "R"],
            rates: vec![48_000.0; 2],
            ..Default::default()
        },
    );
    assert!(load_sofa(&path).unwrap_err().contains("SamplingRate shape"));
    write_fixture(
        &path,
        &Fixture {
            ir_dimensions: vec!["R", "M", "N"],
            ..Default::default()
        },
    );
    assert!(load_sofa(&path).unwrap_err().contains("Data.IR shape"));
}

#[test]
fn xtc_rejects_only_new_selected_nonzero_support_beyond_fft() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("support.sofa");
    // A delay on the unselected center measurement only adds harmless common padding.
    write_fixture(
        &path,
        &Fixture {
            delay: Some((vec!["M", "R"], vec![0.0, 0.0, 0.0, 0.0, 140.0, 140.0])),
            ..Default::default()
        },
    );
    xtc_plugin(&path).unwrap();
    // Stored zeros after the selected shifted impulse can safely exceed the FFT.
    write_fixture(
        &path,
        &Fixture {
            length: FFT,
            delay: Some((vec!["I", "R"], vec![0.0, 3.0])),
            ..Default::default()
        },
    );
    xtc_plugin(&path).unwrap();
    // The final selected tap fits exactly at N-1, but not at N.
    for delay in [FFT - 1, FFT] {
        write_fixture(
            &path,
            &Fixture {
                delay: Some((vec!["I", "R"], vec![0.0, delay as f64])),
                ..Default::default()
            },
        );
        let result = xtc_plugin(&path);
        if delay < FFT {
            assert!(result.is_ok());
        } else {
            assert!(result.err().unwrap().contains("nonzero support beyond FFT"));
        }
    }
    // Existing raw/zero-delay truncation remains a documented compatibility limit.
    for delay in [None, Some((vec!["I", "R"], vec![0.0, 0.0]))] {
        write_fixture(
            &path,
            &Fixture {
                length: FFT + 16,
                shifts: [[FFT, FFT]; 3],
                delay,
                ..Default::default()
            },
        );
        xtc_plugin(&path).unwrap();
    }
}

fn process_dense(plugin: &mut dyn Plugin, frames: usize) -> Vec<f32> {
    let input: Vec<_> = (0..frames)
        .flat_map(|n| {
            [
                ((n * 11 % 31) as f32 - 15.0) / 1024.0,
                ((n * 7 % 29) as f32 - 14.0) / 1024.0,
            ]
        })
        .collect();
    let mut output = vec![f32::NAN; frames * 2];
    assert_eq!(
        plugin
            .process(&input, &mut output, &ProcessContext::new(RATE, frames))
            .unwrap(),
        frames
    );
    output
}

fn drain_pair(a: &mut dyn Plugin, b: &mut dyn Plugin) {
    for _ in 0..1024 {
        let mut actual = [f32::NAN; 14];
        let mut expected = [f32::NAN; 14];
        let context = ProcessContext::new(RATE, 7);
        let ar = a.drain(&mut actual, &context).unwrap();
        let br = b.drain(&mut expected, &context).unwrap();
        assert_eq!(ar, br);
        assert_eq!(actual[..ar.frames * 2], expected[..br.frames * 2]);
        assert!(actual[ar.frames * 2..].iter().all(|sample| sample.is_nan()));
        if ar.complete {
            return;
        }
    }
    panic!("drain failed to finish");
}

#[test]
fn failed_binaural_delay_replacement_or_reinitialize_preserves_live_and_partial_tail_history() {
    let dir = tempfile::tempdir().unwrap();
    let actual_path = dir.path().join("actual.sofa");
    let reference_path = dir.path().join("reference.sofa");
    let invalid_path = dir.path().join("invalid.sofa");
    let invalid = Fixture {
        delay: Some((vec!["I", "R"], vec![0.0, f64::NAN])),
        ..Default::default()
    };
    write_fixture(&invalid_path, &invalid);
    for partial in [false, true] {
        write_valid_fractional_fixture(&actual_path, RATE);
        write_valid_fractional_fixture(&reference_path, RATE);
        let mut actual = binaural_plugin_with_fft(&actual_path, 2, 256);
        let mut reference = binaural_plugin_with_fft(&reference_path, 2, 256);
        actual.initialize(RATE).unwrap();
        reference.initialize(RATE).unwrap();
        let active_offset = Some(32.0 / f64::from(RATE));
        assert_eq!(actual.sofa_delay_rebase_seconds(), active_offset);
        assert_eq!(
            process_dense(&mut actual, 193),
            process_dense(&mut reference, 193)
        );
        let settings = actual.parameters();
        let old_path = actual.get_parameter(&"hrtf_file".into());
        assert!(
            actual
                .set_parameter(
                    "hrtf_file".into(),
                    ParameterValue::String(invalid_path.to_string_lossy().into_owned())
                )
                .unwrap_err()
                .contains("Data.Delay must contain finite sample counts")
        );
        assert_eq!(actual.get_parameter(&"hrtf_file".into()), old_path);
        assert_eq!(actual.sofa_delay_rebase_seconds(), active_offset);
        if partial {
            let context = ProcessContext::new(RATE, 7);
            let mut a = [0.0; 14];
            let mut b = [0.0; 14];
            assert_eq!(
                actual.drain(&mut a, &context).unwrap(),
                reference.drain(&mut b, &context).unwrap()
            );
            assert_eq!(a, b);
        }
        // The configured pathname now resolves to invalid metadata. Validation
        // must fail before changing the old 48 kHz state, even for a rate change.
        write_fixture(&actual_path, &invalid);
        assert!(
            actual
                .initialize(96_000.0)
                .unwrap_err()
                .contains("Data.Delay must contain finite sample counts")
        );
        assert_eq!(
            format!("{:?}", actual.parameters()),
            format!("{settings:?}")
        );
        assert_eq!(actual.get_parameter(&"hrtf_file".into()), old_path);
        assert_eq!(actual.sofa_delay_rebase_seconds(), active_offset);
        if !partial {
            assert_eq!(
                process_dense(&mut actual, 257),
                process_dense(&mut reference, 257)
            );
        }
        drain_pair(&mut actual, &mut reference);
    }
}

#[test]
fn failed_xtc_delay_reinitialize_preserves_live_and_partial_tail_history() {
    let dir = tempfile::tempdir().unwrap();
    let actual_path = dir.path().join("actual.sofa");
    let reference_path = dir.path().join("reference.sofa");
    for partial in [false, true] {
        for path in [&actual_path, &reference_path] {
            write_valid_fractional_fixture(path, RATE);
        }
        let mut actual = xtc_plugin(&actual_path).unwrap();
        let mut reference = xtc_plugin(&reference_path).unwrap();
        actual.initialize(RATE).unwrap();
        reference.initialize(RATE).unwrap();
        let active_offset = Some(32.0 / f64::from(RATE));
        assert_eq!(actual.sofa_delay_rebase_seconds(), active_offset);
        assert_eq!(
            process_dense(&mut actual, 193),
            process_dense(&mut reference, 193)
        );
        if partial {
            let context = ProcessContext::new(RATE, 7);
            let mut a = [0.0; 14];
            let mut b = [0.0; 14];
            assert_eq!(
                actual.drain(&mut a, &context).unwrap(),
                reference.drain(&mut b, &context).unwrap()
            );
            assert_eq!(a, b);
        }
        write_fixture(
            &actual_path,
            &Fixture {
                delay: Some((vec!["I", "R"], vec![0.0, f64::NAN])),
                ..Default::default()
            },
        );
        assert!(
            actual
                .initialize(RATE)
                .unwrap_err()
                .contains("Data.Delay must contain finite sample counts")
        );
        assert_eq!(actual.sofa_delay_rebase_seconds(), active_offset);
        if !partial {
            assert_eq!(
                process_dense(&mut actual, 257),
                process_dense(&mut reference, 257)
            );
        }
        drain_pair(&mut actual, &mut reference);
    }
}

#[test]
fn sqlite_extensions_preserve_already_materialized_samples_without_second_delay() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.sofa");
    write_file(&source, Some(("M", &PER_MEASUREMENT)), &[[0; 2]; 3]);
    let canonical = load_sofa(&source).unwrap().data;
    for extension in ["hrtfdb", "sqlite", "db"] {
        let path = dir.path().join(format!("cache.{extension}"));
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE metadata (key TEXT, value TEXT); CREATE TABLE data (key TEXT, value BLOB);").unwrap();
        for (key, value) in [
            ("convention", canonical.convention.clone()),
            ("sample_rate", canonical.sample_rate.to_string()),
            ("data_sample_rate", canonical.sample_rate.to_string()),
            ("ir_length", canonical.ir_length.to_string()),
            ("num_measurements", canonical.num_measurements.to_string()),
        ] {
            db.execute(
                "INSERT INTO metadata VALUES (?1, ?2)",
                rusqlite::params![key, value],
            )
            .unwrap();
        }
        let positions =
            bincode::serde::encode_to_vec(&canonical.positions, bincode::config::standard())
                .unwrap();
        let samples: Vec<_> = canonical
            .impulse_responses
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        for (key, value) in [("positions", positions), ("impulse_responses", samples)] {
            db.execute(
                "INSERT INTO data VALUES (?1, ?2)",
                rusqlite::params![key, value],
            )
            .unwrap();
        }
        drop(db);
        let loaded = load_sofa(&path).unwrap();
        assert!(!loaded.delay_applied);
        assert_eq!(loaded.delay_rebase_samples, 0);
        assert_eq!(loaded.delay_rebase_seconds, 0.0);
        assert_eq!(loaded.data.ir_length, canonical.ir_length);
        assert_eq!(loaded.data.impulse_responses, canonical.impulse_responses);
        assert_eq!(loaded.data.sample_rate, canonical.sample_rate);
        assert_eq!(binaural(&path, 0), binaural(&source, 0));
        assert_eq!(xtc(&path, 1), xtc(&source, 1));
    }
}

#[test]
fn binaural_rate_conversion_preserves_physical_metadata_delay_and_last_marker() {
    let dir = tempfile::tempdir().unwrap();
    let mut worst_peak_error = 0.0_f64;
    let mut worst_waveform_error = 0.0_f32;
    for (source_rate, target_rate) in [(48_000, 96_000), (44_100, 48_000), (48_000, 44_100)] {
        for (layout, delay) in [("I", [[0, 3]; 3]), ("M", PER_MEASUREMENT)] {
            let metadata = dir.path().join("metadata.sofa");
            let shifted = dir.path().join("shifted.sofa");
            let rows = if layout == "I" { 1 } else { 3 };
            let values = delay[..rows]
                .iter()
                .flat_map(|ears| ears.map(|value| value as f64))
                .collect();
            write_fixture(
                &metadata,
                &Fixture {
                    delay: Some((vec![layout, "R"], values)),
                    rates: vec![f64::from(source_rate)],
                    ..Default::default()
                },
            );
            // Both representations must retain the same physical source window.
            // AUD114 deliberately crops output to ceil(source_length * ratio),
            // so the manually shifted twin includes the same trailing padding
            // that integer metadata materialization adds to the original IR.
            write_fixture(
                &shifted,
                &Fixture {
                    length: IR + delay.iter().flatten().max().unwrap(),
                    shifts: delay,
                    rates: vec![f64::from(source_rate)],
                    ..Default::default()
                },
            );
            for (selected, ears) in delay.iter().enumerate() {
                let (channels, source_channel) = if selected == 2 { (1, 0) } else { (2, selected) };
                let mut actual_plugin = binaural_plugin(&metadata, channels);
                let mut expected_plugin = binaural_plugin(&shifted, channels);
                let actual = render_at(&mut actual_plugin, channels, source_channel, target_rate);
                let expected =
                    render_at(&mut expected_plugin, channels, source_channel, target_rate);
                assert_eq!(actual_plugin.latency_samples(), FFT);
                let waveform_error = max_error(&actual, &expected);
                worst_waveform_error = worst_waveform_error.max(waveform_error);
                assert!(
                    waveform_error < 2e-7,
                    "{source_rate}->{target_rate}, layout={layout}, source={selected}: waveform error={waveform_error}"
                );
                for (ear, &delay) in ears.iter().enumerate() {
                    for marker in [0, 72] {
                        let peak = peak_in(&actual, ear, FFT + marker, FFT + marker + 64);
                        assert!(
                            actual[peak * 2 + ear].abs() > 0.0001,
                            "final/start marker must not disappear"
                        );
                        let physical = (FFT + marker) as f64
                            + delay as f64 * f64::from(target_rate) / f64::from(source_rate);
                        let error = (peak as f64 - physical).abs();
                        worst_peak_error = worst_peak_error.max(error);
                        assert!(
                            error <= 1.0,
                            "{source_rate}->{target_rate}, layout={layout}, source={selected}, ear={ear}, marker={marker}: peak={peak}, expected={physical}"
                        );
                    }
                }
            }
        }
    }
    eprintln!(
        "SOFA metadata + resampling: max peak error={worst_peak_error} frames, max waveform error={worst_waveform_error}"
    );
}

#[test]
fn binaural_fractional_delay_resampling_reset_and_full_eos_match_independent_ir() {
    let directory = tempfile::tempdir().unwrap();
    let delays = vec![0.25, 1.5];
    let (expanded_length, materialized_samples) = manually_materialized_fractional_irs();

    for (source_rate, target_rate) in [(44_100, 48_000), (48_000, 96_000)] {
        let metadata_path = directory
            .path()
            .join(format!("metadata-{source_rate}.sofa"));
        let manual_path = directory.path().join(format!("manual-{source_rate}.sofa"));
        write_fixture(
            &metadata_path,
            &Fixture {
                shifts: PER_MEASUREMENT,
                delay: Some((vec!["I", "R"], delays.clone())),
                rates: vec![f64::from(source_rate)],
                ..Default::default()
            },
        );
        write_fixture(
            &manual_path,
            &Fixture {
                length: expanded_length,
                ir_samples: Some(materialized_samples.clone()),
                rates: vec![f64::from(source_rate)],
                ..Default::default()
            },
        );

        let mut actual = binaural_plugin_with_fft(&metadata_path, 1, 256);
        let mut manual = binaural_plugin_with_fft(&manual_path, 1, 256);
        actual.initialize(target_rate).unwrap();
        manual.initialize(target_rate).unwrap();
        assert_eq!(
            actual.sofa_delay_rebase_seconds(),
            Some(32.0 / f64::from(source_rate))
        );

        let actual_output = render_current_pattern(&mut actual, 1, 0, target_rate, &[1, 17, 127]);
        let manual_output = render_current_pattern(&mut manual, 1, 0, target_rate, &[256]);
        assert_eq!(actual_output.len(), manual_output.len());
        let waveform_error = max_error(&actual_output, &manual_output);
        assert!(
            waveform_error < 3.0e-6,
            "{source_rate}->{target_rate}: fractional SOFA output error={waveform_error}"
        );

        actual.reset();
        manual.reset();
        let actual_after_reset = render_current_pattern(&mut actual, 1, 0, target_rate, &[73]);
        let manual_after_reset =
            render_current_pattern(&mut manual, 1, 0, target_rate, &[1, 17, 127]);
        assert_eq!(actual_after_reset.len(), manual_after_reset.len());
        assert!(
            max_error(&actual_after_reset, &manual_after_reset) < 3.0e-6,
            "{source_rate}->{target_rate}: reset/partition/EOS output differs"
        );
    }
}

#[test]
#[ignore = "manual AUD-131 post-edit audio-array capture to /tmp"]
fn capture_aud131_integer_loader_and_consumer_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("integer-reference.sofa");
    write_file(&path, Some(("M", &PER_MEASUREMENT)), &[[0; 2]; 3]);

    let loaded = load_sofa(&path).unwrap();
    assert!(loaded.delay_applied);
    let output_dir = std::path::Path::new("/tmp/sotf-aud131-postedit/arrays");
    std::fs::create_dir_all(output_dir).unwrap();
    let loader_bytes: Vec<u8> = loaded
        .data
        .impulse_responses
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect();
    std::fs::write(output_dir.join("loader.f32le"), &loader_bytes).unwrap();
    std::fs::write(
        output_dir.join("loader.txt"),
        format!(
            "measurements={} ir_length={} samples={} sample_rate={}\n",
            loaded.data.num_measurements,
            loaded.data.ir_length,
            loaded.data.impulse_responses.len(),
            loaded.data.sample_rate
        ),
    )
    .unwrap();
    println!(
        "loader: {} measurements, {} frames, {} samples",
        loaded.data.num_measurements,
        loaded.data.ir_length,
        loaded.data.impulse_responses.len()
    );

    for selected in 0..3 {
        let output = binaural(&path, selected);
        let bytes: Vec<u8> = output
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        std::fs::write(output_dir.join(format!("binaural-{selected}.f32le")), bytes).unwrap();
        println!("binaural source {selected}: {} samples", output.len());
    }
    for selected in 0..2 {
        let output = xtc(&path, selected);
        let bytes: Vec<u8> = output
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        std::fs::write(output_dir.join(format!("xtc-{selected}.f32le")), bytes).unwrap();
        println!("XTC source {selected}: {} samples", output.len());
    }
}
