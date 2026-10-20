//! Independent nonlinear startup, chatter, range and linked spectral references.
//!
//! Fixed waveform bound: 0.0035 of reference peak (approximately 0.03 dB).
//! The offline spectral oracle uses full-complex f64 FFTs, absolute window
//! positions and a linear overlap sum, never the production FFT/ring/state code.

// Rust guideline compliant 2026-02-21
use rustfft::{FftPlanner, num_complex::Complex};
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_multiband_expander::{
    BandExpanderParams, MultibandExpanderPlugin, MultibandExpanderPluginParams,
};
use std::f64::consts::TAU;

#[global_allocator]
static ALLOCATOR: sotf_host::CountingAlloc = sotf_host::CountingAlloc;

#[derive(Clone, Copy)]
struct Curve {
    threshold: f64,
    knee: f64,
    range: f64,
    hysteresis: f64,
    hold: usize,
    attack: f64,
    release: f64,
}
impl Curve {
    fn new(p: &MultibandExpanderPluginParams, band: usize, clock: f64) -> Self {
        let b = p.bands.get(band).cloned().unwrap_or_default();
        Self {
            threshold: f64::from(b.threshold_db.unwrap_or(p.threshold_db)),
            knee: f64::from(p.knee_db),
            range: f64::from(p.range_db),
            hysteresis: f64::from(p.hysteresis_db),
            hold: (f64::from(p.hold_ms) * 0.001 * clock).round() as usize,
            attack: (-1.0 / (f64::from(p.attack_ms) * 0.001 * clock)).exp(),
            release: (-1.0 / (f64::from(p.release_ms) * 0.001 * clock)).exp(),
        }
    }
    fn attenuation(self, db: f64) -> f64 {
        // Ratio4 means three dB of added attenuation per dB below threshold.
        let distance = self.threshold - db;
        let value = if self.knee < 0.1 {
            3.0 * distance.max(0.0)
        } else if distance <= -self.knee / 2.0 {
            0.0
        } else if distance >= self.knee / 2.0 {
            3.0 * distance
        } else {
            3.0 * (distance + self.knee / 2.0).powi(2) / (2.0 * self.knee)
        };
        value.min(self.range)
    }
}
#[derive(Clone, Copy)]
enum Phase {
    Open,
    Hold(usize),
    Closing,
}
#[derive(Clone, Copy)]
struct Envelope {
    phase: Phase,
    db: f64,
}
impl Envelope {
    fn new() -> Self {
        Self {
            phase: Phase::Open,
            db: 0.0,
        }
    }
    fn gain(&mut self, db: f64, c: Curve) -> f64 {
        let upper = c.threshold + if c.knee < 0.1 { 0.0 } else { c.knee / 2.0 };
        let target = match self.phase {
            Phase::Open if db < upper => {
                self.phase = Phase::Hold(c.hold);
                0.0
            }
            Phase::Open => 0.0,
            Phase::Hold(_) | Phase::Closing if db >= upper => {
                self.phase = Phase::Open;
                0.0
            }
            Phase::Hold(n) if n > 0 => {
                self.phase = Phase::Hold(n - 1);
                0.0
            }
            Phase::Hold(_) if db >= upper - c.hysteresis => 0.0,
            Phase::Hold(_) | Phase::Closing => {
                self.phase = Phase::Closing;
                c.attenuation(db)
            }
        };
        let decay = if target > self.db {
            c.attack
        } else {
            c.release
        };
        self.db = target + decay * (self.db - target);
        assert!(self.db >= -1e-12 && self.db <= c.range + 1e-12);
        10.0_f64.powf(-self.db / 20.0)
    }
}
fn settings(
    spectral: bool,
    knee: f32,
    hysteresis: f32,
    hold: f32,
    range: f32,
) -> MultibandExpanderPluginParams {
    MultibandExpanderPluginParams {
        num_bands: if spectral { 3 } else { 1 },
        crossover_frequencies: vec![400.0, 3000.0],
        processing_mode: if spectral { "spectral" } else { "time_domain" }.into(),
        threshold_db: -48.0,
        ratio: 4.0,
        knee_db: knee,
        hysteresis_db: hysteresis,
        hold_ms: hold,
        range_db: range,
        attack_ms: 1.0,
        release_ms: 40.0,
        link_channels: true,
        sidechain_hpf_hz: Some(0.0),
        auto_makeup: Some(false),
        measured_auto_makeup: Some(false),
        bands: if spectral {
            [-48.0, -42.0, -54.0]
                .into_iter()
                .map(|threshold| BandExpanderParams {
                    threshold_db: Some(threshold),
                    ..Default::default()
                })
                .collect()
        } else {
            vec![]
        },
        ..Default::default()
    }
}
fn input(rate: u32, spectral: bool, channels: usize) -> Vec<f32> {
    (0..16384 * channels)
        .map(|i| {
            let n = i / channels;
            let level = if n < 2048 {
                -60.0
            } else if n < 4096 {
                -36.0
            } else if n < 12288 {
                if (n / 384).is_multiple_of(2) {
                    -47.6
                } else {
                    -48.4
                }
            } else {
                -75.0
            };
            let amplitude = 10.0_f64.powf(level / 20.0);
            let carrier = if spectral {
                let phase = TAU * n as f64 / 1024.0;
                (phase * 7.0).cos() + 0.35 * (phase * 37.0).cos() + 0.15 * (phase * 129.0).cos()
            } else {
                1.0 + 0.01 * (TAU * 13.0 * n as f64 / f64::from(rate)).sin()
            };
            (amplitude * carrier * if i % channels == 0 { 1.0 } else { -0.125 }) as f32
        })
        .collect()
}
fn time_reference(
    input: &[f32],
    rate: u32,
    channels: usize,
    p: &MultibandExpanderPluginParams,
) -> Vec<f64> {
    let c = Curve::new(p, 0, f64::from(rate));
    let mut state = Envelope::new();
    let mut peaks = vec![0.0_f64; channels];
    let window = (0.01 * f64::from(rate)).round() as usize;
    let mut energies = vec![vec![0.0; window]; channels];
    let mut sums = vec![0.0; channels];
    let mut output = vec![0.0; input.len()];
    for (n, frame) in input.chunks_exact(channels).enumerate() {
        let mut linked = 0.0_f64;
        for (ch, &x) in frame.iter().enumerate() {
            let x = f64::from(x);
            peaks[ch] = x
                .abs()
                .max(peaks[ch] * (-1.0 / (0.005 * f64::from(rate))).exp());
            sums[ch] += x * x - energies[ch][n % window];
            energies[ch][n % window] = x * x;
            linked = linked.max(if p.detection_mode.eq_ignore_ascii_case("rms") {
                (sums[ch].max(0.0) / window as f64).sqrt()
            } else {
                peaks[ch]
            });
        }
        let gain = state.gain(20.0 * linked.max(1e-10).log10(), c);
        for ch in 0..channels {
            output[n * channels + ch] = f64::from(frame[ch]) * gain;
        }
    }
    output
}
fn spectral_reference(
    input: &[f32],
    rate: u32,
    channels: usize,
    p: &MultibandExpanderPluginParams,
) -> Vec<f64> {
    const N: usize = 1024;
    const H: usize = 256;
    let frames = input.len() / channels;
    let window: Vec<_> = (0..N)
        .map(|i| 0.5 - 0.5 * (TAU * i as f64 / N as f64).cos())
        .collect();
    let sum: f64 = window.iter().sum();
    let mut planner = FftPlanner::<f64>::new();
    let forward = planner.plan_fft_forward(N);
    let inverse = planner.plan_fft_inverse(N);
    let mut states = vec![Envelope::new(); N / 2 + 1];
    let mut spectra = vec![vec![Complex::new(0.0, 0.0); N]; channels];
    let mut output = vec![0.0; input.len()];
    for start in (-768..frames as isize - N as isize + 1).step_by(H) {
        for (ch, spectrum) in spectra.iter_mut().enumerate() {
            for i in 0..N {
                let source = start + i as isize;
                let x = if source < 0 {
                    0.0
                } else {
                    f64::from(input[source as usize * channels + ch])
                };
                spectrum[i] = Complex::new(x * window[i], 0.0);
            }
            forward.process(spectrum);
        }
        for (k, state) in states.iter_mut().enumerate() {
            let frequency = k as f64 * f64::from(rate) / N as f64;
            let band = p
                .crossover_frequencies
                .iter()
                .take(p.num_bands - 1)
                .filter(|&&cut| frequency >= f64::from(cut))
                .count();
            let amplitude = spectra
                .iter()
                .map(|s| s[k].norm() * 2.0 / sum)
                .fold(0.0_f64, f64::max);
            let gain = state.gain(
                20.0 * amplitude.max(1e-10).log10(),
                Curve::new(p, band, f64::from(rate) / H as f64),
            );
            for spectrum in &mut spectra {
                spectrum[k] *= gain;
                if k > 0 && k < N / 2 {
                    spectrum[N - k] *= gain;
                }
            }
        }
        for (ch, spectrum) in spectra.iter_mut().enumerate() {
            inverse.process(spectrum);
            for i in 0..N {
                let destination = start + N as isize + i as isize;
                if destination >= N as isize && destination < frames as isize {
                    output[destination as usize * channels + ch] +=
                        spectrum[i].re * window[i] / (1.5 * N as f64);
                }
            }
        }
    }
    output
}
fn render(
    p: MultibandExpanderPluginParams,
    input: &[f32],
    rate: u32,
    channels: usize,
    block: usize,
) -> Vec<f32> {
    let mut plugin = MultibandExpanderPlugin::try_from_params(channels, p, rate).unwrap();
    plugin.initialize(f64::from(rate)).unwrap();
    let mut output = input.to_vec();
    for chunk in output.chunks_mut(block * channels) {
        let frames = chunk.len() / channels;
        let heap = sotf_host::test_utils::measure_heap_activity(|| {
            assert_eq!(
                plugin
                    .process_in_place(chunk, &ProcessContext::new(rate, frames))
                    .unwrap(),
                frames
            );
        });
        assert_eq!(heap, (0, 0));
    }
    assert!(output.iter().all(|x| x.is_finite()));
    output
}

#[test]
fn nonlinear_quiet_onsets_chatter_and_range_match_independent_references() {
    let mut cases = 0;
    for rate in [44_100, 96_000] {
        for channels in [1, 2] {
            for spectral in [false, true] {
                for knee in [0.0, 6.0] {
                    for (hysteresis, hold, range) in [
                        (0.0, 0.0, 12.0),
                        (0.0, 0.0, 48.0),
                        (0.0, 10.0, 12.0),
                        (0.0, 10.0, 48.0),
                        (3.0, 0.0, 12.0),
                        (3.0, 0.0, 48.0),
                        (3.0, 10.0, 12.0),
                        (3.0, 10.0, 48.0),
                    ] {
                        for rms in [false, true] {
                            if spectral && rms {
                                continue;
                            }
                            let mut p = settings(spectral, knee, hysteresis, hold, range);
                            p.detection_mode = if rms { "RMS" } else { "Peak" }.into();
                            let input = input(rate, spectral, channels);
                            let expected = if spectral {
                                spectral_reference(&input, rate, channels, &p)
                            } else {
                                time_reference(&input, rate, channels, &p)
                            };
                            let actual = render(p.clone(), &input, rate, channels, 137);
                            let peak = expected
                                .iter()
                                .copied()
                                .map(f64::abs)
                                .fold(0.0_f64, f64::max);
                            assert!(peak > 1e-5);
                            let error = actual
                                .iter()
                                .zip(&expected)
                                .map(|(&a, &b)| (f64::from(a) - b).abs())
                                .fold(0.0_f64, f64::max)
                                / peak;
                            let delay = if spectral { 1024 } else { 0 };
                            let onset = delay * channels..(delay + 2048) * channels;
                            let onset_peak = expected[onset.clone()]
                                .iter()
                                .copied()
                                .map(f64::abs)
                                .fold(0.0_f64, f64::max);
                            assert!(onset_peak > 1e-8);
                            let onset_error = actual[onset.clone()]
                                .iter()
                                .zip(&expected[onset])
                                .map(|(&a, &b)| (f64::from(a) - b).abs())
                                .fold(0.0_f64, f64::max)
                                / onset_peak;
                            assert!(
                                onset_error < 0.0035,
                                "quiet onset {onset_error}: {rate}/{channels}/{spectral}/{rms}/{knee}/{hysteresis}"
                            );
                            println!(
                                "{}",
                                serde_json::json!({"case":"nonlinear_startup_chatter","rate":rate,"channels":channels,"spectral":spectral,"rms":rms,"knee":knee,"hysteresis":hysteresis,"hold_ms":hold,"range_db":range,"relative_peak_error":error,"quiet_onset_relative_error":onset_error})
                            );
                            assert!(
                                error < 0.0035,
                                "relative error {error}: {rate}/{channels}/{spectral}/{rms}/{knee}/{hysteresis}"
                            );
                            assert_eq!(actual, render(p, &input, rate, channels, 1024));
                            cases += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(cases, 192);
}

#[test]
fn constructor_choice_case_preserves_the_requested_route_and_detector() {
    for spectral in [false, true] {
        for rms in [false, true] {
            if spectral && rms {
                continue;
            }
            let mut p = settings(spectral, 6.0, 3.0, 10.0, 48.0);
            p.detection_mode = if rms { "rms" } else { "peak" }.into();
            let input = input(48_000, spectral, 2);
            let expected = render(p.clone(), &input, 48_000, 2, 137);
            for label in if rms {
                ["rms", "RMS", "RmS"]
            } else {
                ["peak", "Peak", "PEAK"]
            } {
                for mode in if spectral {
                    ["spectral", "Spectral", "SPECTRAL"]
                } else {
                    ["time_domain", "Time_Domain", "TIME_DOMAIN"]
                } {
                    p.detection_mode = label.into();
                    p.processing_mode = mode.into();
                    assert_eq!(
                        render(p.clone(), &input, 48_000, 2, 137),
                        expected,
                        "{label}/{mode}"
                    );
                }
            }
        }
    }
}
