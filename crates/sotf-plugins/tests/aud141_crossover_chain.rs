//! Stereo host-chain regression for the 2 -> 6 -> 2 LR24 multiway route.

use sotf_plugins::{BandMergePlugin, CrossoverPlugin, DawHost};

const SAMPLE_RATE: u32 = 48_000;
const WARMUP_FRAMES: usize = SAMPLE_RATE as usize;
const MEASURE_FRAMES: usize = 9_600;
const BLOCK_FRAMES: usize = 1_024;

#[derive(Clone, Copy, Debug)]
struct Complex {
    re: f64,
    im: f64,
}

impl Complex {
    fn norm(self) -> f64 {
        self.re.hypot(self.im)
    }

    fn distance(self, rhs: Self) -> f64 {
        (self.re - rhs.re).hypot(self.im - rhs.im)
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

fn lr24_allpass_at(probe_hz: f64, cutoff_hz: f64) -> Complex {
    let sample_rate = f64::from(SAMPLE_RATE);
    let t = (std::f64::consts::PI * probe_hz / sample_rate).tan()
        / (std::f64::consts::PI * cutoff_hz / sample_rate).tan();
    let denominator = Complex {
        re: 1.0 - t * t,
        im: std::f64::consts::SQRT_2 * t,
    };
    let denominator_squared = denominator * denominator;
    let norm_squared = denominator_squared.re.powi(2) + denominator_squared.im.powi(2);
    Complex {
        re: (1.0 + t.powi(4)) * denominator_squared.re / norm_squared,
        im: -(1.0 + t.powi(4)) * denominator_squared.im / norm_squared,
    }
}

fn project(signal: &[f32], start: usize, frames: usize, frequency_hz: f64) -> Complex {
    let omega = std::f64::consts::TAU * frequency_hz / f64::from(SAMPLE_RATE);
    let mut re = 0.0;
    let mut im = 0.0;
    for (offset, &sample) in signal[start..start + frames].iter().enumerate() {
        let phase = omega * (start + offset) as f64;
        re += f64::from(sample) * phase.cos();
        im -= f64::from(sample) * phase.sin();
    }
    let scale = 2.0 / frames as f64;
    Complex {
        re: re * scale,
        im: im * scale,
    }
}

fn sine_input(frames: usize) -> Vec<f32> {
    (0..frames)
        .flat_map(|frame| {
            let time = frame as f64 / f64::from(SAMPLE_RATE);
            let left_phase = std::f64::consts::TAU * 1_100.0 * time + 0.15;
            let right_phase = std::f64::consts::TAU * 1_700.0 * time + 1.05;
            [
                0.31 * left_phase.sin() as f32,
                0.23 * right_phase.sin() as f32,
            ]
        })
        .collect()
}

#[test]
fn stereo_multiway_crossover_band_merge_chain_matches_complex_reference() {
    let mut host = DawHost::new(2, SAMPLE_RATE);
    host.add_plugin(Box::new(
        CrossoverPlugin::new_multiway(2, "LR24", 1_000.0, "both", &[1_200.0]).unwrap(),
    ))
    .unwrap();
    host.add_plugin(Box::new(BandMergePlugin::new(2, 3).unwrap()))
        .unwrap();
    host.build().unwrap();

    let frames = WARMUP_FRAMES + MEASURE_FRAMES;
    let input = sine_input(frames);
    let mut output = vec![0.0; input.len()];
    let mut cursor = 0;
    while cursor < frames {
        let count = BLOCK_FRAMES.min(frames - cursor);
        host.process(
            &input[cursor * 2..(cursor + count) * 2],
            &mut output[cursor * 2..(cursor + count) * 2],
        )
        .unwrap();
        cursor += count;
    }

    let mut max_complex_error = 0.0_f64;
    let mut max_waveform_residual = 0.0_f64;
    let mut max_cross_channel_projection = 0.0_f64;
    for (channel, frequency_hz, amplitude) in [(0, 1_100.0, 0.31), (1, 1_700.0, 0.23)] {
        let input_channel: Vec<f32> = input.iter().skip(channel).step_by(2).copied().collect();
        let output_channel: Vec<f32> = output.iter().skip(channel).step_by(2).copied().collect();
        let input_projection = project(&input_channel, WARMUP_FRAMES, MEASURE_FRAMES, frequency_hz);
        assert!(input_projection.norm() > amplitude * 0.99);
        let actual = project(&output_channel, WARMUP_FRAMES, MEASURE_FRAMES, frequency_hz);
        let measured = Complex {
            re: (actual.re * input_projection.re + actual.im * input_projection.im)
                / input_projection.norm().powi(2),
            im: (actual.im * input_projection.re - actual.re * input_projection.im)
                / input_projection.norm().powi(2),
        };
        let expected =
            lr24_allpass_at(frequency_hz, 1_000.0) * lr24_allpass_at(frequency_hz, 1_200.0);
        let complex_error = measured.distance(expected);
        max_complex_error = max_complex_error.max(complex_error);
        assert!(
            complex_error <= 2.0e-3,
            "channel={channel} frequency={frequency_hz} measured={measured:?}, expected={expected:?}"
        );
        assert!((measured.norm() - 1.0).abs() <= 2.0e-3);

        let expected_projection = Complex {
            re: input_projection.re * expected.re - input_projection.im * expected.im,
            im: input_projection.re * expected.im + input_projection.im * expected.re,
        };
        let omega = std::f64::consts::TAU * frequency_hz / f64::from(SAMPLE_RATE);
        let residual_rms = (0..MEASURE_FRAMES)
            .map(|offset| {
                let frame = WARMUP_FRAMES + offset;
                let phase = omega * frame as f64;
                let predicted =
                    expected_projection.re * phase.cos() - expected_projection.im * phase.sin();
                (f64::from(output_channel[frame]) - predicted).powi(2)
            })
            .sum::<f64>()
            / MEASURE_FRAMES as f64;
        let input_rms = (input_channel[WARMUP_FRAMES..WARMUP_FRAMES + MEASURE_FRAMES]
            .iter()
            .map(|&sample| f64::from(sample).powi(2))
            .sum::<f64>()
            / MEASURE_FRAMES as f64)
            .sqrt();
        let normalized_waveform_residual = residual_rms.sqrt() / input_rms;
        max_waveform_residual = max_waveform_residual.max(normalized_waveform_residual);
        assert!(
            normalized_waveform_residual <= 2.0e-3,
            "channel={channel} frequency={frequency_hz} waveform residual/input RMS={normalized_waveform_residual}"
        );

        let other_frequency_hz = if channel == 0 { 1_700.0 } else { 1_100.0 };
        let leakage = project(
            &output_channel,
            WARMUP_FRAMES,
            MEASURE_FRAMES,
            other_frequency_hz,
        );
        max_cross_channel_projection = max_cross_channel_projection.max(leakage.norm());
        assert!(
            leakage.norm() <= 2.0e-4,
            "channel={channel} cross-talk projection at {other_frequency_hz} Hz is {}",
            leakage.norm()
        );
    }
    println!(
        "stereo 2->6->2 chain: max complex error={max_complex_error:.3e}, max waveform residual/input RMS={max_waveform_residual:.3e}, max cross-channel projection={max_cross_channel_projection:.3e}"
    );
}
