// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, Plugin, ProcessContext, TailLength};
use sotf_plugin_beamformer::{BeamformerPlugin, BeamformerPluginParams};

fn expected_bound(algorithm: usize, mics: usize, rate: u32, angle: f32, spacing_cm: f32) -> u64 {
    if algorithm != 2 {
        return 1024;
    }
    // Independent plane-wave geometry: the end-to-end path difference is
    // array length times |sin(angle)|, with sound speed 343 meters/second.
    let delay = (mics - 1) as f64 * f64::from(spacing_cm) / 100.0
        * f64::from(angle).to_radians().sin().abs()
        * f64::from(rate)
        / 343.0;
    delay.ceil() as u64 + 31
}

fn make(algorithm: usize, mics: usize, rate: u32, angle: f32, spacing: f32) -> BeamformerPlugin {
    BeamformerPlugin::from_params(
        rate,
        BeamformerPluginParams {
            num_mics: mics,
            beamformer_type: algorithm,
            steer_angle_deg: angle,
            mic_spacing_cm: spacing,
        },
    )
    .unwrap()
}

fn feed(plugin: &mut BeamformerPlugin, input: &[f32], rate: u32) -> Vec<f32> {
    let mics = plugin.input_channels();
    let frames = input.len() / mics;
    let mut output = vec![f32::NAN; frames];
    let mut position = 0;
    let mut iteration = 0;
    while position < frames {
        let take = [17, 257, 1, 73][iteration % 4].min(frames - position);
        assert_eq!(
            plugin.process(
                &input[position * mics..(position + take) * mics],
                &mut output[position..position + take],
                &ProcessContext::new(rate, take)
            ),
            Ok(take)
        );
        position += take;
        iteration += 1;
    }
    output
}

#[test]
fn native_bound_follows_prepared_geometry_and_stays_constant_through_lifecycle() {
    for algorithm in 0..=2 {
        for mics in [2, 8] {
            for angle in [0.0, -37.0] {
                let mut plugin = make(algorithm, mics, 44_100, angle, 50.0);
                assert_eq!(
                    plugin.tail_length(),
                    TailLength::Finite(expected_bound(algorithm, mics, 44_100, angle, 50.0))
                );
                for rate in [44_100, 192_000] {
                    plugin.initialize(f64::from(rate)).unwrap();
                    let expected =
                        TailLength::Finite(expected_bound(algorithm, mics, rate, angle, 50.0));
                    assert_eq!(plugin.tail_length(), expected);
                    feed(&mut plugin, &vec![0.125; 17 * mics], rate);
                    assert_eq!(plugin.tail_length(), expected);
                    assert!(
                        plugin
                            .set_parameter(
                                ParameterId::from("steer_angle_deg"),
                                ParameterValue::Float(12.0)
                            )
                            .is_err()
                    );
                    assert_eq!(plugin.tail_length(), expected);
                    let mut output = [0.0; 256];
                    let mut complete = false;
                    for iteration in 0..32 {
                        let capacity = if iteration == 0 { 1 } else { 256 };
                        let result = plugin
                            .drain(&mut output[..capacity], &ProcessContext::new(rate, 0))
                            .unwrap();
                        assert_eq!(plugin.tail_length(), expected);
                        if result.complete {
                            complete = true;
                            break;
                        }
                    }
                    assert!(complete);
                    assert_eq!(
                        plugin
                            .drain(&mut output, &ProcessContext::new(rate, 0))
                            .unwrap()
                            .frames,
                        0
                    );
                    assert_eq!(plugin.tail_length(), expected);
                    plugin.reset();
                    assert_eq!(plugin.tail_length(), expected);
                }
            }
        }
    }
    let mut unclocked = BeamformerPlugin::new(2, 0).unwrap();
    assert_eq!(unclocked.tail_length(), TailLength::Unknown);
    unclocked.initialize(48_000.0).unwrap();
    assert_eq!(unclocked.tail_length(), TailLength::Finite(1024));
}

#[test]
fn every_hop_phase_clears_with_ordinary_adaptation_inside_the_native_bound() {
    for algorithm in 0..=2 {
        for phase in 1..=256 {
            let mut plugin = make(algorithm, 2, 48_000, 37.0, 50.0);
            let input: Vec<f32> = (0..1024 + phase)
                .flat_map(|n| {
                    [
                        (n as f32 * 0.13).sin() * 0.25,
                        (n as f32 * 0.31).cos() * 0.125,
                    ]
                })
                .collect();
            feed(&mut plugin, &input, 48_000);
            let TailLength::Finite(bound) = plugin.tail_length() else {
                panic!("finite prepared bound")
            };
            assert_eq!(bound, expected_bound(algorithm, 2, 48_000, 37.0, 50.0));
            let output = feed(&mut plugin, &vec![0.0; (bound as usize + 1024) * 2], 48_000);
            assert!(output.iter().all(|x| x.is_finite()));
            assert!(
                output[bound as usize..].iter().all(|&x| x == 0.0),
                "algorithm={algorithm}, phase={phase}"
            );
            assert_eq!(plugin.tail_length(), TailLength::Finite(bound));
        }
    }
}

#[test]
fn spectral_overflow_residue_clears_within_the_same_support() {
    // Finite extreme input can overflow f32 FFT arithmetic (AUD-101). This
    // verifies support, not finite output or later covariance quality during
    // that separate unresolved numerical limitation. Do not freeze adaptation.
    for algorithm in 0..=1 {
        for mics in [2, 8] {
            for phase in [17, 255] {
                for coherent in [false, true] {
                    let mut plugin = make(algorithm, mics, 48_000, 37.0, 50.0);
                    let training: Vec<f32> = (0..1024)
                        .flat_map(|n| {
                            (0..mics).map(move |mic| {
                                ((n as f64 * 0.13 + mic as f64 * 0.71).sin() * 0.25) as f32
                            })
                        })
                        .collect();
                    feed(&mut plugin, &training, 48_000);
                    let burst: Vec<f32> = (0..512 + phase)
                        .flat_map(|_| {
                            (0..mics).map(move |mic| {
                                if coherent || mic % 2 == 0 {
                                    f32::MAX
                                } else {
                                    -f32::MAX
                                }
                            })
                        })
                        .collect();
                    assert!(burst.iter().all(|x| x.is_finite()));
                    feed(&mut plugin, &burst, 48_000);
                    assert_eq!(plugin.tail_length(), TailLength::Finite(1024));
                    let output = feed(&mut plugin, &vec![0.0; (1024 + 8192) * mics], 48_000);
                    assert!(
                        output[1024..].iter().all(|&x| x == 0.0),
                        "algorithm={algorithm}, mics={mics}, phase={phase}"
                    );
                    plugin.reset();
                    assert_eq!(plugin.tail_length(), TailLength::Finite(1024));
                    assert!(
                        feed(&mut plugin, &vec![0.0; 2048 * mics], 48_000)
                            .iter()
                            .all(|&x| x == 0.0)
                    );
                }
            }
        }
    }
}
