//! Independent final-PCM spectra, sub-LSB linearity, and overload recovery.
//!
//! FFT convention: unnormalized forward DFT. Averaged Hann periodograms are
//! divided by window energy, yielding two-sided discrete PSD in LSB squared.
//! Band means compare like units; no one-sided factor or Hz density is mixed in.

// Rust guideline compliant 2026-02-21
use rustfft::{FftPlanner, num_complex::Complex};
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_dither::{DitherPlugin, DitherPluginParams};

const RATES: [u32; 12] = [
    8000, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000, 384000, 768000,
];

fn render(input: &[f32], rate: u32, depth: usize, shaping: bool, dither_type: usize) -> Vec<f32> {
    let mut plugin = DitherPlugin::from_params(
        1,
        DitherPluginParams {
            bit_depth: depth,
            noise_shaping: shaping,
            dither_type,
            seed: 137,
        },
    );
    plugin.initialize(f64::from(rate)).unwrap();
    let mut output = input.to_vec();
    for block in output.chunks_mut(8191) {
        let frames = block.len();
        plugin
            .process_in_place(block, &ProcessContext::new(rate, frames))
            .unwrap();
    }
    output
}

fn expected_psd(rate: u32, frequency: f64, tpdf: bool) -> f64 {
    // Independent published FIR polynomial. Linear interpolation implements a
    // convex combination of adjacent integer-delay phasors, not exp(-j*w*d).
    let scale = (f64::from(rate) / 44100.0).max(1.0);
    let omega = std::f64::consts::TAU * frequency / f64::from(rate);
    let mut ntf = Complex::new(1.0, 0.0);
    for (index, coefficient) in [1.623, -0.982, 0.109].into_iter().enumerate() {
        let delay = (index + 1) as f64 * scale;
        let lower = delay.floor();
        let fraction = delay - lower;
        let phasor = Complex::from_polar(1.0 - fraction, -omega * lower)
            + Complex::from_polar(fraction, -omega * (lower + 1.0));
        ntf -= coefficient * phasor;
    }
    // Quantizer error variance is 1/12 LSB². Explicit nonsubtractive TPDF
    // contributes 1/6 LSB² outside this feedback loop; it is not itself shaped.
    ntf.norm_sqr() / 12.0 + if tpdf { 1.0 / 6.0 } else { 0.0 }
}

#[test]
fn final_pcm_noise_psd_matches_the_rate_scaled_transfer_polynomial() {
    const FFT: usize = 4096;
    const SEGMENTS: usize = 64;
    const PREFIX: usize = 1024;
    let frames = PREFIX + FFT * SEGMENTS;
    // Independent LCG programme has many sub-LSB quantizer phases, is far
    // from the rails, and shares neither PRNG nor seed with production dither.
    let mut state = 0x1234_5678_9876_5432_u64;
    let input: Vec<f32> = (0..frames)
        .map(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (((state >> 32) as f64 / u32::MAX as f64 - 0.5) * 0.007) as f32
        })
        .collect();
    let window: Vec<f64> = (0..FFT)
        .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / FFT as f64).cos())
        .collect();
    let window_energy: f64 = window.iter().map(|x| x * x).sum();
    let fft = FftPlanner::<f64>::new().plan_fft_forward(FFT);
    let mut bins = vec![Complex::new(0.0, 0.0); FFT];
    let mut worst_error_db = 0.0_f64;
    for rate in RATES {
        for depth in 0..3 {
            let lsb_scale = 2.0_f64.powi([16, 20, 24][depth] - 1);
            for tpdf in [false, true] {
                let output = render(&input, rate, depth, true, if tpdf { 0 } else { 1 });
                let mut spectrum = vec![0.0; FFT / 2 + 1];
                for segment in 0..SEGMENTS {
                    for i in 0..FFT {
                        let offset = PREFIX + segment * FFT + i;
                        bins[i] = Complex::new(
                            (f64::from(output[offset]) - f64::from(input[offset]))
                                * lsb_scale
                                * window[i],
                            0.0,
                        );
                    }
                    fft.process(&mut bins);
                    for (power, bin) in spectrum.iter_mut().zip(&bins) {
                        *power += bin.norm_sqr() / (window_energy * SEGMENTS as f64);
                    }
                }
                let bands = if rate >= 44100 {
                    vec![
                        (300.0, 4000.0),
                        (4000.0, 8000.0),
                        (8000.0, 12000.0),
                        (12000.0, 16000.0),
                        (16000.0, 20000.0),
                    ]
                } else {
                    [(0.02, 0.10), (0.10, 0.20), (0.20, 0.35), (0.35, 0.48)]
                        .into_iter()
                        .map(|(a, b)| (a * f64::from(rate), b * f64::from(rate)))
                        .collect()
                };
                for (low, high) in bands {
                    let mut measured = 0.0;
                    let mut expected = 0.0;
                    let mut count = 0;
                    for (bin, &power) in spectrum.iter().enumerate() {
                        let frequency = bin as f64 * f64::from(rate) / FFT as f64;
                        if frequency >= low && frequency < high {
                            measured += power;
                            expected += expected_psd(rate, frequency, tpdf);
                            count += 1;
                        }
                    }
                    assert!(count >= 16, "insufficient independent band bins");
                    let error_db = 10.0 * (measured / expected).log10();
                    worst_error_db = worst_error_db.max(error_db.abs());
                    // 64 records and at least 16 bins provide ample averaging;
                    // 0.5 dB also covers spectral smoothing by the Hann window.
                    assert!(
                        error_db.abs() < 0.5,
                        "rate={rate}, depth={depth}, TPDF={tpdf}, band={low}..{high}: PSD error={error_db} dB"
                    );
                }
            }
        }
    }
    println!("DITHER-SPECTRAL worst band PSD error {worst_error_db} dB");
}

#[test]
fn sub_lsb_sines_preserve_amplitude_phase_and_low_order_harmonics() {
    const FRAMES: usize = 65536;
    let mut worst = 0.0_f64;
    for rate in RATES {
        let bin = (997.0 * FRAMES as f64 / f64::from(rate)).round() as usize;
        let basis: Vec<_> = (0..FRAMES)
            .map(|i| (std::f64::consts::TAU * bin as f64 * i as f64 / FRAMES as f64).sin_cos())
            .collect();
        for depth in 0..3 {
            let scale = 2.0_f64.powi([16, 20, 24][depth] - 1);
            for amplitude in [0.1, 0.5, 2.0] {
                let input: Vec<f32> = basis
                    .iter()
                    .map(|&(s, _)| (amplitude * s / scale) as f32)
                    .collect();
                for shaping in [false, true] {
                    let output = render(&input, rate, depth, shaping, 0);
                    let mut coefficients = [0.0_f64; 6];
                    let mut mean = 0.0;
                    for (&sample, &(s, c)) in output.iter().zip(&basis) {
                        let value = f64::from(sample) * scale;
                        mean += value;
                        // Angle-doubling/tripling gives independent coherent
                        // projections, without measuring the input's FFT bins.
                        let s2 = 2.0 * s * c;
                        let c2 = c * c - s * s;
                        let s3 = s2 * c + c2 * s;
                        let c3 = c2 * c - s2 * s;
                        for (sum, component) in coefficients.iter_mut().zip([s, c, s2, c2, s3, c3])
                        {
                            *sum += value * component * 2.0 / FRAMES as f64;
                        }
                    }
                    coefficients[0] -= amplitude;
                    let error = coefficients.into_iter().map(f64::abs).fold(0.0, f64::max);
                    worst = worst.max(error);
                    // Absolute LSB bounds avoid dividing by a nearly zero
                    // sub-LSB input. A round-only quantizer fails these checks.
                    assert!(
                        error < 0.02 && (mean / FRAMES as f64).abs() < 0.01,
                        "rate={rate}, depth={depth}, amplitude={amplitude}, shaping={shaping}: coherent error={error} LSB"
                    );
                }
            }
        }
    }
    println!("DITHER-LINEARITY worst coherent error {worst} LSB");
}

#[test]
fn overload_clamps_to_pcm_rails_and_recovers_without_polluting_noise_statistics() {
    for rate in RATES {
        for depth in 0..3 {
            let scale = 2.0_f32.powi([16, 20, 24][depth] - 1);
            for shaping in [false, true] {
                let mut input = vec![0.0; (rate as usize / 20).max(4096) + 513];
                for (i, x) in input[..513].iter_mut().enumerate() {
                    *x = if i % 2 == 0 { 2.0 } else { -2.0 };
                }
                let output = render(&input, rate, depth, shaping, 0);
                assert_eq!(output[0], 1.0 - 1.0 / scale);
                assert!(
                    output
                        .iter()
                        .all(|&x| x.is_finite() && (-1.0..=1.0 - 1.0 / scale).contains(&x))
                );
                assert!(
                    output[output.len() - 1024..]
                        .iter()
                        .all(|&x| (x * scale).abs() <= 8.0),
                    "rate={rate}, depth={depth}, shaping={shaping}: overload feedback did not settle"
                );
            }
        }
    }
}

#[test]
fn wrong_callback_rate_cannot_advance_rng_or_shaping_history() {
    let params = DitherPluginParams {
        bit_depth: 0,
        noise_shaping: true,
        dither_type: 0,
        seed: 137,
    };
    let mut tested = DitherPlugin::from_params(1, params.clone());
    let mut reference = DitherPlugin::from_params(1, params);
    tested.initialize(48_000.0).unwrap();
    reference.initialize(48_000.0).unwrap();
    let mut history = vec![0.013; 511];
    let mut same = history.clone();
    tested
        .process_in_place(&mut history, &ProcessContext::new(48_000, 511))
        .unwrap();
    reference
        .process_in_place(&mut same, &ProcessContext::new(48_000, 511))
        .unwrap();
    let mut rejected = vec![0.2; 31];
    let before = rejected.clone();
    assert!(
        tested
            .process_in_place(&mut rejected, &ProcessContext::new(96_000, 31))
            .is_err()
    );
    assert_eq!(rejected, before);
    let mut actual = vec![0.0; 257];
    let mut expected = actual.clone();
    tested
        .process_in_place(&mut actual, &ProcessContext::new(48_000, 257))
        .unwrap();
    reference
        .process_in_place(&mut expected, &ProcessContext::new(48_000, 257))
        .unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn constructor_default_rate_is_prepared_like_explicit_initialization() {
    let params = DitherPluginParams {
        bit_depth: 0,
        noise_shaping: true,
        dither_type: 0,
        seed: 137,
    };
    let mut automatic = DitherPlugin::from_params(1, params.clone());
    let mut explicit = DitherPlugin::from_params(1, params);
    explicit.initialize(48_000.0).unwrap();
    let mut a: Vec<f32> = (0..4096)
        .map(|i| 0.0001 * (i as f32 * 0.713).sin())
        .collect();
    let mut b = a.clone();
    let context = ProcessContext::new(48_000, 4096);
    automatic.process_in_place(&mut a, &context).unwrap();
    explicit.process_in_place(&mut b, &context).unwrap();
    assert!(
        a == b,
        "constructor declared 48 kHz but used an unscaled 44.1 kHz shaper"
    );
}
