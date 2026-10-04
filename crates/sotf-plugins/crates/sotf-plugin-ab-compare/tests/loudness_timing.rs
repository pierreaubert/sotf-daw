//! Independent amplitude and streaming-clock oracles for loudness matching.

use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_plugin_ab_compare::{
    ABCompareData, ABComparePlugin, ABComparePluginParams, LoudnessType, PathConfig,
};

#[global_allocator]
static ALLOCATOR: sotf_host::test_utils::CountingAlloc = sotf_host::test_utils::CountingAlloc;

fn make_plugin(sample_rate: u32, gain_db: f32, loudness_type: LoudnessType) -> ABComparePlugin {
    let mut plugin = ABComparePlugin::from_params(
        2,
        ABComparePluginParams {
            path_b: PathConfig::Plugin {
                plugin_type: "gain".into(),
                parameters: serde_json::json!({"gain_db": gain_db}),
            },
            mix: 1.0,
            gain_smoothing_ms: 10.0,
            loudness_type,
            ..Default::default()
        },
    )
    .unwrap();
    plugin.initialize(f64::from(sample_rate)).unwrap();
    plugin
}

fn signal(sample_rate: u32, seconds: usize) -> Vec<f32> {
    (0..sample_rate as usize * seconds)
        .flat_map(|frame| {
            // Level changes exercise meter history without changing the known
            // relationship between the two linear paths.
            let level = if frame < sample_rate as usize {
                0.1
            } else {
                0.2
            };
            let sample = ((std::f64::consts::TAU * 997.0 * frame as f64 / f64::from(sample_rate))
                .sin()
                * level) as f32;
            [sample, sample * 0.7]
        })
        .collect()
}

fn render(
    plugin: &mut ABComparePlugin,
    sample_rate: u32,
    input: &[f32],
    partitions: &[usize],
) -> Vec<f32> {
    let mut output = vec![0.0; input.len()];
    let mut frame = 0;
    let mut block = 0;
    while frame < input.len() / 2 {
        let count = partitions[block % partitions.len()].min(input.len() / 2 - frame);
        plugin
            .process(
                &input[frame * 2..(frame + count) * 2],
                &mut output[frame * 2..(frame + count) * 2],
                &ProcessContext::new(sample_rate, count),
            )
            .unwrap();
        frame += count;
        block += 1;
    }
    output
}

#[test]
fn correction_magnitude_matches_inverse_path_gain() {
    for loudness_type in [LoudnessType::Momentary, LoudnessType::ShortTerm] {
        for gain_db in [-6.0, 6.0] {
            let input = signal(48_000, 4);
            let mut plugin = make_plugin(48_000, gain_db, loudness_type);
            let output = render(&mut plugin, 48_000, &input, &[257, 9600, 31]);
            let start = 3 * 48_000 * 2;
            let input_energy: f64 = input[start..].iter().map(|&x| f64::from(x).powi(2)).sum();
            let output_energy: f64 = output[start..].iter().map(|&x| f64::from(x).powi(2)).sum();
            let corrected_gain_db = 10.0 * (output_energy / input_energy).log10();
            assert!(
                corrected_gain_db.abs() < 0.02,
                "path {gain_db} dB, {loudness_type:?}: output mismatch {corrected_gain_db} dB"
            );
            let data = plugin.get_data().unwrap();
            let data = data.downcast_ref::<ABCompareData>().unwrap();
            assert!((data.auto_gain_db + gain_db).abs() < 0.02);
        }
    }
}

#[test]
fn correction_timeline_is_independent_of_callback_partition() {
    for sample_rate in [44_100, 48_000] {
        for gain_db in [-6.0, 6.0] {
            let input = signal(sample_rate, 2);
            let reference = render(
                &mut make_plugin(sample_rate, gain_db, LoudnessType::Momentary),
                sample_rate,
                &input,
                &[64],
            );
            for partitions in [&[4096][..], &[9600][..], &[1, 7, 2053, 31, 8192][..]] {
                let output = render(
                    &mut make_plugin(sample_rate, gain_db, LoudnessType::Momentary),
                    sample_rate,
                    &input,
                    partitions,
                );
                let max_error = output
                    .iter()
                    .zip(&reference)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0_f32, f32::max);
                assert!(
                    max_error < 1e-6,
                    "{sample_rate} Hz, path {gain_db} dB, blocks {partitions:?}: max output error {max_error}"
                );
            }
        }
    }
}

#[test]
fn measurement_cannot_change_samples_that_precede_its_boundary() {
    let sample_rate = 48_000;
    let input = signal(sample_rate, 1);
    let output = render(
        &mut make_plugin(sample_rate, 6.0, LoudnessType::Momentary),
        sample_rate,
        &input,
        &[48_000],
    );
    // The loudness meter first completes a 100 ms energy block. A callback
    // containing later measurements must not retroactively change that audio.
    let initial_samples = sample_rate as usize / 10 * 2;
    let raw_gain = 10.0_f64.powf(6.0 / 20.0);
    for (&actual, &dry) in output[..initial_samples].iter().zip(&input) {
        assert!(
            (f64::from(actual) - f64::from(dry) * raw_gain).abs() < 1e-6,
            "future measurement changed initial sample: actual {actual}, dry {dry}"
        );
    }
}

#[test]
fn reset_restarts_the_measurement_clock_at_the_first_sample() {
    let sample_rate = 48_000;
    let input = signal(sample_rate, 2);
    let mut plugin = make_plugin(sample_rate, 6.0, LoudnessType::Momentary);
    // Stop partway through a measurement interval after the gain has changed.
    let _ = render(&mut plugin, sample_rate, &input[..36_137 * 2], &[257, 31]);
    plugin.reset();
    let actual = render(&mut plugin, sample_rate, &input, &[9600]);
    let expected = render(
        &mut make_plugin(sample_rate, 6.0, LoudnessType::Momentary),
        sample_rate,
        &input,
        &[64],
    );
    assert_eq!(actual, expected);
    assert_eq!(
        plugin.latency_samples(),
        0,
        "meter warmup must not add audio latency"
    );
}

#[test]
fn cold_processing_across_measurement_boundaries_does_not_allocate() {
    for nested in [false, true] {
        let mut plugin = if nested {
            make_plugin(48_000, 6.0, LoudnessType::Momentary)
        } else {
            let mut plugin = ABComparePlugin::new(2).unwrap();
            plugin.initialize(48_000.0).unwrap();
            plugin
        };
        let input = signal(48_000, 1);
        let mut output = vec![0.0; input.len()];
        let context = ProcessContext::new(48_000, 48_000);
        // The first callback spans twenty updates, including the first valid
        // loudness target. The second includes established meter history.
        sotf_host::test_utils::assert_no_allocs("cold sample-clock loudness matching", || {
            plugin.process(&input, &mut output, &context).unwrap();
            plugin.process(&input, &mut output, &context).unwrap();
        });
        assert!(output.iter().all(|sample| sample.is_finite()));
        plugin.reset();
        sotf_host::test_utils::assert_no_allocs("first loudness callback after reset", || {
            plugin.process(&input, &mut output, &context).unwrap();
        });
    }
}
