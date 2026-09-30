// Rust guideline compliant 2026-02-21

use flate2::read::GzDecoder;
use sotf_plugins::{
    BandMergePlugin, BandSplitPlugin, CountingAlloc, DawHost, Plugin, ProcessContext,
    assert_no_allocs,
};
use std::fs::File;
use std::io::Read;
use std::path::Path;

#[global_allocator]
static TEST_ALLOCATOR: CountingAlloc = CountingAlloc;

const SAMPLE_RATE: u32 = 48_000;
const PROBE_HZ: f64 = 1_100.0;
const WARMUP_FRAMES: usize = 12_000;
const MEASURE_FRAMES: usize = 24_000;
const TOTAL_FRAMES: usize = WARMUP_FRAMES + MEASURE_FRAMES;
const RESPONSE_TOLERANCE: f64 = 0.002;

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

fn low_high_response(slope: &str, cutoff_hz: f64) -> (Complex, Complex) {
    let normalized = (std::f64::consts::PI * PROBE_HZ / f64::from(SAMPLE_RATE)).tan()
        / (std::f64::consts::PI * cutoff_hz / f64::from(SAMPLE_RATE)).tan();
    let s = Complex {
        re: 0.0,
        im: normalized,
    };
    let (denominator, high_numerator_order) = match slope {
        "LR24" => {
            let second_order = s * s
                + Complex::ONE
                + Complex {
                    re: 0.0,
                    im: std::f64::consts::SQRT_2 * normalized,
                };
            (second_order * second_order, 4)
        }
        "LR48" => {
            let q1 = 1.0 / (2.0 * (std::f64::consts::PI / 8.0).sin());
            let q2 = 1.0 / (2.0 * (3.0 * std::f64::consts::PI / 8.0).sin());
            let first = s * s
                + s * Complex {
                    re: 1.0 / q1,
                    im: 0.0,
                }
                + Complex::ONE;
            let second = s * s
                + s * Complex {
                    re: 1.0 / q2,
                    im: 0.0,
                }
                + Complex::ONE;
            let butterworth4 = first * second;
            (butterworth4 * butterworth4, 8)
        }
        _ => panic!("unsupported slope {slope}"),
    };
    let low = denominator.reciprocal();
    (low, s.powu(high_numerator_order) * low)
}

fn legacy_sum(slope: &str, cutoffs_hz: &[f64]) -> Complex {
    let mut carry = Complex::ONE;
    let mut sum = Complex { re: 0.0, im: 0.0 };
    for &cutoff_hz in cutoffs_hz {
        let (low, high) = low_high_response(slope, cutoff_hz);
        sum = sum + carry * low;
        carry = carry * high;
    }
    sum + carry
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

fn project(samples: &[f32], channel: usize) -> Complex {
    let omega = std::f64::consts::TAU * PROBE_HZ / f64::from(SAMPLE_RATE);
    let mut projection = Complex { re: 0.0, im: 0.0 };
    for frame in WARMUP_FRAMES..WARMUP_FRAMES + MEASURE_FRAMES {
        let sample = f64::from(samples[frame * 2 + channel]);
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

fn append_string(bytes: &mut Vec<u8>, value: &str) {
    append_u32(bytes, value.len() as u32);
    bytes.extend_from_slice(value.as_bytes());
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

#[test]
fn dawhost_band_split_merge_chain_matches_legacy_complex_capture() {
    let cutoffs_hz = [1_000.0, 1_200.0];
    let input = sine_input();
    let input_left = project(&input, 0);
    let input_right = project(&input, 1);
    let capture_path =
        std::env::var_os("SOTF_AUD143_HOST_CAPTURE_PATH").map(std::path::PathBuf::from);
    let mut capture = Vec::new();
    capture.extend_from_slice(b"SOTF-AUD143-DAWHOST-WAVEFORMS\0");
    append_u32(&mut capture, 1);
    append_u32(&mut capture, SAMPLE_RATE);
    append_u32(&mut capture, TOTAL_FRAMES as u32);
    append_u32(&mut capture, WARMUP_FRAMES as u32);
    append_u32(&mut capture, MEASURE_FRAMES as u32);
    capture.extend_from_slice(&PROBE_HZ.to_le_bytes());
    append_f32s(&mut capture, &input);
    append_u32(&mut capture, 2);

    for slope in ["LR24", "LR48"] {
        let mut host = DawHost::new(2, SAMPLE_RATE);
        let split = BandSplitPlugin::new_multiband(2, &cutoffs_hz, slope).unwrap();
        let merge = BandMergePlugin::new(2, cutoffs_hz.len() + 1).unwrap();
        host.add_plugin(Box::new(split)).unwrap();
        host.add_plugin(Box::new(merge)).unwrap();
        assert_eq!(host.plugin_count(), 2);

        let mut output = vec![0.0_f32; TOTAL_FRAMES * 2];
        let processed = host.process(&input, &mut output).unwrap();
        assert_eq!(processed, TOTAL_FRAMES);
        assert!(output.iter().all(|sample| sample.is_finite()));

        let measured_left = project(&output, 0) * input_left.reciprocal();
        let measured_right = project(&output, 1) * input_right.reciprocal();
        let expected = legacy_sum(slope, &cutoffs_hz);

        assert!(
            measured_left.distance(expected) <= RESPONSE_TOLERANCE,
            "{slope} host left response differs: measured {measured_left:?}, reference {expected:?}"
        );
        assert!(
            measured_right.distance(expected) <= RESPONSE_TOLERANCE,
            "{slope} host right response differs: measured {measured_right:?}, reference {expected:?}"
        );
        assert!((measured_left.norm() - 1.0).abs() > 0.02);
        assert!((measured_right.norm() - 1.0).abs() > 0.02);
        assert!(input_left.distance(input_right) > 0.05);

        let (mut left_power, mut right_power, mut difference_power) = (0.0, 0.0, 0.0);
        for frame in WARMUP_FRAMES..WARMUP_FRAMES + MEASURE_FRAMES {
            let left = f64::from(output[frame * 2]);
            let right = f64::from(output[frame * 2 + 1]);
            left_power += left * left;
            right_power += right * right;
            difference_power += (left - right) * (left - right);
        }
        let left_rms = (left_power / MEASURE_FRAMES as f64).sqrt();
        let right_rms = (right_power / MEASURE_FRAMES as f64).sqrt();
        let difference_rms = (difference_power / MEASURE_FRAMES as f64).sqrt();
        assert!(left_rms > 0.01 && right_rms > 0.01);
        assert!(difference_rms > 0.001);

        println!(
            "AUD143_HOST_CAPTURE slope={slope} cuts_hz={cutoffs_hz:?} probe_hz={PROBE_HZ:.1} expected_sum={expected:?} measured_left={measured_left:?} measured_right={measured_right:?} magnitude={:.9}",
            measured_left.norm()
        );

        append_string(&mut capture, slope);
        append_u32(&mut capture, cutoffs_hz.len() as u32);
        for cutoff in cutoffs_hz {
            capture.extend_from_slice(&cutoff.to_le_bytes());
        }
        append_u32(&mut capture, output.len() as u32);
        append_f32s(&mut capture, &output);
    }

    let legacy_fixture = read_gzip_fixture(
        "crates/sotf-plugin-band-split/tests/data/aud143/legacy-host-48k-stereo-v1.bin.gz",
    );
    if capture != legacy_fixture {
        let first_difference = capture
            .iter()
            .zip(&legacy_fixture)
            .position(|(actual, expected)| actual != expected)
            .unwrap_or(capture.len().min(legacy_fixture.len()));
        panic!(
            "current DawHost audio differs from checked-in fixture at byte {first_difference} (actual length {}, fixture length {})",
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
            "AUD143_HOST_WAVEFORM_ARTIFACT path={} bytes={} slopes=2",
            path.display(),
            capture.len()
        );
    }
}

fn populated_split(slope: &str) -> BandSplitPlugin {
    let mut plugin = BandSplitPlugin::new_multiband(2, &[500.0, 1_200.0, 4_000.0], slope).unwrap();
    plugin.initialize(SAMPLE_RATE).unwrap();

    let frames = 4_096;
    let input: Vec<f32> = (0..frames)
        .flat_map(|frame| {
            let time = frame as f64 / f64::from(SAMPLE_RATE);
            let left = std::f64::consts::TAU * time;
            let right = std::f64::consts::TAU * time;
            [
                (0.20 * (left * 213.0).sin()
                    + 0.11 * (left * 1_700.0).sin()
                    + 0.07 * (left * 7_300.0).sin()) as f32,
                (0.18 * (right * 311.0).sin()
                    + 0.13 * (right * 2_300.0).sin()
                    + 0.05 * (right * 8_900.0).sin()) as f32,
            ]
        })
        .collect();
    let bands = 4;
    let output_channels = bands * 2;
    let mut output = vec![0.0_f32; frames * output_channels];
    let processed = plugin
        .process(
            &input,
            &mut output,
            &ProcessContext::new(SAMPLE_RATE, frames),
        )
        .unwrap();
    assert_eq!(processed, frames);
    assert!(output.iter().all(|sample| sample.is_finite()));
    for channel in 0..output_channels {
        let power = (0..frames)
            .map(|frame| {
                let sample = output[frame * output_channels + channel];
                sample * sample
            })
            .sum::<f32>();
        let rms = (power / frames as f32).sqrt();
        assert!(
            rms > 1e-5,
            "BandSplit output channel {channel} was not populated"
        );
    }

    plugin
}

#[test]
#[serial_test::serial]
fn aud143_populated_lr24_reset_has_zero_allocations_control() {
    let mut plugin = populated_split("LR24");
    assert_no_allocs("AUD143 populated LR24 BandSplit reset control", || {
        plugin.reset();
    });
}

#[test]
#[ignore = "intentional pre-fix red gate; run explicitly to record populated LR48 reset allocations"]
#[serial_test::serial]
fn aud143_populated_lr48_reset_is_allocation_free_gate() {
    let mut plugin = populated_split("LR48");
    assert_no_allocs("AUD143 populated LR48 BandSplit reset gate", || {
        plugin.reset();
    });
}
