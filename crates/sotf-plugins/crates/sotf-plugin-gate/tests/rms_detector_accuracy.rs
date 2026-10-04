//! Public Gate regression for finite extreme RMS energy and quiet-signal recovery.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_gate::{GateMode, GatePlugin, GatePluginParams};

fn verify_gain(rate: u32, mode: GateMode, background: f32, threshold: f32, ratio: f32) {
    let window = (10.0_f32 * 0.001 * rate as f32).round() as usize;
    let frames = window * 16;
    let mut detector = vec![background; frames];
    detector[window] = 1.0e20;
    detector[window + window / 2] = -1.0e20;
    let params = GatePluginParams {
        mode,
        threshold_db: threshold,
        ratio,
        attack_ms: 1.0,
        hold_ms: 0.0,
        release_ms: 10.0,
        knee_db: 0.0,
        hysteresis_db: 0.0,
        max_boost_db: 12.0,
        range_db: 12.0,
        detection_mode: "RMS".into(),
        sidechain_external: true,
        ..GatePluginParams::default()
    };
    let mut plugin = GatePlugin::try_from_params(1, params).unwrap();
    plugin.initialize(f64::from(rate)).unwrap();
    let mut rendered = Vec::with_capacity(frames);
    let mut start = 0;
    for &capacity in [1, 7, 113, 256].iter().cycle() {
        if start == frames {
            break;
        }
        let count = capacity.min(frames - start);
        let mut buffer = Vec::with_capacity(count * 2);
        for &sidechain in &detector[start..start + count] {
            buffer.extend([0.25, sidechain]);
        }
        plugin
            .process_in_place(&mut buffer, &ProcessContext::new(rate, count))
            .unwrap();
        for (index, frame) in buffer.as_chunks::<2>().0.iter().enumerate() {
            assert_eq!(frame[1], detector[start + index]);
            rendered.push(frame[0]);
        }
        start += count;
    }

    let attack = (-1.0 / (0.001 * f64::from(rate))).exp();
    let release = (-1.0 / (0.010 * f64::from(rate))).exp();
    let mut envelope = 0.0_f64;
    for (frame, &output) in rendered.iter().enumerate() {
        // Recompute from source samples: neither rolling subtraction nor tree
        // nodes participate in this independent detector reference.
        let first = (frame + 1).saturating_sub(window);
        let energy: f64 = detector[first..=frame]
            .iter()
            .map(|&sample| f64::from(sample).powi(2))
            .sum();
        let rms = (energy / window as f64).sqrt();
        let level = if rms == 0.0 {
            -240.0
        } else {
            20.0 * rms.log10()
        };
        let target = ((f64::from(ratio) - 1.0) * (level - f64::from(threshold)).max(0.0)).min(12.0);
        let coefficient = if target > envelope { attack } else { release };
        envelope = target + coefficient * (envelope - target);
        let expected = if mode == GateMode::Upward {
            envelope
        } else {
            -envelope
        };
        let actual = 20.0 * (f64::from(output) / 0.25).log10();
        // Matches the Gate's documented fast-log/pow settled-audio accuracy.
        assert!(
            (actual - expected).abs() < 0.02,
            "rate={rate} mode={mode:?} background={background} frame={frame}: gain {actual} vs {expected} dB"
        );
    }
}

#[test]
fn extreme_finite_rms_pulses_and_retained_quiet_energy_follow_the_public_gain_law() {
    for rate in [44_100, 48_000, 96_000] {
        for mode in [GateMode::Upward, GateMode::Duck] {
            // Small ratio reveals erroneous huge RMS levels without hiding
            // them behind the gain cap. Zero continuation verifies recovery.
            verify_gain(rate, mode, 0.0, -20.0, 1.01);
            // Quiet audio remains above threshold after the huge pulses leave.
            // A subtractive accumulator could lose it and report false silence.
            verify_gain(rate, mode, 0.1, -40.0, 4.0);
            verify_gain(rate, mode, 0.001, -80.0, 4.0);
        }
    }
}
