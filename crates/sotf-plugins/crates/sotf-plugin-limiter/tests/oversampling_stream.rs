//! Independent clocks, finite zero continuation, and lifecycle of prepared paths.
// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext, TailLength};
use sotf_plugin_limiter::{LimiterData, LimiterPlugin, LimiterPluginParams};

fn make(
    rate: u32,
    channels: usize,
    choice: usize,
    isp: bool,
    mix: f32,
    lookahead: f32,
) -> LimiterPlugin {
    let params: LimiterPluginParams = serde_json::from_value(serde_json::json!({
        "threshold_db": -6.0, "release_ms": 10.0, "lookahead_ms": lookahead,
        "oversampling": choice, "isp_mode": isp, "true_peak": isp, "mix": mix,
        "dual_release": true, "link_amount": 0.37,
    }))
    .unwrap();
    let mut plugin = LimiterPlugin::from_params(channels, params);
    plugin.initialize(f64::from(rate)).unwrap();
    plugin
}
fn programme(frames: usize, channels: usize, scale: f32) -> Vec<f32> {
    let mut state = 0x1234_5678_u32;
    (0..frames * channels)
        .map(|_| {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            ((state >> 8) as f32 / 8388608.0 - 1.0) * scale
        })
        .collect()
}
fn feed(plugin: &mut LimiterPlugin, rate: u32, input: &[f32], partitions: &[usize]) -> Vec<f32> {
    let channels = plugin.channels();
    let mut output = input.to_vec();
    let mut offset = 0;
    let mut block = 0;
    while offset < input.len() / channels {
        let frames = partitions[block % partitions.len()].min(input.len() / channels - offset);
        assert_eq!(
            plugin
                .process_in_place(
                    &mut output[offset * channels..(offset + frames) * channels],
                    &ProcessContext::new(rate, frames)
                )
                .unwrap(),
            frames
        );
        offset += frames;
        block += 1;
    }
    output
}
fn drain(plugin: &mut LimiterPlugin, rate: u32, capacities: &[usize]) -> Vec<f32> {
    let channels = plugin.channels();
    plugin.begin_drain(&ProcessContext::new(rate, 0)).unwrap();
    let mut output = Vec::new();
    for call in 0..10000 {
        let capacity = capacities[call % capacities.len()];
        let mut buffer = vec![12345.0; capacity * channels + channels];
        let bound = plugin.drain_call_bound().unwrap().get();
        assert!(bound > 0);
        let result = plugin
            .drain(
                &mut buffer[..capacity * channels],
                &ProcessContext::new(rate, 0),
            )
            .unwrap();
        assert!(result.frames <= capacity.min(256));
        assert!(
            buffer[result.frames * channels..]
                .iter()
                .all(|&x| x == 12345.0)
        );
        output.extend_from_slice(&buffer[..result.frames * channels]);
        if result.complete {
            assert_eq!(plugin.drain_call_bound().unwrap().get(), 1);
            assert_eq!(
                plugin
                    .drain(&mut [], &ProcessContext::new(rate, 0))
                    .unwrap()
                    .frames,
                0
            );
            return output;
        }
    }
    panic!("drain did not complete")
}

#[test]
fn dry_delay_is_exact_integer_native_latency_at_all_residual_phases() {
    for choice in [1, 2] {
        for rate in [44_100, 48_000, 96_000, 192_000] {
            for phase in 0..256 {
                let channels = if phase % 2 == 0 { 1 } else { 2 };
                let mut plugin = make(rate, channels, choice, false, 0.0, 0.137);
                let native_delay = (0.137_f32 * 0.001 * rate as f32) as usize;
                assert_eq!(plugin.latency_samples(), 512 + native_delay);
                let input = programme(256 + phase, channels, 0.1);
                let mut output = feed(&mut plugin, rate, &input, &[1, 7, 255, 513]);
                output.extend(drain(&mut plugin, rate, &[1, 17, 256, 1024]));
                let delay = plugin.latency_samples() * channels;
                assert!(output[..delay].iter().all(|&x| x == 0.0));
                assert_eq!(
                    &output[delay..delay + input.len()],
                    &input,
                    "choice{choice} rate{rate} phase{phase}"
                );
                assert!(output[delay + input.len()..].iter().all(|&x| x == 0.0));
            }
        }
    }
}

#[test]
fn nonlinear_finite_drain_matches_explicit_zero_continuation_and_partition() {
    for choice in [1, 2] {
        for (rate, channels) in [(44_100, 1), (48_000, 2), (96_000, 6), (192_000, 2)] {
            for isp in [false, true] {
                for phase in [0, 1, 17, 255] {
                    let input = programme(512 + phase, channels, 2.0);
                    let mut actual = make(rate, channels, choice, isp, 1.0, 0.251);
                    let mut reference = make(rate, channels, choice, isp, 1.0, 0.251);
                    let mut alternate = make(rate, channels, choice, isp, 1.0, 0.251);
                    let mut output = feed(&mut actual, rate, &input, &[1, 127, 513]);
                    let mut expected = feed(&mut reference, rate, &input, &[8192]);
                    let mut other = feed(&mut alternate, rate, &input, &[3, 256, 7]);
                    // A post-input control belongs to synthetic continuation and
                    // output time, not the pending accepted-input records.
                    for plugin in [&mut actual, &mut reference, &mut alternate] {
                        plugin
                            .parametric_set_parameter(
                                ParameterId::from("threshold"),
                                ParameterValue::Float(-12.0),
                            )
                            .unwrap();
                        plugin
                            .parametric_set_parameter(
                                ParameterId::from("release"),
                                ParameterValue::Float(70.0),
                            )
                            .unwrap();
                    }
                    let tail = drain(&mut actual, rate, &[1, 13, 256, 4096]);
                    let other_tail = drain(&mut alternate, rate, &[256]);
                    assert_eq!(
                        tail, other_tail,
                        "drain partition choice{choice} rate{rate} phase{phase} isp{isp}"
                    );
                    let zeros = vec![0.0; tail.len()];
                    expected.extend(feed(&mut reference, rate, &zeros, &[7, 255, 900]));
                    output.extend(tail);
                    other.extend(other_tail);
                    assert_eq!(output, other, "input partition");
                    let error = output
                        .iter()
                        .zip(&expected)
                        .map(|(a, b)| (a - b).abs())
                        .fold(0.0_f32, f32::max);
                    assert!(
                        error <= 2.0e-6,
                        "zero continuation error{error} choice{choice} rate{rate} phase{phase} isp{isp}"
                    );
                    let TailLength::Finite(bound) = actual.tail_length() else {
                        panic!("finite tail")
                    };
                    assert!(output.len() / channels <= input.len() / channels + bound as usize);
                }
            }
        }
    }
}

#[test]
fn preflight_and_failed_reinitialization_preserve_live_audio_and_drain() {
    for choice in [1, 2] {
        let rate = 48_000;
        let mut actual = make(rate, 2, choice, true, 1.0, 0.25);
        let mut twin = make(rate, 2, choice, true, 1.0, 0.25);
        let input = programme(513, 2, 1.3);
        assert_eq!(
            feed(&mut actual, rate, &input, &[513]),
            feed(&mut twin, rate, &input, &[513])
        );
        assert!(actual.initialize(f64::from(u32::MAX)).is_err());
        assert_eq!(actual.latency_samples(), twin.latency_samples());
        let mut malformed = [12345.0; 3];
        assert!(
            actual
                .drain(&mut malformed, &ProcessContext::new(rate, 0))
                .is_err()
        );
        assert_eq!(malformed, [12345.0; 3]);
        assert!(
            actual
                .drain(&mut [], &ProcessContext::new(rate, 0))
                .is_err()
        );
        assert!(actual.begin_drain(&ProcessContext::new(44_100, 0)).is_err());
        assert!(
            actual
                .process_in_place(&mut [0.0; 3], &ProcessContext::new(rate, 1))
                .is_err()
        );
        assert_eq!(
            drain(&mut actual, rate, &[1, 255]),
            drain(&mut twin, rate, &[256])
        );
        actual
            .parametric_set_parameter(
                ParameterId::from("oversampling"),
                ParameterValue::Int(choice as i32),
            )
            .unwrap();
        assert!(
            actual
                .parametric_set_parameter(
                    ParameterId::from("threshold"),
                    ParameterValue::Float(-3.0)
                )
                .is_err()
        );
        assert!(
            actual
                .process_in_place(&mut [0.0; 2], &ProcessContext::new(rate, 1))
                .is_err()
        );
        actual.reset();
        twin.reset();
        assert_eq!(
            feed(&mut actual, rate, &input, &[7, 31]),
            feed(&mut twin, rate, &input, &[513])
        );
    }
}

#[test]
fn preinitialization_metadata_and_legacy_json_are_unambiguous() {
    let params: LimiterPluginParams = serde_json::from_str("{}").unwrap();
    assert_eq!(params.oversampling, 0);
    for choice in [1, 2] {
        let mut plugin = LimiterPlugin::new(2, -1.0, 50.0, 5.0, false);
        plugin
            .parametric_set_parameter(
                ParameterId::from("oversampling"),
                ParameterValue::Int(choice),
            )
            .unwrap();
        assert_eq!(plugin.latency_samples(), 0);
        assert!(matches!(plugin.tail_length(), TailLength::Unknown));
        assert_eq!(plugin.drain_output_frames_max(), 256);
        assert!(plugin.drain_call_bound().is_none());
        assert!(plugin.compile_metadata().compiled_op.is_none());
        assert_eq!(plugin.cost_class(), sotf_host::plugin::PluginCostClass::Fft);
        assert_eq!(
            plugin.compile_metadata().cost_class,
            sotf_host::plugin::PluginCostClass::Fft
        );
        assert!(plugin.preferred_oversampling().is_none());
        assert!(
            plugin
                .process_in_place(&mut [0.0; 2], &ProcessContext::new(48_000, 1))
                .is_err()
        );
        plugin.initialize(48_000.0).unwrap();
        assert!(
            plugin
                .parametric_set_parameter(ParameterId::from("oversampling"), ParameterValue::Int(0))
                .is_err()
        );
        let schema = plugin.parameter_schema();
        assert_eq!(schema[10].id.as_str(), "oversampling");
    }
}

#[test]
fn neutral_low_level_high_frequency_and_dry_limiting_never_light_meter() {
    let rate = 48_000;
    for choice in [1, 2] {
        for mix in [0.0, 0.5, 1.0] {
            for frequency in [1000.0, 0.45 * rate as f64, 0.49 * rate as f64] {
                let mut plugin = make(rate, 2, choice, false, mix, 0.137);
                let input: Vec<_> = (0..rate as usize / 2)
                    .flat_map(|frame| {
                        let sample = (2.0 * std::f64::consts::PI * frequency * frame as f64
                            / rate as f64)
                            .sin() as f32
                            * 0.001;
                        [sample, -sample]
                    })
                    .collect();
                feed(&mut plugin, rate, &input, &[1, 257, 8192]);
                let data = plugin
                    .get_data()
                    .unwrap()
                    .downcast::<LimiterData>()
                    .unwrap();
                assert_eq!(
                    data.gain_reduction_db, 0.0,
                    "neutral choice{choice} mix{mix} f{frequency}"
                );
                assert!(!data.is_limiting);
                drain(&mut plugin, rate, &[1, 256]);
            }
        }
        let mut plugin = make(rate, 2, choice, false, 0.0, 0.137);
        feed(
            &mut plugin,
            rate,
            &programme(rate as usize, 2, 20.0),
            &[256],
        );
        let data = plugin
            .get_data()
            .unwrap()
            .downcast::<LimiterData>()
            .unwrap();
        assert_eq!(data.gain_reduction_db, 0.0);
        assert!(!data.is_limiting);
    }
}

#[test]
fn rejected_bulk_factor_change_cannot_retarget_earlier_controls() {
    use sotf_host::parametric_plugin::ParameterSet;
    for choice in [0, 1, 2] {
        for factor in [
            ParameterValue::Int(if choice == 1 { 2 } else { 1 }),
            ParameterValue::Float(1.0),
            ParameterValue::Int(-1),
            ParameterValue::Int(3),
        ] {
            let mut actual = make(48_000, 2, choice, false, 1.0, 0.137);
            let mut twin = make(48_000, 2, choice, false, 1.0, 0.137);
            let input = programme(513, 2, 1.5);
            assert_eq!(
                feed(&mut actual, 48_000, &input, &[513]),
                feed(&mut twin, 48_000, &input, &[513])
            );
            let before = actual.current_values();
            let mut values = ParameterSet::new();
            // BTreeMap visits mix before oversampling, so a sequential setter
            // loop would retarget it before rejecting the new factor.
            values.insert(ParameterId::from("mix"), ParameterValue::Float(0.35));
            values.insert(ParameterId::from("oversampling"), factor);
            assert!(actual.apply_values(values).is_err());
            assert_eq!(actual.current_values(), before);
            assert_eq!(
                feed(&mut actual, 48_000, &input, &[1, 257]),
                feed(&mut twin, 48_000, &input, &[513])
            );
        }
    }
}

#[test]
fn reset_installs_current_controls_and_successful_reinitialize_matches_fresh_rate() {
    for choice in [1, 2] {
        let mut actual = make(48_000, 2, choice, false, 1.0, 0.137);
        feed(&mut actual, 48_000, &programme(31, 2, 3.0), &[31]);
        let settings = [
            ("threshold", ParameterValue::Float(-10.0)),
            ("release", ParameterValue::Float(97.0)),
            ("mix", ParameterValue::Float(0.25)),
            ("dual_release", ParameterValue::Bool(true)),
            ("link_amount", ParameterValue::Float(0.9)),
            ("true_peak", ParameterValue::Bool(true)),
        ];
        for (key, value) in &settings {
            actual
                .parametric_set_parameter(ParameterId::from(*key), value.clone())
                .unwrap();
        }
        // All new controls differ from the unconsumed pending input snapshot.
        actual.reset();
        for rate in [48_000, 192_000] {
            if rate != 48_000 {
                actual.initialize(f64::from(rate)).unwrap();
            }
            let mut fresh = make(rate, 2, choice, false, 0.25, 0.137);
            for (key, value) in &settings {
                fresh
                    .parametric_set_parameter(ParameterId::from(*key), value.clone())
                    .unwrap();
            }
            // Constructor controls have no transition when starting a new epoch.
            fresh.reset();
            let input = programme(511, 2, 1.7);
            assert_eq!(
                feed(&mut actual, rate, &input, &[1, 127]),
                feed(&mut fresh, rate, &input, &[511])
            );
            let context = ProcessContext::new(rate, 0);
            actual.begin_drain(&context).unwrap();
            fresh.begin_drain(&context).unwrap();
            let mut a = [0.0; 2];
            let mut b = [0.0; 2];
            assert_eq!(
                actual.drain(&mut a, &context).unwrap(),
                fresh.drain(&mut b, &context).unwrap()
            );
            assert_eq!(a, b);
            assert!(actual.initialize(f64::from(u32::MAX)).is_err());
            assert_eq!(
                drain(&mut actual, rate, &[1, 17, 256]),
                drain(&mut fresh, rate, &[256])
            );
        }
    }
}
