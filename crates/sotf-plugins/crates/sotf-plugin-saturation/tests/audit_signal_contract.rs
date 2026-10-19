//! Independent signal checks for antiderivative processing and DC removal.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_saturation::{SaturationPlugin, SaturationPluginParams};

fn plugin(mode: &str, rate: u32, adaa: bool, dc: bool) -> SaturationPlugin {
    let mut plugin = SaturationPlugin::from_validated_params(
        1,
        SaturationPluginParams {
            mode: mode.into(),
            drive: 8.0,
            mix: 1.0,
            oversampling: "Off".into(),
            use_adaa: adaa,
            dc_blocker_enabled: dc,
            ..Default::default()
        },
    );
    plugin.initialize(f64::from(rate)).unwrap();
    plugin
}

#[test]
fn soft_clip_adaa_matches_numerical_interval_integral_from_cold_start() {
    // Simpson quadrature integrates the continuous waveshaper between adjacent
    // driven samples, independently of the implementation's antiderivative.
    let input: Vec<f32> = (0..4096)
        .map(|i| (0.7 * (i as f64 * 0.731).sin()) as f32)
        .collect();
    for rate in [44_100, 48_000, 96_000] {
        let mut processor = plugin("Soft Clip", rate, true, false);
        let mut actual = input.clone();
        processor
            .process_in_place(&mut actual, &ProcessContext::new(rate, input.len()))
            .unwrap();
        let mut previous = 0.0_f64;
        for (&sample, &output) in input.iter().zip(&actual) {
            let current = f64::from(sample) * 8.0;
            let mut sum = previous.tanh() + current.tanh();
            for step in 1..128 {
                let x = previous + (current - previous) * step as f64 / 128.0;
                sum += if step % 2 == 0 { 2.0 } else { 4.0 } * x.tanh();
            }
            let expected = sum / (3.0 * 128.0 * 8.0_f64.tanh());
            assert!(
                (f64::from(output) - expected).abs() < 2e-4,
                "rate={rate}, input={sample}: actual={output}, expected={expected}"
            );
            previous = current;
        }
        processor.reset();
        let mut restarted = input.clone();
        processor
            .process_in_place(&mut restarted, &ProcessContext::new(rate, input.len()))
            .unwrap();
        assert_eq!(actual, restarted, "reset must reproduce cold ADAA history");
    }
}

#[test]
fn dc_blocker_removes_steady_offset_for_every_mode_and_adaa_setting() {
    for rate in [44_100, 48_000, 96_000] {
        for mode in ["Soft Clip", "Tube", "Tape", "Exciter", "Asymmetric"] {
            for adaa in [false, true] {
                let mut processor = plugin(mode, rate, adaa, true);
                let mut elapsed = 0;
                let mut tail_peak = 0.0_f32;
                while elapsed < rate as usize * 2 {
                    let count = 257.min(rate as usize * 2 - elapsed);
                    let mut block = vec![0.2; count];
                    processor
                        .process_in_place(&mut block, &ProcessContext::new(rate, count))
                        .unwrap();
                    assert!(block.iter().all(|x| x.is_finite()));
                    if elapsed > rate as usize {
                        tail_peak = block.iter().map(|x| x.abs()).fold(tail_peak, f32::max);
                    }
                    elapsed += count;
                }
                assert!(
                    tail_peak < 1e-5,
                    "mode={mode}, rate={rate}, adaa={adaa}: steady DC={tail_peak}"
                );
            }
        }
    }
}
