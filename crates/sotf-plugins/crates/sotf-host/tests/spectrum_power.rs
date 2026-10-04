//! Independent finite-window spectrum power and endpoint calibration.

// Rust guideline compliant 2026-02-21
use sotf_host::analyzer::SpectrumData;
use sotf_host::analyzer_spectrum::{SpectrumAnalyzerPlugin, SpectrumConfig};
use sotf_host::{Plugin, ProcessContext};
use std::f64::consts::{FRAC_PI_2, TAU};
use std::sync::Arc;

const N: usize = 4096;

fn signal(tones: &[(usize, f64, f64)]) -> Vec<f32> {
    (0..N)
        .map(|n| {
            tones
                .iter()
                .map(|&(bin, amplitude, phase)| {
                    amplitude * (TAU * bin as f64 * n as f64 / N as f64 + phase).cos()
                })
                .sum::<f64>() as f32
        })
        .collect()
}

fn analyze(input: &[f32], channels: usize, rate: u32, maximum: f32) -> Arc<SpectrumData> {
    let mut plugin = SpectrumAnalyzerPlugin::with_config_at_sample_rate(
        channels,
        f64::from(rate),
        SpectrumConfig {
            num_bins: 100,
            min_freq: 10.0,
            max_freq: maximum,
            smoothing: 0.0,
        },
    )
    .unwrap();
    plugin.initialize(f64::from(rate)).unwrap();
    let mut output = vec![f32::NAN; input.len()];
    assert_eq!(
        plugin
            .process(input, &mut output, &ProcessContext::new(rate, N))
            .unwrap(),
        N
    );
    assert_eq!(input, output);
    plugin.get_data().unwrap().downcast().unwrap()
}

fn summed_band_db(data: &SpectrumData) -> f64 {
    10.0 * data
        .magnitudes
        .iter()
        .map(|&db| 10.0_f64.powf(f64::from(db) / 10.0))
        .sum::<f64>()
        .log10()
}

// Parseval gives the total windowed energy without using an FFT or production
// bin mapping/normalization. Twice weighted mean square preserves the existing
// full-scale interior sine reference. Fixtures have no DC/low-frequency energy.
fn independent_power_db(input: &[f32]) -> f64 {
    let (mut energy, mut window_power) = (0.0, 0.0);
    for (n, &sample) in input.iter().enumerate() {
        let window = 0.5 - 0.5 * (TAU * n as f64 / N as f64).cos();
        energy += (f64::from(sample) * window).powi(2);
        window_power += window * window;
    }
    10.0 * (2.0 * energy / window_power).log10()
}

#[test]
fn integrated_bands_match_independent_hann_weighted_energy() {
    let fixtures = [
        vec![(2048, 1.0, 0.0)],
        vec![(2048, 0.125, 0.4)],
        vec![(2047, 1.0, 0.0)],
        vec![(2047, 1.0, FRAC_PI_2)],
        vec![(2046, 0.3, 0.7)],
        vec![(128, 1.0, 0.0)],
        vec![(127, 0.25, 0.4), (2047, 0.3, 0.7), (2048, 0.1, 0.0)],
    ];
    for rate in [16_000, 32_000, 40_000] {
        for tones in &fixtures {
            let input = signal(tones);
            let data = analyze(&input, 1, rate, rate as f32 / 2.0);
            let expected = independent_power_db(&input);
            let actual = summed_band_db(&data);
            // Public display uses approximate f32 logarithms. This bound is
            // small relative to either endpoint defect and preserves that API.
            assert!(
                (actual - expected).abs() < 0.01,
                "rate={rate}, tones={tones:?}: band power {actual}, time-domain reference {expected}"
            );
        }
    }
}

#[test]
fn maximum_frequency_includes_its_fft_line_and_coherent_peak() {
    let rate = 40_000;
    let input = signal(&[(512, 1.0, 0.0)]);
    let data = analyze(&input, 1, rate, 5000.0);
    // A periodic-Hann coherent cosine has squared calibrated line amplitudes
    // 1/4, 1, 1/4. The upper sideband is outside this requested range. The center
    // and lower sideband retain 5/6 of the full ENBW-corrected band power.
    let expected = 10.0 * (5.0_f64 / 6.0).log10();
    assert!((summed_band_db(&data) - expected).abs() < 0.01);
    assert!(data.peak_magnitude.abs() < 0.01);
}

#[test]
fn nyquist_peak_and_channel_maximum_preserve_their_existing_conventions() {
    let input = signal(&[(2048, 1.0, 0.0)]);
    let mono = analyze(&input, 1, 40_000, 20_000.0);
    assert!(mono.peak_magnitude.abs() < 0.01);
    assert!((summed_band_db(&mono) - 10.0 * 2.0_f64.log10()).abs() < 0.01);
    for stereo in [
        input.iter().flat_map(|&x| [x, -x]).collect::<Vec<_>>(),
        input.iter().flat_map(|&x| [0.0, x]).collect::<Vec<_>>(),
    ] {
        let data = analyze(&stereo, 2, 40_000, 20_000.0);
        assert_eq!(data.magnitudes, mono.magnitudes);
        assert_eq!(data.peak_magnitude, mono.peak_magnitude);
    }
}
