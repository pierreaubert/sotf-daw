//! Independent offline WOLA reference for the spectral dynamics clock.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_spectral_compressor::{SpectralCompressorPlugin, SpectralCompressorPluginParams};
use std::f64::consts::TAU;
use std::ops::{Add, Mul, Sub};

#[derive(Clone, Copy, Default)]
struct C(f64, f64);
impl Add for C {
    type Output = Self;
    fn add(self, b: Self) -> Self {
        Self(self.0 + b.0, self.1 + b.1)
    }
}
impl Sub for C {
    type Output = Self;
    fn sub(self, b: Self) -> Self {
        Self(self.0 - b.0, self.1 - b.1)
    }
}
impl Mul for C {
    type Output = Self;
    fn mul(self, b: Self) -> Self {
        Self(self.0 * b.0 - self.1 * b.1, self.0 * b.1 + self.1 * b.0)
    }
}
fn transform(x: &mut [C], inverse: bool) {
    let n = x.len();
    for i in 0..n {
        let j = i.reverse_bits() >> (usize::BITS - n.trailing_zeros());
        if j > i {
            x.swap(i, j);
        }
    }
    let mut width = 2;
    while width <= n {
        for start in (0..n).step_by(width) {
            for j in 0..width / 2 {
                let angle = if inverse { TAU } else { -TAU } * j as f64 / width as f64;
                let a = x[start + j];
                let b = x[start + j + width / 2] * C(angle.cos(), angle.sin());
                x[start + j] = a + b;
                x[start + j + width / 2] = a - b;
            }
        }
        width *= 2;
    }
    if inverse {
        for c in x {
            *c = *c * C(1.0 / n as f64, 0.0);
        }
    }
}

fn reduction(db: f64, threshold: f64, ratio: f64, knee: f64) -> f64 {
    let over = db - threshold;
    let slope = 1.0 - 1.0 / ratio;
    if over < -knee / 2.0 {
        0.0
    } else if over > knee / 2.0 || knee == 0.0 {
        slope * over.max(0.0)
    } else {
        slope * (over + knee / 2.0).powi(2) / (2.0 * knee)
    }
}

fn reference(
    input: &[f32],
    channels: usize,
    rate: u32,
    p: &SpectralCompressorPluginParams,
) -> Vec<f64> {
    let n = [1024, 2048, 4096][p.fft_size_index];
    let h = n / 4;
    let frames = input.len() / channels;
    let bins = n / 2 + 1;
    let mut output = vec![0.0; (2 * n + (frames - 1) / h * h) * channels];
    let window: Vec<f64> = (0..n)
        .map(|i| 0.5 - 0.5 * (TAU * i as f64 / n as f64).cos())
        .collect();
    let mut envelope = vec![vec![0.0_f64; bins]; channels];
    let attack = (-(h as f64) / (f64::from(p.attack_ms) * 0.001 * f64::from(rate))).exp();
    let release = (-(h as f64) / (f64::from(p.release_ms) * 0.001 * f64::from(rate))).exp();
    // Absolute window origins make priming, latency and finite support explicit;
    // this reference has no circular histories or streaming output scheduler.
    for origin in (-(3 * h as isize)..frames as isize).step_by(h) {
        let mut spectra = vec![vec![C::default(); n]; channels];
        let mut targets = vec![vec![0.0; bins]; channels];
        for ch in 0..channels {
            for (i, c) in spectra[ch].iter_mut().enumerate() {
                let time = origin + i as isize;
                if (0..frames as isize).contains(&time) {
                    c.0 = f64::from(input[time as usize * channels + ch]) * window[i];
                }
            }
            transform(&mut spectra[ch], false);
            let magnitudes: Vec<f64> = (0..bins)
                .map(|k| {
                    let c = spectra[ch][k];
                    c.0.hypot(c.1)
                        * if k == 0 || k == n / 2 {
                            2.0 / n as f64
                        } else {
                            4.0 / n as f64
                        }
                })
                .collect();
            for (k, target) in targets[ch].iter_mut().enumerate() {
                let power = magnitudes[k.saturating_sub(2)..(k + 3).min(bins)]
                    .iter()
                    .map(|m| m * m)
                    .sum::<f64>();
                let db = 20.0 * (power / 1.5).sqrt().max(1e-10).log10();
                *target = reduction(
                    db,
                    f64::from(p.threshold_db),
                    f64::from(p.ratio),
                    f64::from(p.knee_db),
                );
            }
        }
        for k in 0..bins {
            let maximum = targets.iter().map(|ch| ch[k]).fold(0.0, f64::max);
            for ch in 0..channels {
                let target =
                    targets[ch][k] + f64::from(p.channel_link) * (maximum - targets[ch][k]);
                let coeff = if target > envelope[ch][k] {
                    attack
                } else {
                    release
                };
                envelope[ch][k] = target + (envelope[ch][k] - target) * coeff;
            }
        }
        for ch in 0..channels {
            // Frequency filtering is an output operation; it must not become a
            // second recursive process inside the temporal envelope recurrence.
            let median: Vec<f64> = (0..bins)
                .map(|k| {
                    if k == 0 {
                        return envelope[ch][0].min(envelope[ch][1]);
                    }
                    if k == bins - 1 {
                        return envelope[ch][k].min(envelope[ch][k - 1]);
                    }
                    let mut values = [envelope[ch][k - 1], envelope[ch][k], envelope[ch][k + 1]];
                    values.sort_by(f64::total_cmp);
                    values[1]
                })
                .collect();
            let radius = if p.spectral_smoothing > 0.001 {
                (f64::from(p.spectral_smoothing) * 12.0).ceil() as usize
            } else {
                0
            };
            for k in 0..bins {
                let from = k.saturating_sub(radius);
                let to = (k + radius + 1).min(bins);
                let db = median[from..to].iter().sum::<f64>() / (to - from) as f64;
                let gain = if db > 0.001 {
                    10.0_f64.powf(-db / 20.0)
                } else {
                    1.0
                };
                spectra[ch][k] = spectra[ch][k] * C(gain, 0.0);
                if k > 0 && k < n / 2 {
                    spectra[ch][n - k] = spectra[ch][n - k] * C(gain, 0.0);
                }
            }
            transform(&mut spectra[ch], true);
            for i in 0..n {
                let time = origin + i as isize;
                if time >= 0 {
                    output[(time as usize + n) * channels + ch] +=
                        spectra[ch][i].0 * window[i] / 1.5;
                }
            }
        }
    }
    output
}

fn render(
    input: &[f32],
    channels: usize,
    rate: u32,
    p: &SpectralCompressorPluginParams,
    parts: &[usize],
) -> Vec<f32> {
    let mut plugin = SpectralCompressorPlugin::from_params(channels, p.clone());
    plugin.initialize(f64::from(rate)).unwrap();
    let n = [1024, 2048, 4096][p.fft_size_index];
    assert_eq!(plugin.latency_samples(), n);
    let mut output = input.to_vec();
    let mut offset = 0;
    let mut block = 0;
    while offset < input.len() / channels {
        let frames = parts[block % parts.len()].min(input.len() / channels - offset);
        plugin
            .process_in_place(
                &mut output[offset * channels..(offset + frames) * channels],
                &ProcessContext::new(rate, frames),
            )
            .unwrap();
        offset += frames;
        block += 1;
    }
    for call in 0..10000 {
        let capacity = [1, 17, 511][call % 3];
        let mut tail = vec![1234.0; capacity * channels];
        let result = plugin
            .drain(&mut tail, &ProcessContext::new(rate, capacity))
            .unwrap();
        assert!(
            tail[result.frames * channels..]
                .iter()
                .all(|&x| x == 1234.0)
        );
        output.extend_from_slice(&tail[..result.frames * channels]);
        if result.complete {
            return output;
        }
    }
    panic!("drain did not complete")
}
fn material(n: usize) -> Vec<f32> {
    let mut state = 17_u64;
    (0..n * 7 + 37)
        .flat_map(|i| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let noise = (state as i64 as f64 / i64::MAX as f64) as f32;
            let tone = (TAU * 43.0 * i as f64 / n as f64).sin() as f32;
            let other = (TAU * 151.25 * i as f64 / n as f64).cos() as f32;
            let a = if i < n * 2 {
                0.015
            } else if i < n * 4 {
                0.7
            } else {
                0.04
            };
            let burst = if (n * 3..n * 3 + 73).contains(&i) {
                0.4 * noise
            } else {
                0.0
            };
            let marker = if i == 0 || i == n * 7 + 36 { 0.3 } else { 0.0 };
            [tone * a + burst + marker, other * 0.035 - burst * 0.5]
        })
        .collect()
}
#[test]
fn isolated_tones_broadband_transients_and_release_match_offline_hop_dynamics() {
    let mut maximum = 0.0_f64;
    for index in 0..3 {
        let input = material([1024, 2048, 4096][index]);
        for rate in [44_100, 48_000, 96_000, 192_000] {
            for (smoothing, link) in [(0.0, 0.0), (0.5, 0.5), (1.0, 1.0)] {
                let p = SpectralCompressorPluginParams {
                    fft_size_index: index,
                    threshold_db: -30.0,
                    ratio: 4.0,
                    attack_ms: 10.0,
                    release_ms: 80.0,
                    knee_db: 6.0,
                    spectral_smoothing: smoothing,
                    channel_link: link,
                    mix: 1.0,
                    ..Default::default()
                };
                let expected = reference(&input, 2, rate, &p);
                let actual = render(&input, 2, rate, &p, &[1, 31, 257, 1023]);
                assert_eq!(actual.len(), expected.len());
                let error = actual
                    .iter()
                    .zip(&expected)
                    .map(|(&a, &b)| (f64::from(a) - b).abs())
                    .fold(0.0_f64, f64::max);
                maximum = maximum.max(error);
                assert!(
                    error < 3e-6,
                    "FFT={index} {rate} smoothing={smoothing} link={link}: waveform error {error:e}"
                );
            }
        }
    }
    println!("maximum independent dynamics sample error={maximum:e}");
}
#[test]
fn unity_wola_reconstruction_preserves_arbitrary_endpoints_at_every_rate() {
    for index in 0..3 {
        let n = [1024, 2048, 4096][index];
        let input = material(n);
        for rate in [44_100, 48_000, 96_000, 192_000] {
            let p = SpectralCompressorPluginParams {
                fft_size_index: index,
                ratio: 1.0,
                ..Default::default()
            };
            let actual = render(&input, 2, rate, &p, &[n + 7, 1, 19]);
            for (i, &sample) in actual.iter().enumerate() {
                let expected = if i >= n * 2 {
                    input.get(i - n * 2).copied().unwrap_or(0.0)
                } else {
                    0.0
                };
                assert!(
                    (sample - expected).abs() < 2e-6,
                    "FFT={n} {rate} sample={i}: {sample} versus delayed input {expected}"
                );
            }
        }
    }
}

#[test]
fn full_link_preserves_channel_ratios_polarity_and_silence_during_transients() {
    for index in 0..3 {
        let n = [1024, 2048, 4096][index];
        let input: Vec<f32> = (0..7 * n + 19)
            .flat_map(|i| {
                let x = (0.7 * (TAU * 47.0 * i as f64 / n as f64).sin()) as f32;
                [x, x * 0.1, -x * 0.3, 0.0]
            })
            .collect();
        for rate in [44_100, 48_000, 96_000, 192_000] {
            let mut p = SpectralCompressorPluginParams {
                fft_size_index: index,
                threshold_db: -18.0,
                ratio: 8.0,
                attack_ms: 3.0,
                release_ms: 100.0,
                knee_db: 0.0,
                spectral_smoothing: 0.5,
                channel_link: 1.0,
                ..Default::default()
            };
            let linked = render(&input, 4, rate, &p, &[1, 43, 509]);
            for frame in linked.as_chunks::<4>().0.iter() {
                assert!((frame[1] - frame[0] * 0.1).abs() < 2e-6);
                assert!((frame[2] + frame[0] * 0.3).abs() < 2e-6);
                assert_eq!(frame[3], 0.0);
            }
            let rms = |x: &[f32]| {
                (x.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|f| f64::from(f[0]).powi(2))
                    .sum::<f64>()
                    / (x.len() / 4) as f64)
                    .sqrt()
            };
            assert!(
                rms(&linked[4 * n * 4..6 * n * 4]) < rms(&input[3 * n * 4..5 * n * 4]) * 0.8,
                "link test must exercise active reduction"
            );
            p.channel_link = 0.0;
            let independent = render(&input, 4, rate, &p, &[4096]);
            for (i, frame) in independent.as_chunks::<4>().0.iter().enumerate() {
                let expected = if i >= n {
                    input.get((i - n) * 4 + 1).copied().unwrap_or(0.0)
                } else {
                    0.0
                };
                assert!(
                    (frame[1] - expected).abs() < 2e-6,
                    "independent quiet channel compressed at {i}"
                );
            }
        }
    }
}

fn amplitude(x: &[f32], bin: usize, period: usize) -> f64 {
    let (re, im) = x.iter().enumerate().fold((0.0, 0.0), |(re, im), (i, &x)| {
        let phase = TAU * bin as f64 * i as f64 / period as f64;
        (
            re + f64::from(x) * phase.cos(),
            im + f64::from(x) * phase.sin(),
        )
    });
    2.0 * re.hypot(im) / x.len() as f64
}
#[test]
fn remote_quiet_tone_does_not_pump_with_modulated_loud_resonance() {
    for index in 0..3 {
        let n = [1024, 2048, 4096][index];
        let input: Vec<f32> = (0..n * 32)
            .map(|i| {
                let phase = TAU * i as f64 / n as f64;
                ((0.4 + 0.3 * (phase * 5.0).sin()) * (phase * 43.0).sin()
                    + 0.02 * (phase * 200.0).sin()) as f32
            })
            .collect();
        for rate in [44_100, 48_000, 96_000, 192_000] {
            let p = SpectralCompressorPluginParams {
                fft_size_index: index,
                threshold_db: -24.0,
                ratio: 10.0,
                attack_ms: 1.0,
                release_ms: 40.0,
                knee_db: 0.0,
                spectral_smoothing: 1.0,
                ..Default::default()
            };
            let output = render(&input, 1, rate, &p, &[4096, 17]);
            let settled = &output[17 * n..33 * n];
            let quiet = amplitude(settled, 200, n);
            assert!(
                (quiet - 0.02).abs() < 1e-5,
                "quiet carrier moved: FFT={n} {rate} {quiet}"
            );
            for sideband in [195, 205] {
                let pumping = amplitude(settled, sideband, n);
                assert!(
                    pumping < quiet * 0.001,
                    "FFT={n} {rate}: remote pumping sideband {sideband} = {pumping:e}"
                );
            }
            assert!(
                amplitude(settled, 43, n) < 0.32,
                "loud resonance must be compressed"
            );
        }
    }
}
