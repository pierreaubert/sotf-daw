//! Measure the streamed impulse response against independent filter prototypes.

// Rust guideline compliant 2026-02-21

use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_linear_phase_eq::{BandConfig, LinearPhaseEqPlugin, LinearPhaseEqPluginParams};

fn magnitude(band: &BandConfig, frequency: f64, sample_rate: f64) -> f64 {
    if !band.active {
        return 1.0;
    }
    // Evaluate the continuous prototype after the bilinear frequency mapping.
    // This does not use the production biquad, its coefficients, or FIR design.
    // Prototypes: https://www.w3.org/TR/audio-eq-cookbook/
    let ratio = (std::f64::consts::PI * frequency / sample_rate).tan()
        / (std::f64::consts::PI * band.frequency / sample_rate).tan();
    let r2 = ratio * ratio;
    let a = 10.0_f64.powf(band.gain_db / 40.0);
    let common = (1.0 - r2).powi(2);
    match band.filter_type.as_str() {
        "Peak" => ((common + (a * ratio / band.q).powi(2))
            / (common + (ratio / (a * band.q)).powi(2)))
        .sqrt(),
        "Lowpass" => 1.0 / (common + (ratio / band.q).powi(2)).sqrt(),
        "Highpass" => r2 / (common + (ratio / band.q).powi(2)).sqrt(),
        "Lowshelf" => {
            a * (((a - r2).powi(2) + 2.0 * a * r2) / ((1.0 - a * r2).powi(2) + 2.0 * a * r2)).sqrt()
        }
        "Highshelf" => {
            a * (((1.0 - a * r2).powi(2) + 2.0 * a * r2) / ((a - r2).powi(2) + 2.0 * a * r2)).sqrt()
        }
        _ => panic!("unsupported reference filter"),
    }
}

fn impulse_response(
    bands: &[BandConfig],
    rate: u32,
    length_index: usize,
    phase: usize,
) -> (Vec<f32>, usize) {
    let taps = 1024 << length_index;
    let mut plugin = LinearPhaseEqPlugin::from_params(
        1,
        rate,
        LinearPhaseEqPluginParams {
            num_filters: bands.len(),
            fir_length_index: length_index,
            phase_mode_index: phase,
            auto_gain: false,
            mix: 1.0,
            filters: bands.to_vec(),
        },
    )
    .unwrap();
    let latency = plugin.latency_samples();
    let mut output = vec![0.0; taps + 128];
    output[0] = 1.0;
    let mut position = 0;
    for block in [1, 31, 127, 257, 7, 1024].into_iter().cycle() {
        if position == output.len() {
            break;
        }
        let end = (position + block).min(output.len());
        plugin
            .process_in_place(
                &mut output[position..end],
                &ProcessContext::new(rate, end - position),
            )
            .unwrap();
        position = end;
    }
    (output, latency)
}

fn response(impulse: &[f32], frequency: f64, rate: u32, alignment: usize) -> (f64, f64) {
    impulse
        .iter()
        .enumerate()
        .fold((0.0, 0.0), |(re, im), (n, &sample)| {
            let angle =
                std::f64::consts::TAU * frequency * (n as f64 - alignment as f64) / f64::from(rate);
            (
                re + f64::from(sample) * angle.cos(),
                im - f64::from(sample) * angle.sin(),
            )
        })
}

#[test]
fn all_filter_shapes_match_their_analytic_magnitude_and_linear_phase() {
    for rate in [44_100, 48_000, 96_000] {
        for shape in ["Peak", "Lowshelf", "Highshelf", "Lowpass", "Highpass"] {
            for gain in [-12.0, 12.0] {
                // Use a broad, well-resolved band to check the transfer rather
                // than demanding resolution below the finite FIR's bandwidth.
                let band = BandConfig {
                    filter_type: shape.into(),
                    frequency: f64::from(rate) / 8.0,
                    q: 1.0,
                    gain_db: gain,
                    active: true,
                };
                for phase in [0, 1] {
                    for length_index in [0, 3] {
                        let (impulse, latency) = impulse_response(
                            std::slice::from_ref(&band),
                            rate,
                            length_index,
                            phase,
                        );
                        for fraction in [0.025, 0.05, 0.1, 0.125, 0.2, 0.3, 0.4, 0.45] {
                            let frequency = f64::from(rate) * fraction;
                            let expected = magnitude(&band, frequency, f64::from(rate));
                            let (re, im) = response(&impulse, frequency, rate, latency);
                            let error_db = 20.0 * (re.hypot(im) / expected).log10();
                            // 0.05 dB bounds interpolation/window error for these
                            // broad targets, independently of the selected phase.
                            assert!(
                                error_db.abs() < 0.05,
                                "{shape} gain={gain} rate={rate} taps={} phase={phase} frequency={frequency}: magnitude error {error_db} dB",
                                1024 << length_index
                            );
                            if phase == 0 {
                                let phase_error = im.atan2(re).abs();
                                assert!(
                                    phase_error < 1e-4,
                                    "{shape} rate={rate} taps={} frequency={frequency}: phase error {phase_error} rad",
                                    1024 << length_index
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn combined_low_frequency_bands_match_the_product_of_analytic_responses() {
    let bands = [
        BandConfig {
            filter_type: "Peak".into(),
            frequency: 500.0,
            q: 2.0,
            gain_db: 9.0,
            active: true,
        },
        BandConfig {
            filter_type: "Peak".into(),
            frequency: 2000.0,
            q: 1.5,
            gain_db: -6.0,
            active: true,
        },
        BandConfig {
            filter_type: "Lowshelf".into(),
            frequency: 150.0,
            q: 1.0,
            gain_db: 3.0,
            active: true,
        },
    ];
    for rate in [44_100, 48_000, 96_000] {
        let (impulse, latency) = impulse_response(&bands, rate, 3, 0);
        for frequency in [
            50.0, 150.0, 250.0, 500.0, 750.0, 1000.0, 2000.0, 4000.0, 8000.0,
        ] {
            let expected: f64 = bands
                .iter()
                .map(|b| magnitude(b, frequency, f64::from(rate)))
                .product();
            let (re, im) = response(&impulse, frequency, rate, latency);
            let error_db = 20.0 * (re.hypot(im) / expected).log10();
            assert!(
                error_db.abs() < 0.05,
                "rate={rate} frequency={frequency}: magnitude error {error_db} dB"
            );
            assert!(
                im.atan2(re).abs() < 1e-4,
                "rate={rate} frequency={frequency}: unexpected phase"
            );
        }
    }
}
