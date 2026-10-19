//! Sustained streaming clocks checked without production position iterators.

// Rust guideline compliant 2026-02-21
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, ParameterId, ParameterValue, Plugin, ProcessContext};
use sotf_plugin_resampler::{ResamplerPlugin, ResamplerQuality};
use std::f64::consts::TAU;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

const CHUNK: usize = 1024;
const FREQUENCY: f64 = 997.0;

fn taps(quality: ResamplerQuality) -> usize {
    match quality {
        ResamplerQuality::Fast => 64,
        ResamplerQuality::Medium => 128,
        ResamplerQuality::High => 256,
    }
}

// A frozen two-quadrature fit absorbs only constant gain and group delay.
// Later windows cannot refit timing and thereby hide accumulated clock error.
#[derive(Default)]
struct ToneCheck {
    seen: usize,
    ss: f64,
    cc: f64,
    sc: f64,
    ys: f64,
    yc: f64,
    fit: Option<(f64, f64)>,
    error_energy: f64,
    maximum: f64,
    measured: usize,
}

impl ToneCheck {
    fn sample(&mut self, sample: f32, source_position: f64, input_rate: u32) {
        assert!(sample.is_finite());
        let (s, c) = (TAU * FREQUENCY * source_position / f64::from(input_rate)).sin_cos();
        self.seen += 1;
        if self.seen <= 4096 {
            return;
        }
        if self.seen <= 12_288 {
            self.ss += s * s;
            self.cc += c * c;
            self.sc += s * c;
            self.ys += f64::from(sample) * s;
            self.yc += f64::from(sample) * c;
            if self.seen == 12_288 {
                let determinant = self.ss * self.cc - self.sc * self.sc;
                let a = (self.ys * self.cc - self.yc * self.sc) / determinant;
                let b = (self.yc * self.ss - self.ys * self.sc) / determinant;
                assert!((a.hypot(b) - 0.25).abs() < 0.0003);
                self.fit = Some((a, b));
            }
        } else {
            let (a, b) = self.fit.unwrap();
            let error = f64::from(sample) - a * s - b * c;
            self.maximum = self.maximum.max(error.abs());
            self.error_energy += error * error;
            self.measured += 1;
        }
    }

    fn finish(&self, label: &str) -> f64 {
        assert!(self.measured > 1_000_000);
        let rms = (self.error_energy / self.measured as f64).sqrt();
        // Fixed -80 dBFS RMS / -70 dBFS peak requirements at the low-band
        // test tone, including table interpolation ripple and f32 arithmetic.
        assert!(rms < 1e-4, "{label}: rms {rms}");
        assert!(
            self.maximum < 0.000_316_228,
            "{label}: peak {}",
            self.maximum
        );
        rms
    }
}

fn make(input: u32, output: f64, quality: ResamplerQuality) -> ResamplerPlugin {
    let mut p = ResamplerPlugin::with_quality(1, input, output, CHUNK, quality).unwrap();
    p.initialize(f64::from(input)).unwrap();
    p
}

fn drain(p: &mut ResamplerPlugin, rate: u32) -> Vec<f32> {
    let mut scratch = vec![f32::NAN; p.drain_output_frames_max()];
    let mut tail = Vec::new();
    for _ in 0..32 {
        scratch.fill(f32::NAN);
        let mut result = None;
        assert_no_allocs_or_deallocs("long-stream drain", || {
            result = Some(
                p.drain(&mut scratch, &ProcessContext::new(rate, 0))
                    .unwrap(),
            );
        });
        let result = result.unwrap();
        assert!(scratch[..result.frames].iter().all(|x| x.is_finite()));
        assert!(scratch[result.frames..].iter().all(|x| x.is_nan()));
        tail.extend_from_slice(&scratch[..result.frames]);
        if result.complete {
            return tail;
        }
    }
    panic!("long stream did not drain");
}

// Exact binary64 multiplication and outward integer rounding using integers.
// All tested positive ratios are in [0.5, 2), with denominator <= 2^53.
// This does not use the production f64 frame-count formula.
fn binary_count(frames: usize, ratio: f64, ceil: bool) -> usize {
    assert!((0.5..2.0).contains(&ratio));
    let bits = ratio.to_bits();
    let numerator = u128::from((bits & ((1_u64 << 52) - 1)) | (1_u64 << 52));
    let denominator = 1_u128 << (1075 - ((bits >> 52) & 2047));
    let product = frames as u128 * numerator;
    if ceil {
        product.div_ceil(denominator) as usize
    } else {
        (product / denominator) as usize
    }
}

fn check_fixed_streams(seconds: usize) {
    for quality in [
        ResamplerQuality::Fast,
        ResamplerQuality::Medium,
        ResamplerQuality::High,
    ] {
        for (rate, output_rate) in [
            (44_100, 48_000.0),
            (48_000, 44_100.0),
            (48_000, 48_000.0 * 2.0_f64.sqrt()),
        ] {
            let mut p = make(rate, output_rate, quality);
            let ratio = output_rate / f64::from(rate);
            let source_frames = rate as usize * seconds + 37;
            let mut input = vec![0.0; CHUNK + 17];
            // Reserve for a callback plus a full pending residual chunk.
            let mut output = vec![f32::NAN; p.output_frames_for_input(input.len() + CHUNK)];
            let partitions = [1, CHUNK + 17, 7, 251, 509, CHUNK];
            let mut accepted = 0;
            let mut emitted = 0;
            let mut block = 0;
            let mut check = ToneCheck::default();
            while accepted < source_frames {
                let frames = partitions[block % partitions.len()].min(source_frames - accepted);
                for (i, x) in input[..frames].iter_mut().enumerate() {
                    *x = (0.25 * (TAU * FREQUENCY * (accepted + i) as f64 / f64::from(rate)).sin())
                        as f32;
                }
                output.fill(f32::NAN);
                let mut written = 0;
                assert_no_allocs_or_deallocs("long fixed stream", || {
                    written = p
                        .process(
                            &input[..frames],
                            &mut output,
                            &ProcessContext::new(rate, frames),
                        )
                        .unwrap();
                });
                for (i, &x) in output[..written].iter().enumerate() {
                    check.sample(x, (emitted + i + 1) as f64 / ratio, rate);
                }
                assert!(output[written..].iter().all(|x| x.is_nan()));
                emitted += written;
                accepted += frames;
                block += 1;
            }
            let rms = check.finish(&format!("{quality:?}/{rate}/{output_rate}"));
            emitted += drain(&mut p, rate).len();
            let expected = binary_count(source_frames, ratio, true)
                + binary_count(taps(quality) / 2, ratio, false);
            assert_eq!(emitted, expected, "{quality:?}/{rate}/{output_rate}");
            if output_rate == 48_000.0 || output_rate == 44_100.0 {
                let rational = (source_frames as u128 * output_rate as u128)
                    .div_ceil(u128::from(rate))
                    + (taps(quality) as u128 / 2 * output_rate as u128) / u128::from(rate);
                assert_eq!(emitted as u128, rational);
            }
            println!(
                "LONG_FIXED quality={quality:?} input_rate={rate} output_rate={output_rate:.12} input_frames={source_frames} output_frames={emitted} rms={rms:.12e} peak={:.12e}",
                check.maximum
            );
        }
    }
}

fn check_clock_drift(seconds: usize) {
    let rate = 48_000;
    for quality in [
        ResamplerQuality::Fast,
        ResamplerQuality::Medium,
        ResamplerQuality::High,
    ] {
        let mut actual = make(rate, f64::from(rate), quality);
        let mut partitioned = make(rate, f64::from(rate), quality);
        for p in [&mut actual, &mut partitioned] {
            p.set_parameter(
                ParameterId::from("dynamic_ratio"),
                ParameterValue::Bool(true),
            )
            .unwrap();
        }
        let blocks = (rate as usize * seconds).div_ceil(CHUNK);
        let source_frames = blocks * CHUNK;
        let mut input = vec![0.0; CHUNK];
        let capacity = actual.output_frames_for_input(CHUNK);
        let mut output = vec![f32::NAN; capacity];
        let mut split = vec![f32::NAN; capacity];
        let mut joined = Vec::with_capacity(capacity);
        let mut check = ToneCheck::default();
        let mut anchor = -(taps(quality) as f64 - 1.0);
        let mut previous = 1.0;
        let mut emitted = 0;
        let mut endpoint = None;
        for block in 0..blocks {
            // Bipolar +/-100 ppm triangle with 4.27 s period, not a
            // constant ratio or one-way slew. Every block ramps its clock.
            let phase = (block % 200) as f64 / 100.0;
            let ppm = if phase < 1.0 {
                -100.0 + 200.0 * phase
            } else {
                300.0 - 200.0 * phase
            };
            let ratio = 1.0 + ppm * 1e-6;
            for (i, x) in input.iter_mut().enumerate() {
                *x = (0.25 * (TAU * FREQUENCY * (block * CHUNK + i) as f64 / f64::from(rate)).sin())
                    as f32;
            }
            output.fill(f32::NAN);
            joined.clear();
            assert_no_allocs_or_deallocs("clock drift controls and callbacks", || {
                actual.try_set_ratio(ratio, true).unwrap();
                partitioned.try_set_ratio(ratio, true).unwrap();
                let written = actual
                    .process(&input, &mut output, &ProcessContext::new(rate, CHUNK))
                    .unwrap();
                let mut offset = 0;
                let pattern = [1, 7, 127, 251];
                let mut part = 0;
                while offset < CHUNK {
                    let n = pattern[part % pattern.len()].min(CHUNK - offset);
                    let count = partitioned
                        .process(
                            &input[offset..offset + n],
                            &mut split,
                            &ProcessContext::new(rate, n),
                        )
                        .unwrap();
                    joined.extend_from_slice(&split[..count]);
                    offset += n;
                    part += 1;
                }
                assert_eq!(written, joined.len());
            });
            assert!(
                output[..joined.len()] == joined,
                "partition {quality:?}/{block}"
            );
            let count = joined.len();
            assert!(output[count..].iter().all(|x| x.is_nan()));
            let h0 = 1.0 / previous;
            let h1 = 1.0 / ratio;
            // Closed-form sum of a linearly changing reciprocal ratio.
            // Candidate block lengths have their own ramp denominator.
            let last = |n: usize| anchor + n as f64 * h0 + (h1 - h0) * (n + 1) as f64 / 2.0;
            let boundary = CHUNK as f64 - (taps(quality) + 1) as f64;
            assert!(last(count) <= boundary + 1e-7);
            assert!(last(count + 1) > boundary - 1e-7);
            for (i, &x) in output[..count].iter().enumerate() {
                let j = (i + 1) as f64;
                let position = (block * CHUNK) as f64
                    + anchor
                    + j * h0
                    + (h1 - h0) * j * (j + 1.0) / (2.0 * count as f64);
                check.sample(x, position, rate);
            }
            anchor = last(count) - CHUNK as f64;
            previous = ratio;
            emitted += count;
        }
        let rms = check.finish(&format!("drift {quality:?}"));
        let mut predicted = emitted;
        let mut origin = source_frames;
        // After the final ramp, the constant-clock crossing can be solved
        // directly. No backend sizing/position iterator supplies this count.
        for _ in 0..32 {
            let boundary = source_frames as f64 - (taps(quality) / 2) as f64 + 1.0;
            let remaining = ((boundary - origin as f64 - anchor) * previous)
                .ceil()
                .max(1.0) as usize;
            let support = CHUNK as f64 - (taps(quality) + 1) as f64;
            let available = ((support - anchor) * previous).floor().max(0.0) as usize;
            if remaining <= available {
                endpoint = Some(predicted + remaining);
                break;
            }
            predicted += available;
            anchor += available as f64 / previous - CHUNK as f64;
            origin += CHUNK;
        }
        let a = drain(&mut actual, rate);
        let b = drain(&mut partitioned, rate);
        assert!(a == b, "partitioned EOF waveform {quality:?}");
        assert_eq!(
            emitted + a.len(),
            endpoint.unwrap(),
            "drift endpoint {quality:?}"
        );
        println!(
            "LONG_DRIFT quality={quality:?} input_frames={source_frames} output_frames={} rms={rms:.12e} peak={:.12e}",
            emitted + a.len(),
            check.maximum
        );
    }
}

#[test]
fn minute_rational_and_irrational_streams_keep_counts_and_frozen_tone_phase() {
    check_fixed_streams(60);
}

#[test]
fn minute_clock_drift_preserves_quadratic_timestamps_partitions_and_endpoint() {
    check_clock_drift(60);
}

#[test]
#[ignore = "One-hour numerical soak; run explicitly for the DSP audit"]
fn hour_rational_and_irrational_streams_keep_counts_and_frozen_tone_phase() {
    check_fixed_streams(3600);
}

#[test]
#[ignore = "One-hour numerical soak; run explicitly for the DSP audit"]
fn hour_clock_drift_preserves_quadratic_timestamps_partitions_and_endpoint() {
    check_clock_drift(3600);
}
