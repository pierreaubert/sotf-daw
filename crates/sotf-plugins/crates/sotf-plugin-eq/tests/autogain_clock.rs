//! AutoGain follows accepted audio frames and cannot use a later input suffix.
// Rust guideline compliant 2026-02-21
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_eq::{EqPlugin, EqPluginParams};

fn plugin(rate: u32, enabled: bool) -> Box<dyn Plugin> {
    let params: EqPluginParams = serde_json::from_value(serde_json::json!({
        "filters": [{"filter_type":"peak", "freq":1000.0, "q":1.0, "db_gain":9.0}],
        "auto_gain": {"enabled":enabled, "smoothing_ms":100.0, "max_gain_db":12.0}
    }))
    .unwrap();
    let mut plugin = EqPlugin::from_params(2, rate, params)
        .unwrap()
        .into_boxed_plugin();
    plugin.initialize(rate).unwrap();
    plugin
}

fn source(rate: u32, frames: usize) -> Vec<f32> {
    (0..frames)
        .flat_map(|frame| {
            let time = frame as f64 / f64::from(rate);
            let level = [0.02, 0.1, 0.04][frame / (rate as usize / 3) % 3];
            [
                (level * (std::f64::consts::TAU * 1000.0 * time).sin()) as f32,
                (level * 0.3 * (std::f64::consts::TAU * 3700.0 * time).sin()) as f32,
            ]
        })
        .collect()
}

fn render(plugin: &mut dyn Plugin, input: &[f32], rate: u32, block: usize) -> Vec<f32> {
    let mut output = vec![f32::NAN; input.len()];
    for (source, destination) in input.chunks(2 * block).zip(output.chunks_mut(2 * block)) {
        let frames = source.len() / 2;
        assert_eq!(
            plugin
                .process(source, destination, &ProcessContext::new(rate, frames))
                .unwrap(),
            frames
        );
    }
    assert!(output.iter().all(|sample| sample.is_finite()));
    output
}

fn assert_exact(actual: &[f32], expected: &[f32], context: &str) {
    assert_eq!(actual.len(), expected.len());
    if let Some(index) = actual
        .iter()
        .zip(expected)
        .position(|(a, b)| a.to_bits() != b.to_bits())
    {
        let maximum = actual
            .iter()
            .zip(expected)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        panic!(
            "{context}: first different sample {index}, actual {}, expected {}, maximum {maximum}",
            actual[index], expected[index]
        );
    }
}

#[test]
fn native_autogain_is_exact_across_callback_partitions() {
    for rate in [48_000, 96_000] {
        let input = source(rate, 6 * rate as usize + 17);
        for enabled in [false, true] {
            let expected = render(plugin(rate, enabled).as_mut(), &input, rate, 137);
            for block in [512, 8192] {
                let actual = render(plugin(rate, enabled).as_mut(), &input, rate, block);
                assert_exact(
                    &actual,
                    &expected,
                    &format!("rate{rate}/enabled{enabled}/block{block}"),
                );
            }
        }
    }
}

#[test]
fn later_suffix_cannot_change_the_identical_output_prefix() {
    for rate in [48_000, 96_000] {
        // Preserve the original public red: this puts the old callback counter
        // one call before publication, independent of the proposed fixed clock.
        let history = source(rate, 109 * 4096);
        let first = source(rate, 8192);
        let mut different = first.clone();
        for frame in different[8192..].as_chunks_mut::<2>().0 {
            frame[0] *= 0.1;
            frame[1] *= 10.0;
        }
        for enabled in [false, true] {
            let mut a = plugin(rate, enabled);
            let mut b = plugin(rate, enabled);
            assert_exact(
                &render(a.as_mut(), &history, rate, 4096),
                &render(b.as_mut(), &history, rate, 4096),
                "identical history",
            );
            let actual = render(a.as_mut(), &first, rate, 8192);
            let expected = render(b.as_mut(), &different, rate, 8192);
            assert_exact(
                &actual[..8192],
                &expected[..8192],
                &format!("rate{rate}/enabled{enabled} prefix"),
            );
        }
    }
}

fn configured(rate: u32, factor: i32, gain_db: Option<f64>, enabled: bool) -> Box<dyn Plugin> {
    let filters = gain_db.map_or_else(Vec::new, |gain| {
        vec![serde_json::json!({"filter_type":"peak", "freq":1000.0, "q":1.0, "db_gain":gain})]
    });
    let params = serde_json::from_value(serde_json::json!({
        "filters":filters,
        "auto_gain":{"enabled":enabled,"smoothing_ms":100.0,"max_gain_db":12.0}
    }))
    .unwrap();
    let mut plugin = EqPlugin::from_params(2, rate, params)
        .unwrap()
        .into_boxed_plugin();
    plugin
        .set_parameter(
            sotf_host::ParameterId::from("oversampling"),
            sotf_host::ParameterValue::Int(factor),
        )
        .unwrap();
    plugin.initialize(rate).unwrap();
    plugin
}

fn meter(plugin: &dyn Plugin) -> sotf_host::auto_gain::AutoGainData {
    plugin
        .get_data()
        .unwrap()
        .downcast::<sotf_host::auto_gain::AutoGainData>()
        .unwrap()
        .as_ref()
        .clone()
}

/// A separate public meter/gain instance, with an explicit time-shifted input
/// array and scalar frame recurrence. It never calls the EQ clock helper.
fn clock_oracle(rate: u32, input: &[f32], raw: &[f32], delay: usize) -> Vec<f32> {
    use sotf_host::auto_gain::{AutoGain, AutoGainParams};
    let mut reference = vec![0.0; input.len()];
    if delay * 2 < input.len() {
        reference[delay * 2..].copy_from_slice(&input[..input.len() - delay * 2]);
    }
    let mut gain = AutoGain::new(
        2,
        rate,
        AutoGainParams {
            enabled: true,
            max_gain_db: 12.0,
            smoothing_ms: 100.0,
            ..AutoGainParams::default()
        },
    )
    .unwrap();
    let mut result = raw.to_vec();
    let interval_samples = (rate as usize / 10).max(1) * 2;
    for (reference, output) in reference
        .chunks(interval_samples)
        .zip(result.chunks_mut(interval_samples))
    {
        gain.ingest_input(reference).unwrap();
        gain.ingest_output(output).unwrap();
        for frame in output.as_chunks_mut::<2>().0 {
            let scalar = gain.next_gain_linear();
            frame.iter_mut().for_each(|sample| *sample *= scalar);
        }
        if output.len() == interval_samples {
            gain.refresh_input_measurement();
            gain.refresh_output_measurement();
        }
    }
    result
}

#[test]
fn aligned_base_rate_meter_matches_independent_scalar_clock() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        let input = source(rate, 2 * rate as usize + 113);
        for factor in [1, 2, 4] {
            for gain_db in [-9.0, 9.0] {
                let mut raw_plugin = configured(rate, factor, Some(gain_db), false);
                let delay = raw_plugin.latency_samples();
                let raw = render(raw_plugin.as_mut(), &input, rate, 137);
                let expected = clock_oracle(rate, &input, &raw, delay);
                // The gain target is causal: no compensation before the first
                // complete 100 ms interval, including oversampler startup.
                assert_exact(
                    &expected[..rate as usize / 10 * 2],
                    &raw[..rate as usize / 10 * 2],
                    "first interval",
                );
                assert!(
                    expected
                        .iter()
                        .zip(&raw)
                        .any(|(a, b)| (a - b).abs() > 0.001)
                );
                for block in [17, 4096] {
                    let mut candidate = configured(rate, factor, Some(gain_db), true);
                    let actual = render(candidate.as_mut(), &input, rate, block);
                    assert_exact(
                        &actual,
                        &expected,
                        &format!("oracle rate{rate}/factor{factor}/gain{gain_db}/block{block}"),
                    );
                }
            }
        }
    }
}

#[test]
fn oversampled_reference_delay_equals_the_independent_neutral_impulse_peak() {
    for rate in [44_100, 48_000, 192_000] {
        for factor in [1, 2, 4] {
            let mut plugin = configured(rate, factor, None, false);
            let mut input = vec![0.0; 4096 * 2];
            input[0] = 0.5;
            input[1] = -0.25;
            let output = render(plugin.as_mut(), &input, rate, 137);
            let peak = output
                .as_chunks::<2>()
                .0
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a[0].abs().total_cmp(&b[0].abs()))
                .unwrap()
                .0;
            assert_eq!(peak, plugin.latency_samples(), "rate{rate}/factor{factor}");
        }
    }
}

#[test]
fn actual_compiled_host_and_fallback_share_the_same_clock() {
    use sotf_host::{DawHost, ParameterValue};
    for compiled in [false, true] {
        for tdf2 in [false, true] {
            let rate = 48_000;
            let mut direct = configured(rate, 1, Some(9.0), true);
            let mut hosted = configured(rate, 1, Some(9.0), true);
            for plugin in [&mut direct, &mut hosted] {
                plugin
                    .set_parameter(
                        sotf_host::ParameterId::from("tdf2"),
                        ParameterValue::Bool(tdf2),
                    )
                    .unwrap();
            }
            let mut host = DawHost::new(2, rate);
            host.set_compiled_linear_enabled(compiled);
            host.add_plugin(hosted).unwrap();
            host.build().unwrap();
            let input = source(rate, rate as usize);
            for epoch in 0..3 {
                if epoch == 1 {
                    direct
                        .set_parameter(
                            sotf_host::ParameterId::from("band_0_gain"),
                            ParameterValue::Float(-9.0),
                        )
                        .unwrap();
                    host.set_plugin_parameter_immediate(
                        0,
                        "band_0_gain",
                        ParameterValue::Float(-9.0),
                    )
                    .unwrap();
                }
                let expected = render(direct.as_mut(), &input, rate, 512);
                let mut actual = vec![0.0; input.len()];
                for (input, output) in input.chunks(1024).zip(actual.chunks_mut(1024)) {
                    assert_eq!(host.process(input, output).unwrap(), input.len() / 2);
                }
                assert_exact(
                    &actual,
                    &expected,
                    &format!("compiled{compiled}/tdf2{tdf2}/epoch{epoch}"),
                );
            }
        }
    }
}

fn drain(plugin: &mut dyn Plugin, rate: u32, capacity: usize) -> Vec<f32> {
    let mut result = Vec::new();
    for _ in 0..2048 {
        let mut buffer = vec![123.0; capacity * 2 + 3];
        let progress = plugin
            .drain(&mut buffer[..capacity * 2], &ProcessContext::new(rate, 0))
            .unwrap();
        assert!(
            buffer[progress.frames * 2..]
                .iter()
                .all(|&sample| sample == 123.0)
        );
        result.extend_from_slice(&buffer[..progress.frames * 2]);
        if progress.complete {
            return result;
        }
    }
    panic!("finite EQ did not complete");
}

#[test]
fn warm_finite_eos_advances_clock_only_when_canonical_audio_is_generated() {
    for rate in [44_100, 48_000, 192_000] {
        for factor in [2, 4] {
            // A near-Nyquist input produces real resampler loss, so the empty
            // finite bank has a nontrivial learned compensation target.
            let interval = rate as usize / 10;
            for phase in [1, 255] {
                let frames = interval * 12 - phase;
                let input: Vec<_> = (0..frames)
                    .flat_map(|frame| {
                        let phase = std::f64::consts::TAU * frame as f64;
                        // A passband component keeps both loudness meters
                        // above their gate while the high component is removed.
                        let x = (0.08 * (0.49 * phase).sin() + 0.02 * (0.1 * phase).sin()) as f32;
                        [x, -0.5 * x]
                    })
                    .collect();
                let mut reference = configured(rate, factor, None, true);
                render(reference.as_mut(), &input, rate, 137);
                let expected = render(reference.as_mut(), &[0.0; 2048], rate, 256);
                let reference_data = meter(reference.as_ref());
                assert!(
                    reference_data.gain_db > 0.01,
                    "active compensation required: rate{rate}/factor{factor}/phase{phase}: {} dB",
                    reference_data.gain_db
                );
                for capacity in [1, 17, 256, 1024] {
                    let mut candidate = configured(rate, factor, None, true);
                    render(candidate.as_mut(), &input, rate, 137);
                    assert_exact(
                        &drain(candidate.as_mut(), rate, capacity),
                        &expected,
                        "finite EOS",
                    );
                    let actual_data = meter(candidate.as_ref());
                    assert_eq!(
                        actual_data.gain_db.to_bits(),
                        reference_data.gain_db.to_bits()
                    );
                    assert_eq!(
                        actual_data.input_lufs.to_bits(),
                        reference_data.input_lufs.to_bits()
                    );
                    assert_eq!(
                        actual_data.output_lufs.to_bits(),
                        reference_data.output_lufs.to_bits()
                    );
                    candidate.initialize(rate).unwrap();
                    let restarted = meter(candidate.as_ref());
                    assert_eq!(restarted.gain_db, 0.0);
                    assert_eq!(restarted.input_peak, 0.0);
                    assert_eq!(restarted.input_lufs, f64::NEG_INFINITY);
                    let fresh = render(
                        configured(rate, factor, None, true).as_mut(),
                        &input[..4096],
                        rate,
                        137,
                    );
                    assert_exact(
                        &render(candidate.as_mut(), &input[..4096], rate, 137),
                        &fresh,
                        "post-EOS initialization",
                    );
                }
            }
        }
    }
}

#[test]
fn structural_measurement_epochs_clear_diagnostics_without_resetting_gain() {
    use sotf_host::{ParameterId, ParameterValue};
    for (from, to) in [(1, 2), (2, 4), (4, 1), (2, 2)] {
        let rate = 48_000;
        let mut candidate = configured(rate, from, Some(9.0), true);
        let history = source(rate, rate as usize * 2);
        render(candidate.as_mut(), &history, rate, 137);
        let before = meter(candidate.as_ref());
        assert!(before.gain_db < -1.0 && before.input_lufs.is_finite());
        candidate
            .set_parameter(ParameterId::from("oversampling"), ParameterValue::Int(to))
            .unwrap();
        let after = meter(candidate.as_ref());
        assert_eq!(after.gain_db.to_bits(), before.gain_db.to_bits());
        assert_eq!(
            (after.input_lufs, after.output_lufs),
            (f64::NEG_INFINITY, f64::NEG_INFINITY)
        );
        assert_eq!((after.input_peak, after.output_peak), (0.0, 0.0));
        // Successful reinitialization starts another aligned meter epoch, but
        // retains the original gain-state contract rather than jumping to 1.
        candidate.initialize(96_000).unwrap();
        assert_eq!(
            meter(candidate.as_ref()).gain_db.to_bits(),
            before.gain_db.to_bits()
        );
        let first_interval = source(96_000, 9600);
        render(candidate.as_mut(), &first_interval[..19198], 96_000, 137);
        assert_eq!(meter(candidate.as_ref()).input_peak, 0.0);
        render(candidate.as_mut(), &first_interval[19198..], 96_000, 1);
        assert!(meter(candidate.as_ref()).input_peak > 0.0);
        candidate.reset();
        let signal = source(96_000, 96_017);
        let actual = render(candidate.as_mut(), &signal, 96_000, 137);
        let expected = render(
            configured(96_000, to, Some(9.0), true).as_mut(),
            &signal,
            96_000,
            137,
        );
        assert_exact(&actual, &expected, "reset fresh replay");
    }
}

#[test]
fn disabled_diagnostics_and_rejected_or_empty_calls_preserve_the_frame_clock() {
    use sotf_host::{ParameterId, ParameterValue};
    for factor in [1, 2, 4] {
        let rate = 48_000;
        let mut candidate = configured(rate, factor, Some(9.0), false);
        let mut twin = configured(rate, factor, Some(9.0), false);
        let history = source(rate, 48_000);
        assert_exact(
            &render(candidate.as_mut(), &history, rate, 137),
            &render(twin.as_mut(), &history, rate, 137),
            "disabled history",
        );
        let data = meter(candidate.as_ref());
        assert!(!data.enabled && data.input_lufs.is_finite() && data.output_lufs.is_finite());
        assert_eq!(data.gain_db, 0.0);
        let mut sentinel = [123.0; 4];
        assert_eq!(
            candidate
                .process(&[], &mut sentinel, &ProcessContext::new(rate, 0))
                .unwrap(),
            0
        );
        assert!(candidate.initialize(0).is_err());
        assert!(
            candidate
                .set_parameter(ParameterId::from("oversampling"), ParameterValue::Int(3))
                .is_err()
        );
        assert!(
            candidate
                .process(&[], &mut sentinel, &ProcessContext::new(rate, 1))
                .is_err()
        );
        assert_eq!(sentinel, [123.0; 4]);
        if factor > 1 {
            let oversized = vec![0.0; 8194];
            let mut output = oversized.clone();
            assert!(
                candidate
                    .process(&oversized, &mut output, &ProcessContext::new(rate, 4097))
                    .is_err()
            );
        }
        for plugin in [&mut candidate, &mut twin] {
            plugin
                .set_parameter(
                    ParameterId::from("auto_gain_enabled"),
                    ParameterValue::Bool(true),
                )
                .unwrap();
        }
        let continuation = source(rate, 48_117);
        assert_exact(
            &render(candidate.as_mut(), &continuation, rate, 137),
            &render(twin.as_mut(), &continuation, rate, 137),
            "retained clock after invalid calls",
        );
    }
}

#[test]
fn published_peaks_cover_the_entire_aligned_measurement_interval() {
    for enabled in [false, true] {
        for factor in [1, 2, 4] {
            let rate = 48_000;
            let frames = rate as usize / 10;
            let delay = configured(rate, factor, Some(9.0), false).latency_samples();
            for marker in [0, frames - delay - 100] {
                let mut input = vec![0.001; frames * 4];
                input[marker * 2] = 0.9;
                input[marker * 2 + 1] = -0.4;
                let mut raw_plugin = configured(rate, factor, Some(9.0), false);
                let raw = render(raw_plugin.as_mut(), &input, rate, 137);
                let mut aligned = vec![0.0; input.len()];
                aligned[delay * 2..].copy_from_slice(&input[..input.len() - delay * 2]);
                for block in [137, 4096] {
                    let mut candidate = configured(rate, factor, Some(9.0), enabled);
                    let mut retained = None;
                    for interval in 0..2 {
                        let span = interval * frames * 2..(interval + 1) * frames * 2;
                        render(candidate.as_mut(), &input[span.clone()], rate, block);
                        let expected_input = aligned[span.clone()]
                            .iter()
                            .map(|x| f64::from(x.abs()))
                            .fold(0.0, f64::max);
                        let expected_output = raw[span]
                            .iter()
                            .map(|x| f64::from(x.abs()))
                            .fold(0.0, f64::max);
                        let data = meter(candidate.as_ref());
                        assert_eq!(
                            data.input_peak, expected_input,
                            "input enabled{enabled}/factor{factor}/block{block}/marker{marker}/interval{interval}"
                        );
                        assert_eq!(
                            data.output_peak, expected_output,
                            "output enabled{enabled}/factor{factor}/block{block}/marker{marker}/interval{interval}"
                        );
                        if interval == 0 {
                            retained = candidate.get_data();
                        }
                    }
                    let retained = retained
                        .unwrap()
                        .downcast::<sotf_host::auto_gain::AutoGainData>()
                        .unwrap();
                    assert_eq!(retained.input_peak, f64::from(0.9_f32));
                    assert_eq!(meter(candidate.as_ref()).input_peak, f64::from(0.001_f32));
                }
            }
        }
    }
}

#[test]
fn host_prepared_drain_matches_the_same_native_clock_and_finite_suffix() {
    use sotf_host::DawHost;
    for factor in [1, 2, 4] {
        let rate = 48_000;
        let mut direct = configured(rate, factor, None, true);
        let mut host = DawHost::new(2, rate);
        host.set_compiled_linear_enabled(true);
        host.add_plugin(configured(rate, factor, None, true))
            .unwrap();
        host.build().unwrap();
        let input = source(rate, 48_000 - 1);
        for input in input.chunks(514) {
            let expected = render(direct.as_mut(), input, rate, 257);
            let mut actual = vec![123.0; input.len()];
            assert_eq!(host.process(input, &mut actual).unwrap(), input.len() / 2);
            assert_exact(&actual, &expected, "host finite prefix");
        }
        let expected = drain(direct.as_mut(), rate, 17);
        let capacity = host.drain_output_frames_max();
        let mut actual = Vec::new();
        for _ in 0..16 {
            let mut buffer = vec![123.0; capacity * 2 + 3];
            let result = host.drain(&mut buffer[..capacity * 2]).unwrap();
            assert!(buffer[result.frames * 2..].iter().all(|x| *x == 123.0));
            actual.extend_from_slice(&buffer[..result.frames * 2]);
            if result.complete {
                break;
            }
        }
        assert_exact(&actual, &expected, "host finite suffix");
        assert_eq!(actual.len(), if factor == 1 { 0 } else { 2048 });
    }
}
