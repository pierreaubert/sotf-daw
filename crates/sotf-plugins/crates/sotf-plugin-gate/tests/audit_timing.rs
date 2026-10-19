//! Independent gate latch, sample clock, and linked first-hit checks.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_gate::{GatePlugin, GatePluginParams};

fn prepared(rate: u32, channels: usize, hold_ms: f32, hysteresis_db: f32) -> GatePlugin {
    let mut gate = GatePlugin::try_from_params(
        channels,
        GatePluginParams {
            threshold_db: -20.0,
            ratio: 100.0,
            attack_ms: 1.0,
            hold_ms,
            release_ms: 50.0,
            range_db: 60.0,
            hysteresis_db,
            knee_db: 0.0,
            sidechain_external: true,
            sidechain_hpf_hz: 0.0,
            detection_mode: "peak".into(),
            link_channels: channels > 1,
            ..Default::default()
        },
    )
    .unwrap();
    gate.initialize(f64::from(rate)).unwrap();
    gate
}

fn render(gate: &mut GatePlugin, rate: u32, sidechain: &[f32], pattern: &[usize]) -> Vec<f32> {
    // A unit programme separates the audible gain from the detector signal.
    let mut buffer: Vec<f32> = sidechain.iter().flat_map(|&sc| [1.0, sc]).collect();
    let mut position = 0;
    for &requested in pattern.iter().cycle() {
        let frames = requested.min(sidechain.len() - position);
        if frames == 0 {
            break;
        }
        gate.process_in_place(
            &mut buffer[position * 2..(position + frames) * 2],
            &ProcessContext::new(rate, frames),
        )
        .unwrap();
        position += frames;
    }
    buffer
        .as_chunks::<2>()
        .0
        .iter()
        .map(|frame| frame[0])
        .collect()
}

#[test]
fn hysteresis_preserves_the_open_latch_without_a_vacuous_unity_ratio() {
    let sidechain: Vec<f32> = (0..12_000)
        .map(|i| if i % 2 == 0 { 0.12 } else { 0.08 })
        .collect();
    for rate in [44_100, 48_000, 96_000] {
        let with_hysteresis = render(
            &mut prepared(rate, 1, 0.0, 4.0),
            rate,
            &sidechain,
            &[7, 257, 8192],
        );
        assert!(
            with_hysteresis.iter().all(|&gain| gain == 1.0),
            "an opened latch must stay open above the -24 dB closing threshold"
        );
        let without = render(
            &mut prepared(rate, 1, 0.0, 0.0),
            rate,
            &sidechain,
            &[7, 257, 8192],
        );
        assert!(
            without[6000..].iter().any(|&gain| gain < 0.9),
            "the same signal must exercise actual gain reduction without hysteresis"
        );
    }
}

#[test]
fn hold_retrigger_and_attack_release_follow_independent_sample_time_oracles() {
    for rate in [44_100, 48_000, 96_000] {
        let hold = (0.010 * f64::from(rate)).round() as usize;
        let partial_hold = hold / 2;
        let closing_frames = rate as usize / 20; // One 50 ms release time constant.
        let opening_frames = rate as usize / 100; // Ten 1 ms attack time constants.
        let mut sidechain = vec![0.2];
        sidechain.extend(vec![0.05; partial_hold]);
        sidechain.push(0.2); // Retrigger before the first hold expires.
        sidechain.extend(vec![0.05; hold + closing_frames]);
        let reopening = sidechain.len();
        sidechain.extend(vec![0.2; opening_frames]);
        let reference = render(&mut prepared(rate, 1, 10.0, 0.0), rate, &sidechain, &[1]);
        for pattern in [&[8192][..], &[7, 31, 257][..]] {
            let actual = render(&mut prepared(rate, 1, 10.0, 0.0), rate, &sidechain, pattern);
            assert_eq!(
                actual, reference,
                "sample clock must not depend on callbacks"
            );
        }
        let release_start = 2 + partial_hold + hold;
        let mut worst_release_error = 0.0_f64;
        let mut worst_attack_error = 0.0_f64;
        assert!(
            reference[..release_start].iter().all(|&gain| gain == 1.0),
            "the retriggered hold must last exactly {hold} below-threshold samples"
        );
        for (sample, &gain) in reference[release_start..reopening].iter().enumerate() {
            let expected_db =
                60.0 * (1.0 - (-((sample + 1) as f64) / (0.050 * f64::from(rate))).exp());
            let actual_db = -20.0 * f64::from(gain).log10();
            worst_release_error = worst_release_error.max((actual_db - expected_db).abs());
        }
        let initial_db =
            60.0 * (1.0 - (-(closing_frames as f64) / (0.050 * f64::from(rate))).exp());
        for (sample, &gain) in reference[reopening..].iter().enumerate() {
            let expected_db =
                initial_db * (-((sample + 1) as f64) / (0.001 * f64::from(rate))).exp();
            let actual_db = -20.0 * f64::from(gain).log10();
            worst_attack_error = worst_attack_error.max((actual_db - expected_db).abs());
        }
        println!(
            "rate={rate}, max_release_error_db={worst_release_error}, max_attack_error_db={worst_attack_error}"
        );
        // Includes the documented fast log/power conversion approximation, not
        // just f32 pole rounding. A 0.02 dB bound is under 0.24% in amplitude
        // and is below the mid-trajectory error of a 1% time-constant change.
        assert!(worst_release_error < 0.02 && worst_attack_error < 0.02);
    }
}

#[test]
fn a_first_hit_on_either_detector_channel_preserves_the_linked_stereo_image() {
    for rate in [44_100, 48_000, 96_000] {
        for detector_channel in [0, 1] {
            let mut gate = prepared(rate, 2, 10.0, 0.0);
            let mut buffer = vec![0.0; 1024 * 4];
            // Unequal programme amplitudes and one-sided detection expose
            // independent-channel gain or accidental channel mapping changes.
            buffer[0] = 0.8;
            buffer[1] = -0.2;
            buffer[2 + detector_channel] = 0.8;
            gate.process_in_place(&mut buffer, &ProcessContext::new(rate, 1024))
                .unwrap();
            assert_eq!(
                &buffer[..2],
                &[0.8, -0.2],
                "startup must not suppress the first hit"
            );
            assert!(
                buffer
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .skip(1)
                    .all(|f| f[0] == 0.0 && f[1] == 0.0)
            );
        }
    }
}
