//! Independent step-response and lifecycle oracles for transient shaping.

// Rust guideline compliant 2026-02-21
use sotf_host::parametric_plugin::ParameterSet;
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_transient_shaper::{
    TransientShaperData, TransientShaperPlugin, TransientShaperPluginParams,
};

fn prepared(rate: u32) -> TransientShaperPlugin {
    let mut plugin = TransientShaperPlugin::from_params(
        2,
        TransientShaperPluginParams {
            attack: 80.0,
            sustain: -40.0,
            sensitivity_db: -12.0,
            ..Default::default()
        },
    )
    .unwrap();
    plugin.initialize(f64::from(rate)).unwrap();
    plugin
}

fn render(
    plugin: &mut TransientShaperPlugin,
    rate: u32,
    input: &[f32],
    pattern: &[usize],
) -> Vec<f32> {
    let mut output = input.to_vec();
    let mut position = 0;
    for &requested in pattern.iter().cycle() {
        let frames = requested.min(input.len() / 2 - position);
        if frames == 0 {
            break;
        }
        plugin
            .process_in_place(
                &mut output[position * 2..(position + frames) * 2],
                &ProcessContext::new(rate, frames),
            )
            .unwrap();
        position += frames;
    }
    output
}

#[test]
fn quiet_start_first_hit_and_percussion_match_closed_form_step_responses() {
    for rate in [44_100, 48_000, 96_000] {
        let mut segments = vec![
            (1e-5_f32, rate as usize / 100),
            (0.0, rate as usize / 300),
            (0.2, 1),
            (0.0, rate as usize / 100),
        ];
        for _ in 0..8 {
            segments.extend([(0.1, rate as usize / 1000), (0.0, rate as usize / 500)]);
        }
        segments.push((0.1, rate as usize / 10));
        let input: Vec<f32> = segments
            .iter()
            .flat_map(|&(value, frames)| (0..frames).flat_map(move |_| [value, -value * 0.25]))
            .collect();
        let output = render(&mut prepared(rate), rate, &input, &[1]);
        let mut position = 0;
        let mut fast = 0.0_f64;
        let mut slow = 0.0_f64;
        let mut maximum_error = 0.0_f64;
        for &(value, frames) in &segments {
            let target = f64::from(value);
            let fast_tau = if target > fast { 0.0001 } else { 0.005 };
            let slow_tau = if target > slow { 0.010 } else { 0.100 };
            let fast_start = fast;
            let slow_start = slow;
            for sample in 1..=frames {
                // Closed-form continuous-time step responses sampled at Fs;
                // no production coefficients or envelope helpers are reused.
                fast = target
                    + (fast_start - target)
                        * (-(sample as f64) / (fast_tau * f64::from(rate))).exp();
                slow = target
                    + (slow_start - target)
                        * (-(sample as f64) / (slow_tau * f64::from(rate))).exp();
                let gain = if slow > 10.0_f64.powf(-12.0 / 20.0) * 0.001 {
                    let attack = ((fast - slow).max(0.0) / slow.max(1e-10)).clamp(0.0, 1.0);
                    let sustain = (slow / fast.max(1e-10)).clamp(0.0, 1.0);
                    (1.0 + 0.8 * attack - 0.4 * sustain).clamp(0.25, 4.0)
                } else {
                    1.0
                };
                let expected = target * gain;
                maximum_error =
                    maximum_error.max((f64::from(output[position * 2]) - expected).abs());
                assert!(
                    (output[position * 2 + 1] + output[position * 2] * 0.25).abs() < 1e-7,
                    "linked image changed"
                );
                position += 1;
            }
        }
        assert!(
            maximum_error < 2e-5,
            "rate={rate}: step response error={maximum_error}"
        );
        assert_eq!(
            output[0], 1e-5,
            "quiet starts below sensitivity must remain unchanged"
        );
        for pattern in [&[8192][..], &[7, 31, 257][..]] {
            assert_eq!(render(&mut prepared(rate), rate, &input, pattern), output);
        }
    }
}

#[test]
fn mismatched_rate_rejection_preserves_audio_and_envelope_history() {
    let mut tested = prepared(48_000);
    let mut reference = prepared(48_000);
    let history = vec![0.1; 2048];
    render(&mut tested, 48_000, &history, &[257]);
    render(&mut reference, 48_000, &history, &[257]);
    let mut rejected = vec![0.2; 128];
    let original = rejected.clone();
    assert!(
        tested
            .process_in_place(&mut rejected, &ProcessContext::new(96_000, 64))
            .is_err()
    );
    assert_eq!(rejected, original);
    let input = vec![0.02; 2048];
    assert_eq!(
        render(&mut tested, 48_000, &input, &[257]),
        render(&mut reference, 48_000, &input, &[257])
    );
}

#[test]
fn malformed_bulk_controls_are_rejected_before_any_mutation() {
    let mut plugin = prepared(48_000);
    let before = plugin.current_values();
    let mut values = ParameterSet::new();
    values.insert(ParameterId::from("attack"), ParameterValue::Float(25.0));
    values.insert(ParameterId::from("mix"), ParameterValue::Float(f32::NAN));
    assert!(plugin.apply_values(values).is_err());
    assert_eq!(plugin.current_values(), before);
}

#[test]
fn reset_immediately_clears_published_history_and_preserves_held_snapshots() {
    let mut plugin = prepared(48_000);
    render(&mut plugin, 48_000, &vec![0.1; 4800], &[257]);
    let held = plugin.get_data().unwrap();
    let previous = held
        .downcast_ref::<TransientShaperData>()
        .unwrap()
        .sustain_level;
    assert!(previous > 0.01);
    plugin.reset();
    let reset = plugin.get_data().unwrap();
    let reset = reset.downcast_ref::<TransientShaperData>().unwrap();
    assert_eq!(reset.transient_level, 0.0);
    assert_eq!(reset.sustain_level, 0.0);
    assert_eq!(reset.gain, 1.0);
    assert_eq!(
        held.downcast_ref::<TransientShaperData>()
            .unwrap()
            .sustain_level,
        previous
    );
}

#[test]
fn reinitialize_preserves_audio_envelopes_while_reset_clears_them() {
    let mut plugin = prepared(48_000);
    render(&mut plugin, 48_000, &vec![0.1; 480 * 2], &[257]);
    plugin.initialize(96_000.0).unwrap();
    let actual = render(&mut plugin, 96_000, &[0.1, -0.025], &[1]);
    // The original 10 ms history continues for one sample at the new rate.
    let elapsed = 0.010 + 1.0 / 96_000.0;
    let fast = 0.1 * (1.0 - (-elapsed / 0.0001_f64).exp());
    let slow = 0.1 * (1.0 - (-elapsed / 0.010_f64).exp());
    let expected = 0.1 * (1.0 + 0.8 * ((fast - slow) / slow).clamp(0.0, 1.0) - 0.4 * (slow / fast));
    assert!((f64::from(actual[0]) - expected).abs() < 2e-5);
    plugin.reset();
    let fresh = render(&mut plugin, 96_000, &[0.1, -0.025], &[1]);
    assert_eq!(fresh, [0.1, -0.025]);
    assert!(
        (actual[0] - fresh[0]).abs() > 0.01,
        "the test must distinguish retained and fresh history"
    );
}

#[test]
fn sustained_tone_emphasis_is_lower_than_onset_and_preserves_stereo_ratios() {
    for rate in [44_100, 48_000, 96_000] {
        let mut plugin = TransientShaperPlugin::from_params(
            2,
            TransientShaperPluginParams {
                attack: 80.0,
                sustain: 0.0,
                sensitivity_db: -12.0,
                ..Default::default()
            },
        )
        .unwrap();
        plugin.initialize(f64::from(rate)).unwrap();
        let input: Vec<f32> = (0..rate as usize)
            .flat_map(|frame| {
                let x = (0.1
                    * (std::f64::consts::TAU * 997.0 * frame as f64 / f64::from(rate)).sin())
                    as f32;
                [x, -0.25 * x]
            })
            .collect();
        let output = render(&mut plugin, rate, &input, &[257, 8192, 31]);
        let start = rate as usize; // Last half second, after the initial onset.
        let input_energy: f64 = input[start..].iter().map(|&x| f64::from(x).powi(2)).sum();
        let output_energy: f64 = output[start..].iter().map(|&x| f64::from(x).powi(2)).sum();
        let gain = (output_energy / input_energy).sqrt();
        let onset_frames = (rate as usize / 100) * 2;
        let onset_input: f64 = input[..onset_frames]
            .iter()
            .map(|&x| f64::from(x).powi(2))
            .sum();
        let onset_output: f64 = output[..onset_frames]
            .iter()
            .map(|&x| f64::from(x).powi(2))
            .sum();
        let onset_gain = (onset_output / onset_input).sqrt();
        // This dual-envelope effect can still color a sustained tone. Test
        // its intended relative onset emphasis, not an undocumented tonal null.
        assert!(
            onset_gain > gain + 0.2 && onset_gain <= 1.80001,
            "rate={rate}: onset={onset_gain}, sustained={gain}"
        );
        assert!(
            output
                .as_chunks::<2>()
                .0
                .iter()
                .all(|f| (f[1] + 0.25 * f[0]).abs() < 1e-7)
        );
        plugin.reset();
        let quiet: Vec<f32> = input.iter().map(|x| x * 1e-5).collect();
        assert_eq!(
            render(&mut plugin, rate, &quiet, &[8192]),
            quiet,
            "a sustained tone entirely below sensitivity must remain unchanged"
        );
    }
}
