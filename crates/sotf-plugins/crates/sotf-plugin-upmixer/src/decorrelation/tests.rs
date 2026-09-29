//! Independent small-transform phase and finite all-pass checks.
// Rust guideline compliant 2026-02-21
use crate::{UpmixerPlugin, UpmixerPluginParams};
use rustfft::num_complex::Complex;
use sotf_host::Plugin;

fn stereo(n: usize) -> UpmixerPlugin {
    let params: UpmixerPluginParams = serde_json::from_value(serde_json::json!({
        "fft_size": n, "speaker_config": "2.0", "enable_hr_direct": false,
    }))
    .unwrap();
    let mut plugin = UpmixerPlugin::from_params(params);
    plugin.initialize(48_000).unwrap();
    plugin
}

#[test]
fn seeded_small_filter_matches_fixed_pulses_and_independent_complex_dft() {
    for (n, pulses) in [
        (64, &[(1, 1.0), (36, -1.0)][..]),
        (128, &[(1, 1.0), (36, -1.0), (57, 1.0)][..]),
    ] {
        let plugin = stereo(n);
        // Seed 85997 is the documented channel-four seed. These pulse fixtures
        // were derived independently using integer LCG steps and IEEE-f32
        // rounding. The second pulse lies past nominal N/2 for N=64: preserving
        // that existing cursor/offset behavior is intentional.
        let actual = plugin.generate_velvet_noise_filter_with_seed(85_997, n / 2 + 1);
        let length = n / 2;
        let fade_length = length / 4;
        let fade_start = length - fade_length;
        let mut impulse = vec![0.0_f64; n];
        for &(position, value) in pulses {
            let fade = if (fade_start..length).contains(&position) {
                0.5 * (1.0
                    + (std::f64::consts::PI * (position - fade_start) as f64 / fade_length as f64)
                        .cos())
            } else {
                1.0
            };
            impulse[position] = value * fade;
        }
        for (bin, value) in actual.iter().enumerate() {
            let mut expected = Complex::<f64>::new(0.0, 0.0);
            for (sample, &coefficient) in impulse.iter().enumerate() {
                let phase = -std::f64::consts::TAU * bin as f64 * sample as f64 / n as f64;
                expected += Complex::new(phase.cos(), phase.sin()) * coefficient;
            }
            if bin == 0 || bin == n / 2 || expected.norm() <= 1e-9 {
                expected = Complex::new(1.0, 0.0);
            } else {
                expected /= expected.norm();
            }
            let value = Complex::new(f64::from(value.re), f64::from(value.im));
            assert!((value.norm() - 1.0).abs() < 2e-6);
            assert!(
                (value - expected).norm() < 3e-5,
                "n={n}, bin={bin}: {value:?} != {expected:?}"
            );
        }
        assert_eq!(actual[0], Complex::new(1.0, 0.0));
        assert_eq!(actual[n / 2], Complex::new(1.0, 0.0));
    }
}

#[test]
fn empty_seeded_sequence_has_finite_identity_fallback() {
    let mut plugin = stereo(64);
    // Exercise the private generator's empty-sequence guard independently of
    // public parameter limits. The first pulse occurs beyond this short window.
    plugin.decorrelation.velvet_noise_density = 1.0;
    let filter = plugin.generate_velvet_noise_filter_with_seed(1_234, 33);
    assert!(filter.iter().all(|&value| value == Complex::new(1.0, 0.0)));
}
