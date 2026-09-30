// Rust guideline compliant 2026-02-21

use flate2::read::GzDecoder;
use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_plugin_band_split::BandSplitPlugin;
use std::fs::File;
use std::io::Read;
use std::path::Path;

const SAMPLE_RATE: u32 = 48_000;
const PROBE_HZ: f64 = 1_100.0;
const WARMUP_FRAMES: usize = 12_000;
const MEASURE_FRAMES: usize = 24_000;
const TOTAL_FRAMES: usize = WARMUP_FRAMES + MEASURE_FRAMES;
const COMPLEX_RESPONSE_TOLERANCE: f64 = 0.002;
const PHASE_COMPENSATION_MAGNITUDE_TOLERANCE: f64 = 0.005;

#[derive(Clone, Copy, Debug)]
struct Complex {
    re: f64,
    im: f64,
}

impl Complex {
    const ONE: Self = Self { re: 1.0, im: 0.0 };

    fn norm(self) -> f64 {
        self.re.hypot(self.im)
    }

    fn distance(self, rhs: Self) -> f64 {
        (self.re - rhs.re).hypot(self.im - rhs.im)
    }

    fn reciprocal(self) -> Self {
        let denominator = self.re * self.re + self.im * self.im;
        Self {
            re: self.re / denominator,
            im: -self.im / denominator,
        }
    }

    fn powu(self, exponent: u32) -> Self {
        (0..exponent).fold(Self::ONE, |product, _| product * self)
    }
}

impl std::ops::Add for Complex {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self {
            re: self.re + rhs.re,
            im: self.im + rhs.im,
        }
    }
}

impl std::ops::Mul for Complex {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self::Output {
        Self {
            re: self.re * rhs.re - self.im * rhs.im,
            im: self.re * rhs.im + self.im * rhs.re,
        }
    }
}

fn low_high_response(slope: &str, probe_hz: f64, cutoff_hz: f64) -> (Complex, Complex) {
    let normalized = (std::f64::consts::PI * probe_hz / f64::from(SAMPLE_RATE)).tan()
        / (std::f64::consts::PI * cutoff_hz / f64::from(SAMPLE_RATE)).tan();
    let s = Complex {
        re: 0.0,
        im: normalized,
    };

    let (denominator, high_numerator_order) = match slope {
        "LR24" => {
            let butterworth_second_order = s * s
                + Complex { re: 1.0, im: 0.0 }
                + Complex {
                    re: 0.0,
                    im: std::f64::consts::SQRT_2 * normalized,
                };
            (butterworth_second_order * butterworth_second_order, 4)
        }
        "LR48" => {
            let q1 = 1.0 / (2.0 * (std::f64::consts::PI / 8.0).sin());
            let q2 = 1.0 / (2.0 * (3.0 * std::f64::consts::PI / 8.0).sin());
            let first_biquad = s * s
                + s * Complex {
                    re: 1.0 / q1,
                    im: 0.0,
                }
                + Complex::ONE;
            let second_biquad = s * s
                + s * Complex {
                    re: 1.0 / q2,
                    im: 0.0,
                }
                + Complex::ONE;
            let butterworth_fourth_order = first_biquad * second_biquad;
            (butterworth_fourth_order * butterworth_fourth_order, 8)
        }
        _ => panic!("unsupported slope {slope}"),
    };

    let low = denominator.reciprocal();
    let high = s.powu(high_numerator_order) * low;
    (low, high)
}

fn legacy_band_responses(slope: &str, cutoffs_hz: &[f64], probe_hz: f64) -> Vec<Complex> {
    let mut carry = Complex::ONE;
    let mut bands = Vec::with_capacity(cutoffs_hz.len() + 1);
    for &cutoff_hz in cutoffs_hz {
        let (low, high) = low_high_response(slope, probe_hz, cutoff_hz);
        bands.push(carry * low);
        carry = carry * high;
    }
    bands.push(carry);
    bands
}

fn sine_input() -> Vec<f32> {
    (0..TOTAL_FRAMES)
        .flat_map(|frame| {
            let time = frame as f64 / f64::from(SAMPLE_RATE);
            let phase = std::f64::consts::TAU * PROBE_HZ * time;
            [
                (0.31 * (phase + 0.15).sin()) as f32,
                (0.23 * (phase + 1.05).sin()) as f32,
            ]
        })
        .collect()
}

fn project_interleaved(
    samples: &[f32],
    channels: usize,
    channel: usize,
    start_frame: usize,
    frames: usize,
) -> Complex {
    let omega = std::f64::consts::TAU * PROBE_HZ / f64::from(SAMPLE_RATE);
    let mut projection = Complex { re: 0.0, im: 0.0 };
    for frame in start_frame..start_frame + frames {
        let sample = f64::from(samples[frame * channels + channel]);
        let phase = omega * frame as f64;
        projection.re += sample * phase.cos();
        projection.im -= sample * phase.sin();
    }
    let scale = 2.0 / frames as f64;
    Complex {
        re: projection.re * scale,
        im: projection.im * scale,
    }
}

fn measured_band_responses(output: &[f32], bands: usize, input: &[f32]) -> Vec<[Complex; 2]> {
    let input_left = project_interleaved(input, 2, 0, WARMUP_FRAMES, MEASURE_FRAMES);
    let input_right = project_interleaved(input, 2, 1, WARMUP_FRAMES, MEASURE_FRAMES);
    (0..bands)
        .map(|band| {
            let left = project_band(output, bands, band, 0) * input_left.reciprocal();
            let right = project_band(output, bands, band, 1) * input_right.reciprocal();
            [left, right]
        })
        .collect()
}

fn project_band(output: &[f32], bands: usize, band: usize, channel: usize) -> Complex {
    let omega = std::f64::consts::TAU * PROBE_HZ / f64::from(SAMPLE_RATE);
    let mut projection = Complex { re: 0.0, im: 0.0 };
    for frame in WARMUP_FRAMES..WARMUP_FRAMES + MEASURE_FRAMES {
        let sample = f64::from(output[frame * bands * 2 + band * 2 + channel]);
        let phase = omega * frame as f64;
        projection.re += sample * phase.cos();
        projection.im -= sample * phase.sin();
    }
    let scale = 2.0 / MEASURE_FRAMES as f64;
    Complex {
        re: projection.re * scale,
        im: projection.im * scale,
    }
}

fn append_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn append_f32s(bytes: &mut Vec<u8>, samples: &[f32]) {
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
}

fn read_gzip_fixture(relative_path: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative_path);
    let file = File::open(&path)
        .unwrap_or_else(|error| panic!("failed to open fixture {}: {error}", path.display()));
    let mut decoded = Vec::new();
    GzDecoder::new(file)
        .read_to_end(&mut decoded)
        .unwrap_or_else(|error| panic!("failed to decompress {}: {error}", path.display()));
    decoded
}

fn append_string(bytes: &mut Vec<u8>, value: &str) {
    append_u32(bytes, value.len() as u32);
    bytes.extend_from_slice(value.as_bytes());
}

#[derive(Clone, Copy)]
struct BaselineCase {
    slope: &'static str,
    name: &'static str,
    cutoffs_hz: &'static [f64],
}

const BASELINE_CASES: &[BaselineCase] = &[
    BaselineCase {
        slope: "LR24",
        name: "two-band-control",
        cutoffs_hz: &[1_000.0],
    },
    BaselineCase {
        slope: "LR48",
        name: "two-band-control",
        cutoffs_hz: &[1_000.0],
    },
    BaselineCase {
        slope: "LR24",
        name: "three-band-close",
        cutoffs_hz: &[1_000.0, 1_200.0],
    },
    BaselineCase {
        slope: "LR48",
        name: "three-band-close",
        cutoffs_hz: &[1_000.0, 1_200.0],
    },
    BaselineCase {
        slope: "LR24",
        name: "three-band-wide",
        cutoffs_hz: &[500.0, 2_000.0],
    },
    BaselineCase {
        slope: "LR48",
        name: "three-band-wide",
        cutoffs_hz: &[500.0, 2_000.0],
    },
    BaselineCase {
        slope: "LR24",
        name: "four-band-close",
        cutoffs_hz: &[1_000.0, 1_100.0, 1_200.0],
    },
    BaselineCase {
        slope: "LR48",
        name: "four-band-close",
        cutoffs_hz: &[1_000.0, 1_100.0, 1_200.0],
    },
    BaselineCase {
        slope: "LR24",
        name: "four-band-wide",
        cutoffs_hz: &[250.0, 1_000.0, 4_000.0],
    },
    BaselineCase {
        slope: "LR48",
        name: "four-band-wide",
        cutoffs_hz: &[250.0, 1_000.0, 4_000.0],
    },
];

#[test]
fn captures_public_legacy_complex_controls_before_phase_changes() {
    let input = sine_input();
    let input_left = project_interleaved(&input, 2, 0, WARMUP_FRAMES, MEASURE_FRAMES);
    let input_right = project_interleaved(&input, 2, 1, WARMUP_FRAMES, MEASURE_FRAMES);
    let capture_path =
        std::env::var_os("SOTF_AUD143_SPLIT_CAPTURE_PATH").map(std::path::PathBuf::from);
    let mut capture = Vec::new();
    capture.extend_from_slice(b"SOTF-AUD143-SPLIT-WAVEFORMS\0");
    append_u32(&mut capture, 1);
    append_u32(&mut capture, SAMPLE_RATE);
    append_u32(&mut capture, TOTAL_FRAMES as u32);
    append_u32(&mut capture, WARMUP_FRAMES as u32);
    append_u32(&mut capture, MEASURE_FRAMES as u32);
    append_f32s(&mut capture, &input);
    append_u32(&mut capture, BASELINE_CASES.len() as u32);

    for case in BASELINE_CASES {
        let bands = case.cutoffs_hz.len() + 1;
        let mut plugin = BandSplitPlugin::new_multiband(2, case.cutoffs_hz, case.slope).unwrap();
        plugin.initialize(SAMPLE_RATE).unwrap();
        let mut output = vec![0.0_f32; TOTAL_FRAMES * bands * 2];
        let processed = plugin
            .process(
                &input,
                &mut output,
                &ProcessContext::new(SAMPLE_RATE, TOTAL_FRAMES),
            )
            .unwrap();
        assert_eq!(processed, TOTAL_FRAMES);
        assert!(output.iter().all(|sample| sample.is_finite()));

        let measured = measured_band_responses(&output, bands, &input);
        let expected = legacy_band_responses(case.slope, case.cutoffs_hz, PROBE_HZ);
        assert_eq!(measured.len(), expected.len());

        let mut measured_sum = Complex { re: 0.0, im: 0.0 };
        let expected_sum = expected
            .iter()
            .copied()
            .fold(Complex { re: 0.0, im: 0.0 }, |sum, band| sum + band);
        for (band, (&actual, &reference)) in measured.iter().zip(expected.iter()).enumerate() {
            assert!(
                actual[0].distance(reference) <= COMPLEX_RESPONSE_TOLERANCE,
                "{} {} band {} left response differs: measured {:?}, reference {:?}",
                case.slope,
                case.name,
                band,
                actual[0],
                reference
            );
            assert!(
                actual[1].distance(reference) <= COMPLEX_RESPONSE_TOLERANCE,
                "{} {} band {} right response differs: measured {:?}, reference {:?}",
                case.slope,
                case.name,
                band,
                actual[1],
                reference
            );
            measured_sum = measured_sum + actual[0];
        }

        let measured_sum_right = measured
            .iter()
            .map(|band| band[1])
            .fold(Complex { re: 0.0, im: 0.0 }, |sum, band| sum + band);
        assert!(
            measured_sum.distance(expected_sum) <= COMPLEX_RESPONSE_TOLERANCE,
            "{} {} left sum differs: measured {:?}, reference {:?}",
            case.slope,
            case.name,
            measured_sum,
            expected_sum
        );
        assert!(
            measured_sum_right.distance(expected_sum) <= COMPLEX_RESPONSE_TOLERANCE,
            "{} {} right sum differs: measured {:?}, reference {:?}",
            case.slope,
            case.name,
            measured_sum_right,
            expected_sum
        );

        if bands == 2 {
            assert!((measured_sum.norm() - 1.0).abs() <= COMPLEX_RESPONSE_TOLERANCE);
        }

        let (mut left_power, mut right_power, mut stereo_difference_power) = (0.0, 0.0, 0.0);
        for frame in WARMUP_FRAMES..WARMUP_FRAMES + MEASURE_FRAMES {
            let frame_offset = frame * bands * 2;
            let left_sum = (0..bands)
                .map(|band| output[frame_offset + band * 2])
                .sum::<f32>();
            let right_sum = (0..bands)
                .map(|band| output[frame_offset + band * 2 + 1])
                .sum::<f32>();
            left_power += f64::from(left_sum * left_sum);
            right_power += f64::from(right_sum * right_sum);
            stereo_difference_power += f64::from((left_sum - right_sum).powi(2));
        }
        let left_rms = (left_power / MEASURE_FRAMES as f64).sqrt();
        let right_rms = (right_power / MEASURE_FRAMES as f64).sqrt();
        let stereo_difference_rms = (stereo_difference_power / MEASURE_FRAMES as f64).sqrt();
        assert!(left_rms > 0.001 && right_rms > 0.001);
        assert!(stereo_difference_rms > 0.001);
        assert!(measured_sum.norm() > 0.01 && measured_sum_right.norm() > 0.01);
        assert!(input_left.norm() > 0.1 && input_right.norm() > 0.1);

        println!(
            "AUD143_CAPTURE slope={} case={} cuts_hz={:?} probe_hz={:.1} expected_sum={:?} measured_sum_left={:?} measured_sum_right={:?}",
            case.slope,
            case.name,
            case.cutoffs_hz,
            PROBE_HZ,
            expected_sum,
            measured_sum,
            measured_sum_right
        );
        for (band, responses) in measured.iter().enumerate() {
            println!(
                "AUD143_BAND slope={} case={} band={} left={:?} right={:?}",
                case.slope, case.name, band, responses[0], responses[1]
            );
        }

        append_string(&mut capture, case.slope);
        append_string(&mut capture, case.name);
        append_u32(&mut capture, case.cutoffs_hz.len() as u32);
        for cutoff in case.cutoffs_hz {
            capture.extend_from_slice(&cutoff.to_le_bytes());
        }
        append_u32(&mut capture, bands as u32);
        append_u32(&mut capture, output.len() as u32);
        append_f32s(&mut capture, &output);
    }

    let legacy_fixture = read_gzip_fixture("tests/data/aud143/legacy-split-48k-stereo-v1.bin.gz");
    if capture != legacy_fixture {
        let first_difference = capture
            .iter()
            .zip(&legacy_fixture)
            .position(|(actual, expected)| actual != expected)
            .unwrap_or(capture.len().min(legacy_fixture.len()));
        panic!(
            "current legacy audio differs from checked-in fixture at byte {first_difference} (actual length {}, fixture length {})",
            capture.len(),
            legacy_fixture.len()
        );
    }

    if let Some(path) = capture_path {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, &capture)
            .unwrap_or_else(|error| panic!("failed to write {}: {error}", path.display()));
        println!(
            "AUD143_WAVEFORM_ARTIFACT path={} bytes={} cases={}",
            path.display(),
            capture.len(),
            BASELINE_CASES.len()
        );
    }
}

fn assert_phase_compensation_magnitude_gate(slope: &str) {
    let cutoffs_hz = [1_000.0, 1_200.0];
    let bands = cutoffs_hz.len() + 1;
    let input = sine_input();
    let mut plugin = BandSplitPlugin::new_multiband(2, &cutoffs_hz, slope).unwrap();
    plugin.initialize(SAMPLE_RATE).unwrap();
    let mut output = vec![0.0_f32; TOTAL_FRAMES * bands * 2];
    plugin
        .process(
            &input,
            &mut output,
            &ProcessContext::new(SAMPLE_RATE, TOTAL_FRAMES),
        )
        .unwrap();

    let actual_bands = measured_band_responses(&output, bands, &input);
    let measured_sum = actual_bands
        .iter()
        .map(|band| band[0])
        .fold(Complex { re: 0.0, im: 0.0 }, |sum, band| sum + band);
    println!(
        "AUD143_EXPECTED_RED slope={} cuts_hz={:?} probe_hz={:.1} measured_sum={:?} magnitude={:.9}",
        slope,
        cutoffs_hz,
        PROBE_HZ,
        measured_sum,
        measured_sum.norm()
    );
    assert!(
        (measured_sum.norm() - 1.0).abs() <= PHASE_COMPENSATION_MAGNITUDE_TOLERANCE,
        "{slope} phase compensation gate failed: summed magnitude was {:.6}, expected 1.0 ± {:.3}",
        measured_sum.norm(),
        PHASE_COMPENSATION_MAGNITUDE_TOLERANCE
    );
}

#[test]
#[ignore = "intentional pre-fix red gate; run explicitly to record the current LR24 result"]
fn aud143_lr24_three_band_unity_magnitude_gate() {
    assert_phase_compensation_magnitude_gate("LR24");
}

#[test]
#[ignore = "intentional pre-fix red gate; run explicitly to record the current LR48 result"]
fn aud143_lr48_three_band_unity_magnitude_gate() {
    assert_phase_compensation_magnitude_gate("LR48");
}
