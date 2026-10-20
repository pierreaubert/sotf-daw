//! Reinitialization must clear prior program and settle retained controls.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_de_esser::{DeEsserPlugin, DeEsserPluginParams};

#[test]
fn populated_reinitialize_matches_fresh_reset_at_retained_targets() {
    for (mode, topology) in [
        ("Wideband", "Minimum-Phase"),
        ("Split-Band", "Minimum-Phase"),
        ("Split-Band", "Linear-Phase"),
    ] {
        for rate in [44_100, 48_000, 96_000] {
            let params = DeEsserPluginParams {
                frequency: 6000.0,
                threshold: -40.0,
                ratio: 20.0,
                attack_ms: 0.1,
                release_ms: 200.0,
                range_db: 12.0,
                lookahead_ms: 2.0,
                mode: mode.into(),
                split_topology: topology.into(),
                ..Default::default()
            };
            let mut actual = DeEsserPlugin::from_params(2, params.clone()).unwrap();
            actual.initialize(48_000.0).unwrap();
            let mut warm: Vec<_> = (0..4800)
                .flat_map(|i| {
                    let x = 0.8 * (std::f32::consts::TAU * 6000.0 * i as f32 / 48_000.0).sin();
                    [x, 0.1 * x]
                })
                .collect();
            actual
                .process_in_place(&mut warm, &ProcessContext::new(48_000, 4800))
                .unwrap();
            for (name, value) in [("mix", 0.37), ("range_db", 6.0), ("stereo_link", 0.3)] {
                actual
                    .parametric_set_parameter(ParameterId::from(name), ParameterValue::Float(value))
                    .unwrap();
            }
            actual.initialize(f64::from(rate)).unwrap();
            let mut fresh = DeEsserPlugin::from_params(
                2,
                DeEsserPluginParams {
                    mix: 0.37,
                    range_db: 6.0,
                    stereo_link: 0.3,
                    ..params
                },
            )
            .unwrap();
            fresh.initialize(f64::from(rate)).unwrap();
            fresh.reset();
            let input: Vec<_> = (0..4097)
                .flat_map(|i| {
                    let x = 0.2 * (std::f32::consts::TAU * 6000.0 * i as f32 / rate as f32).sin();
                    [x, 0.2 * x]
                })
                .collect();
            let mut a = input.clone();
            let mut b = input;
            actual
                .process_in_place(&mut a, &ProcessContext::new(rate, 4097))
                .unwrap();
            fresh
                .process_in_place(&mut b, &ProcessContext::new(rate, 4097))
                .unwrap();
            let maximum = a
                .iter()
                .zip(&b)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert_eq!(maximum, 0.0, "{mode}/{topology}/{rate}");
        }
    }
}

#[test]
fn rejected_reinitialize_preserves_populated_program_history() {
    for mode in ["Wideband", "Split-Band"] {
        let params = DeEsserPluginParams {
            frequency: 6000.0,
            threshold: -40.0,
            ratio: 20.0,
            attack_ms: 0.1,
            release_ms: 200.0,
            mode: mode.into(),
            lookahead_ms: 2.0,
            ..Default::default()
        };
        let mut actual = DeEsserPlugin::from_params(2, params.clone()).unwrap();
        let mut expected = DeEsserPlugin::from_params(2, params).unwrap();
        actual.initialize(48_000.0).unwrap();
        expected.initialize(48_000.0).unwrap();
        for epoch in 0..2 {
            if epoch == 1 {
                for invalid in [0.0, f64::NAN, f64::INFINITY, 8000.0] {
                    assert!(actual.initialize(invalid).is_err());
                }
            }
            let input: Vec<_> = (0..4096)
                .flat_map(|i| {
                    let x = 0.7 * (std::f32::consts::TAU * 6000.0 * i as f32 / 48_000.0).sin();
                    [x, x * 0.1]
                })
                .collect();
            let mut a = input.clone();
            let mut b = input;
            actual
                .process_in_place(&mut a, &ProcessContext::new(48_000, 4096))
                .unwrap();
            expected
                .process_in_place(&mut b, &ProcessContext::new(48_000, 4096))
                .unwrap();
            let maximum = a
                .iter()
                .zip(&b)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert_eq!(maximum, 0.0, "{mode}/{epoch}");
        }
    }
}

#[test]
fn quiet_clip_after_reinitialize_is_not_ducked_by_the_previous_fricative() {
    for (mode, topology) in [
        ("Wideband", "Minimum-Phase"),
        ("Split-Band", "Minimum-Phase"),
        ("Split-Band", "Linear-Phase"),
    ] {
        let params = DeEsserPluginParams {
            frequency: 6000.0,
            threshold: -40.0,
            ratio: 20.0,
            attack_ms: 0.1,
            release_ms: 200.0,
            range_db: 12.0,
            mode: mode.into(),
            split_topology: topology.into(),
            lookahead_ms: 2.0,
            ..Default::default()
        };
        let mut actual = DeEsserPlugin::from_params(2, params.clone()).unwrap();
        actual.initialize(48_000.0).unwrap();
        let tone = |amplitude| {
            (0..4800)
                .flat_map(|i| {
                    let x =
                        amplitude * (std::f32::consts::TAU * 6000.0 * i as f32 / 48_000.0).sin();
                    [x, x * 0.1]
                })
                .collect::<Vec<_>>()
        };
        actual
            .process_in_place(&mut tone(0.8), &ProcessContext::new(48_000, 4800))
            .unwrap();
        // Retain every control target unchanged: this isolates the dynamics
        // envelope from the live-smoother transition in the other regression.
        actual.initialize(48_000.0).unwrap();
        let mut fresh = DeEsserPlugin::from_params(2, params).unwrap();
        fresh.initialize(48_000.0).unwrap();
        fresh.reset();
        let mut a = tone(0.0001);
        let mut b = a.clone();
        actual
            .process_in_place(&mut a, &ProcessContext::new(48_000, 4800))
            .unwrap();
        fresh
            .process_in_place(&mut b, &ProcessContext::new(48_000, 4800))
            .unwrap();
        let maximum = a
            .iter()
            .zip(&b)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);
        assert_eq!(maximum, 0.0, "quiet restart {mode}/{topology}");
    }
}
