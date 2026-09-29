//! Published contour checkpoints and independent loudness-matching oracles.

// Rust guideline compliant 2026-02-21
use sotf_host::auto_gain::AutoGainData;
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_plugin_loudness_compensation::iso226::{compute_iso226_delta, iso226_spl_at_freq};
use sotf_plugin_loudness_compensation::{
    LoudnessCompensationPlugin, LoudnessCompensationPluginParams,
};

#[global_allocator]
static ALLOCATOR: sotf_host::test_utils::CountingAlloc = sotf_host::test_utils::CountingAlloc;

#[test]
fn iso_2003_matches_published_spl_checkpoints() {
    // ISO 226:2003 Annex B, Table B.1 (published SPL, not production coefficients):
    // https://standards.iteh.ai/catalog/standards/iso/f15d18f8-69b0-4f46-a648-1bb26caca757/iso-226-2003
    // Columns are 20 Hz, 100 Hz, 1 kHz and 12.5 kHz. Table rounding is 0.1 dB.
    for (phon, expected) in [
        (20.0, [89.6, 48.4, 20.0, 33.0]),
        (40.0, [99.9, 64.4, 40.0, 51.5]),
        (60.0, [109.5, 78.7, 60.0, 68.6]),
        (80.0, [119.0, 92.5, 80.0, 85.4]),
    ] {
        for (index, expected_db) in [0, 7, 17, 28].into_iter().zip(expected) {
            let actual = iso226_spl_at_freq(index, phon);
            assert!(
                (actual - expected_db).abs() < 0.06,
                "{phon} phon, index {index}: expected {expected_db}, got {actual} dB SPL"
            );
        }
    }
}

#[test]
fn contour_difference_matches_published_reference_levels() {
    let delta = compute_iso226_delta(20.0, 80.0);
    // Differences of four independently rounded table values have at most
    // 0.2 dB uncertainty; 1 kHz is exactly zero by the normalized contract.
    for (index, expected) in [(0, 30.6), (7, 15.9), (28, 7.6)] {
        assert!((delta[index].1 - expected).abs() < 0.2);
    }
    assert_eq!(delta[17].1, 0.0);
}

fn plugin(sample_rate: u32, gain_db: f32, position: &str) -> LoudnessCompensationPlugin {
    let mut plugin = LoudnessCompensationPlugin::from_params(
        2,
        LoudnessCompensationPluginParams {
            low_gain: 0.0,
            high_gain: 0.0,
            mid_enabled: true,
            mid_freq: 1000.0,
            mid_gain: gain_db,
            auto_gain_enabled: true,
            auto_gain_position: position.into(),
            auto_gain_max_db: 12.0,
            auto_gain_smoothing_ms: 10.0,
            ..Default::default()
        },
    )
    .unwrap();
    plugin.initialize(sample_rate).unwrap();
    plugin
}

fn signal(sample_rate: u32, seconds: usize) -> Vec<f32> {
    (0..sample_rate as usize * seconds)
        .flat_map(|frame| {
            let level = match frame / sample_rate as usize {
                0..=1 => 0.1,
                2..=3 => 0.3,
                _ => 0.05,
            };
            let sample = (level
                * (std::f64::consts::TAU * 1000.0 * frame as f64 / f64::from(sample_rate)).sin())
                as f32;
            [sample, 0.7 * sample]
        })
        .collect()
}

fn render(
    plugin: &mut LoudnessCompensationPlugin,
    sample_rate: u32,
    input: &[f32],
    partitions: &[usize],
) -> Vec<f32> {
    let mut output = input.to_vec();
    let mut frame = 0;
    let mut block = 0;
    while frame < input.len() / 2 {
        let count = partitions[block % partitions.len()].min(input.len() / 2 - frame);
        plugin
            .process_in_place(
                &mut output[frame * 2..(frame + count) * 2],
                &ProcessContext::new(sample_rate, count),
            )
            .unwrap();
        frame += count;
        block += 1;
    }
    output
}

fn gain_db(input: &[f32], output: &[f32]) -> f64 {
    let energy = |samples: &[f32]| samples.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>();
    10.0 * (energy(output) / energy(input)).log10()
}

#[test]
fn pre_and_post_match_analytical_peak_gain_after_level_changes() {
    // A peaking EQ has exactly its requested gain at its center frequency.
    // This establishes the correction independently of filter coefficients.
    let sample_rate = 48_000;
    let input = signal(sample_rate, 6);
    for position in ["pre", "post"] {
        for eq_gain in [-6.0, 6.0] {
            let mut plugin = plugin(sample_rate, eq_gain, position);
            let output = render(&mut plugin, sample_rate, &input, &[257, 9600, 31]);
            for second in [1, 3, 5] {
                let start = (second * 48_000 + 24_000) * 2;
                let end = (second + 1) * 48_000 * 2;
                let residual = gain_db(&input[start..end], &output[start..end]);
                assert!(
                    residual.abs() < 0.03,
                    "{position}, EQ {eq_gain} dB, second {second}: residual {residual} dB"
                );
            }
            let data = plugin.get_data().unwrap();
            let data = data.downcast_ref::<AutoGainData>().unwrap();
            assert!((data.gain_db + eq_gain).abs() < 0.03);
            assert!((data.output_lufs - data.input_lufs).abs() < 0.03);
        }
    }
}

#[test]
fn gain_timeline_is_independent_of_callback_partition() {
    for sample_rate in [44_100, 48_000] {
        let input = signal(sample_rate, 3);
        for position in ["pre", "post"] {
            let reference = render(
                &mut plugin(sample_rate, 6.0, position),
                sample_rate,
                &input,
                &[64],
            );
            for partitions in [&[9600][..], &[1, 7, 2053, 31, 8192][..]] {
                let actual = render(
                    &mut plugin(sample_rate, 6.0, position),
                    sample_rate,
                    &input,
                    partitions,
                );
                let error = actual
                    .iter()
                    .zip(&reference)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0_f32, f32::max);
                assert!(error < 1e-6, "{sample_rate} Hz, {position}: {error}");
            }
        }
    }
}

#[test]
fn reset_restarts_filter_meter_and_gain_timelines() {
    let input = signal(48_000, 2);
    for position in ["pre", "post"] {
        let mut dirty = plugin(48_000, 6.0, position);
        let _ = render(&mut dirty, 48_000, &input[..36_137 * 2], &[257, 31]);
        dirty.reset();
        let actual = render(&mut dirty, 48_000, &input, &[9600]);
        let expected = render(&mut plugin(48_000, 6.0, position), 48_000, &input, &[64]);
        assert!(
            actual == expected,
            "{position}: reset changed the audio timeline"
        );
    }
}

#[test]
fn initial_meter_warmup_cannot_retroactively_change_audio() {
    let input = signal(48_000, 1);
    for position in ["pre", "post"] {
        let mut compensated = plugin(48_000, 6.0, position);
        let mut raw = plugin(48_000, 6.0, position);
        raw.set_parameter(
            ParameterId::from("auto_gain_enabled"),
            ParameterValue::Bool(false),
        )
        .unwrap();
        let actual = render(&mut compensated, 48_000, &input, &[48_000]);
        let expected = render(&mut raw, 48_000, &input, &[64]);
        // No complete 100 ms energy block exists before frame 4800.
        assert!(actual[..4800 * 2] == expected[..4800 * 2]);
    }
}

#[test]
fn cold_and_reset_gain_processing_does_not_allocate() {
    for position in ["pre", "post"] {
        let mut plugin = plugin(48_000, 6.0, position);
        let mut input = signal(48_000, 1);
        let context = ProcessContext::new(48_000, 48_000);
        sotf_host::test_utils::assert_no_allocs("cold loudness compensation", || {
            plugin.process_in_place(&mut input, &context).unwrap();
            plugin.process_in_place(&mut input, &context).unwrap();
        });
        sotf_host::test_utils::assert_no_allocs("reset loudness compensation", || {
            plugin.reset();
            plugin.process_in_place(&mut input, &context).unwrap();
        });
    }
}

#[test]
fn rejected_calibration_removal_preserves_settings_and_audio_history() {
    for position in ["disabled", "pre", "post"] {
        let make = || {
            let mut plugin = LoudnessCompensationPlugin::from_params(
                2,
                LoudnessCompensationPluginParams {
                    mode: 2,
                    auto_calibrated: true,
                    playback_volume_db: -18.0,
                    auto_gain_enabled: position != "disabled",
                    auto_gain_position: position.into(),
                    ..Default::default()
                },
            )
            .unwrap();
            plugin.initialize(48_000).unwrap();
            plugin
        };
        let mut actual = make();
        let mut untouched = make();
        let input = signal(48_000, 2);
        let split = 36_137 * 2;
        let _ = render(&mut actual, 48_000, &input[..split], &[257, 31]);
        let _ = render(&mut untouched, 48_000, &input[..split], &[257, 31]);
        let settings = actual.current_values();
        let schema_values = |plugin: &LoudnessCompensationPlugin| {
            plugin
                .parameter_schema()
                .iter()
                .map(|p| (p.id.clone(), p.default_value.clone()))
                .collect::<Vec<_>>()
        };
        let schema = schema_values(&actual);
        let error = actual
            .set_parameter(
                ParameterId::from("auto_calibrated"),
                ParameterValue::Bool(false),
            )
            .unwrap_err();
        assert!(error.contains("cannot remove SPL calibration"));
        assert_eq!(actual.current_values(), settings);
        assert_eq!(schema_values(&actual), schema);
        let actual = render(&mut actual, 48_000, &input[split..], &[1, 9600, 31]);
        let expected = render(&mut untouched, 48_000, &input[split..], &[1, 9600, 31]);
        assert!(
            actual == expected,
            "{position}: rejection changed DSP history"
        );
    }
}

#[test]
fn calibration_can_be_removed_after_leaving_auto_mode() {
    let mut plugin = plugin(48_000, 6.0, "post");
    plugin
        .set_parameter(
            ParameterId::from("auto_calibrated"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    plugin
        .set_parameter(ParameterId::from("mode"), ParameterValue::Int(2))
        .unwrap();
    plugin
        .set_parameter(ParameterId::from("mode"), ParameterValue::Int(1))
        .unwrap();
    plugin
        .set_parameter(
            ParameterId::from("auto_calibrated"),
            ParameterValue::Bool(false),
        )
        .unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("auto_calibrated")),
        Some(ParameterValue::Bool(false))
    );
}
