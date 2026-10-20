//! Exact capture extent and causal profile adoption at callback boundaries.

// Rust guideline compliant 2026-02-21
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{
    CountingAlloc, ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext,
};
use sotf_plugin_hiss_reducer::profile::CaptureState;
use sotf_plugin_hiss_reducer::{HissReducerPlugin, HissReducerPluginParams};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

fn noise(frames: usize, channels: usize) -> Vec<f32> {
    let mut seed = 0x173a_58e9_u32;
    (0..frames * channels)
        .map(|_| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            0.03 * (seed as f64 / u32::MAX as f64 * 2.0 - 1.0) as f32
        })
        .collect()
}

#[test]
fn oversized_capture_ignores_program_after_the_exact_one_second_endpoint() {
    for rate in [44_100.0_f64, 48_000.0, 96_000.0, 44_100.25] {
        let target = rate.round() as usize;
        for channels in [1, 2, 8] {
            let prefix = noise(target, channels);
            let mut overrun = prefix.clone();
            overrun.extend((0..2049 * channels).map(|i| {
                (0.6 * (std::f64::consts::TAU * 6000.0 * (i / channels) as f64 / rate).sin()) as f32
            }));
            let mut actual = CaptureState::new(channels);
            let mut reference = CaptureState::new(channels);
            actual.start(rate, 4000.0).unwrap();
            reference.start(rate, 4000.0).unwrap();
            assert_no_allocs_or_deallocs("bounded capture accumulation", || {
                actual.accumulate(&overrun)
            });
            reference.accumulate(&prefix);
            assert_eq!(
                actual.frames_accumulated(),
                target as u64,
                "capture overrun {rate}/{channels}"
            );
            let mut a = vec![0.0; channels];
            let mut b = a.clone();
            let mut sa = vec![0.0; channels * 513];
            let mut sb = sa.clone();
            assert_eq!(
                actual.take_completed(&mut a, &mut sa),
                reference.take_completed(&mut b, &mut sb)
            );
            assert_eq!(a, b);
            assert_eq!(sa, sb);
        }
    }
}

fn plugin(rate: f64, channels: usize, spectral: bool) -> HissReducerPlugin {
    let mut p = HissReducerPlugin::from_params(
        channels,
        HissReducerPluginParams {
            spectral_mode: spectral,
            use_captured_profile: true,
            threshold_db: -80.0,
            strength: 1.0,
            frequency_hz: 4000.0,
            ..Default::default()
        },
    );
    p.initialize(rate).unwrap();
    p
}

fn render(
    p: &mut HissReducerPlugin,
    input: &[f32],
    rate: f64,
    channels: usize,
    parts: &[usize],
) -> Vec<f32> {
    let mut out = input.to_vec();
    let mut offset = 0;
    let mut part = 0;
    while offset < input.len() / channels {
        let n = parts[part % parts.len()].min(input.len() / channels - offset);
        assert_no_allocs_or_deallocs("profile boundary callback", || {
            p.process_in_place(
                &mut out[offset * channels..(offset + n) * channels],
                &ProcessContext::new(rate, n),
            )
            .unwrap();
        });
        offset += n;
        part += 1;
    }
    assert!(out.iter().all(|x| x.is_finite()));
    out
}

#[test]
fn new_profile_is_adopted_after_capture_and_is_callback_partition_independent() {
    for spectral in [false, true] {
        let rate = 48_000.0;
        let target = rate as usize;
        let channels = 2;
        let input = noise(target + 4097, channels);
        let mut whole = plugin(rate, channels, spectral);
        let mut split = plugin(rate, channels, spectral);
        let mut no_profile = plugin(rate, channels, spectral);
        for p in [&mut whole, &mut split] {
            p.parametric_set_parameter(
                ParameterId::from("learn_noise"),
                ParameterValue::Bool(true),
            )
            .unwrap();
        }
        let actual = render(
            &mut whole,
            &input,
            rate,
            channels,
            &[input.len() / channels],
        );
        let expected = render(&mut split, &input, rate, channels, &[1, 73, 257, 1024]);
        let untouched = render(&mut no_profile, &input, rate, channels, &[257]);
        let maximum = actual
            .iter()
            .zip(&expected)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);
        assert_eq!(
            maximum, 0.0,
            "profile audio depends on callback partition, spectral={spectral}"
        );
        let prefix_error = actual[..target * channels]
            .iter()
            .zip(&untouched[..target * channels])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);
        assert_eq!(
            prefix_error, 0.0,
            "new profile used before capture completion"
        );
        assert_eq!(
            whole.persisted_params().captured_profile,
            split.persisted_params().captured_profile
        );
    }
}

#[test]
fn backend_switch_refusals_preserve_partial_capture_and_the_previous_profile() {
    use sotf_host::parametric_plugin::ParameterSet;
    for spectral in [false, true] {
        let input = noise(48_000, 1);
        let mut actual = plugin(48_000.0, 1, spectral);
        let mut reference = plugin(48_000.0, 1, spectral);
        for p in [&mut actual, &mut reference] {
            p.parametric_set_parameter("learn_noise".into(), ParameterValue::Bool(true))
                .unwrap();
            render(p, &input, 48_000.0, 1, &[257]);
            p.parametric_set_parameter("learn_noise".into(), ParameterValue::Bool(true))
                .unwrap();
        }
        let old = actual.persisted_params().captured_profile.unwrap();
        let next: Vec<_> = input.iter().map(|x| x * 4.0).collect();
        let a = render(&mut actual, &next[..24_000], 48_000.0, 1, &[73]);
        let b = render(&mut reference, &next[..24_000], 48_000.0, 1, &[73]);
        assert!(a == b);
        assert!(
            actual
                .parametric_set_parameter("spectral_mode".into(), ParameterValue::Bool(!spectral))
                .is_err()
        );
        let mut batch = ParameterSet::new();
        batch.insert("threshold_db".into(), ParameterValue::Float(-20.0));
        batch.insert("spectral_mode".into(), ParameterValue::Bool(!spectral));
        assert!(actual.apply_values(batch).is_err());
        assert_eq!(
            actual.persisted_params().captured_profile.as_ref(),
            Some(&old)
        );

        // Reconstruction is the supported switch: export only the completed
        // profile, never a partial in-progress measurement.
        let mut switched_params = actual.persisted_params();
        switched_params.spectral_mode = !spectral;
        let mut switched = HissReducerPlugin::from_params(1, switched_params);
        switched.initialize(48_000.0).unwrap();
        assert_eq!(
            switched.persisted_params().captured_profile.as_ref(),
            Some(&old)
        );
        assert_eq!(
            switched.parametric_get_parameter(&"learn_noise".into()),
            Some(ParameterValue::Bool(false))
        );

        let a = render(&mut actual, &next[24_000..], 48_000.0, 1, &[8192]);
        let b = render(&mut reference, &next[24_000..], 48_000.0, 1, &[8192]);
        assert!(a == b);
        let new = actual.persisted_params().captured_profile.unwrap();
        assert_ne!(new.floor_db_per_channel, old.floor_db_per_channel);
        assert_eq!(Some(new), reference.persisted_params().captured_profile);
    }
}

#[test]
fn cross_rate_reinitialization_cancels_partial_capture_and_never_uses_stale_bins() {
    for spectral in [false, true] {
        let mut actual = plugin(48_000.0, 1, spectral);
        actual
            .parametric_set_parameter("learn_noise".into(), ParameterValue::Bool(true))
            .unwrap();
        render(&mut actual, &noise(48_000, 1), 48_000.0, 1, &[513]);
        let stored = actual.persisted_params().captured_profile.unwrap();
        let mut previous_rate = 48_000.0;
        for rate in [96_000.0, 44_100.0, 48_000.5, 48_000.0] {
            actual
                .parametric_set_parameter("learn_noise".into(), ParameterValue::Bool(true))
                .unwrap();
            render(&mut actual, &noise(513, 1), previous_rate, 1, &[513]);
            actual.initialize(rate).unwrap();
            assert_eq!(
                actual.parametric_get_parameter(&"learn_noise".into()),
                Some(ParameterValue::Bool(false))
            );
            assert_eq!(
                actual.persisted_params().captured_profile.as_ref(),
                Some(&stored)
            );
            assert_eq!(actual.profile_uses_spectral(), rate == 48_000.0);
            let mut expected_params = actual.persisted_params();
            if rate != 48_000.0 {
                let profile = expected_params.captured_profile.as_mut().unwrap();
                profile.spectral = None;
                profile.format_version = 1;
            }
            let mut expected = HissReducerPlugin::from_params(1, expected_params);
            expected.initialize(rate).unwrap();
            actual.reset();
            expected.reset();
            let input = noise(4097, 1);
            let a = render(&mut actual, &input, rate, 1, &[7, 257]);
            let b = render(&mut expected, &input, rate, 1, &[1024]);
            assert!(
                a == b,
                "stale spectral bins or state at {rate}, spectral={spectral}"
            );
            previous_rate = rate;
        }
    }
}
