//! Independent reconstruction and crossover-edge transient audit for entry 35.
//!
//! Fixed bounds: sample/solo closure 2e-5; complex response 2e-3;
//! transient peak index one frame. Spectral unity uses a pure delayed input.
//! Filter oracles use independent f64 Tustin coefficients and DF-I state.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_multiband_expander::{MultibandExpanderPlugin, MultibandExpanderPluginParams};
use std::f64::consts::{PI, SQRT_2};

#[global_allocator]
static ALLOCATOR: sotf_host::CountingAlloc = sotf_host::CountingAlloc;
const SAMPLE: f64 = 2e-5;
const COMPLEX: f64 = 2e-3;

#[derive(Clone, Copy)]
struct Section {
    b: [f64; 3],
    a: [f64; 2],
    x: [f64; 2],
    y: [f64; 2],
}
impl Section {
    fn new(cut: f64, rate: f64, high: bool) -> Self {
        let k = (PI * cut / rate).tan();
        let denominator = 1.0 + SQRT_2 * k + k * k;
        let numerator = if high {
            [1.0, -2.0, 1.0]
        } else {
            [k * k, 2.0 * k * k, k * k]
        };
        Self {
            b: numerator.map(|v| v / denominator),
            a: [
                2.0 * (k * k - 1.0) / denominator,
                (1.0 - SQRT_2 * k + k * k) / denominator,
            ],
            x: [0.0; 2],
            y: [0.0; 2],
        }
    }
    fn process(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.b[1] * self.x[0] + self.b[2] * self.x[1]
            - self.a[0] * self.y[0]
            - self.a[1] * self.y[1];
        self.x = [x, self.x[0]];
        self.y = [y, self.y[0]];
        y
    }
}
struct Split {
    low: [Section; 2],
    high: [Section; 2],
}
impl Split {
    fn new(cut: f64, rate: f64) -> Self {
        Self {
            low: [Section::new(cut, rate, false); 2],
            high: [Section::new(cut, rate, true); 2],
        }
    }
    fn process(&mut self, x: f64) -> (f64, f64) {
        let low = self.low[0].process(x);
        let low = self.low[1].process(low);
        let high = self.high[0].process(x);
        let high = self.high[1].process(high);
        (low, high)
    }
}
fn bands(input: &[f32], channels: usize, rate: u32, cuts: &[f64]) -> Vec<Vec<f64>> {
    let mut states: Vec<Vec<_>> = cuts
        .iter()
        .map(|&cut| {
            (0..channels)
                .map(|_| Split::new(cut, f64::from(rate)))
                .collect()
        })
        .collect();
    let mut output = vec![vec![0.0; input.len()]; cuts.len() + 1];
    for (index, &sample) in input.iter().enumerate() {
        let ch = index % channels;
        let mut remaining = f64::from(sample);
        for (split, state) in states.iter_mut().enumerate() {
            let (low, high) = state[ch].process(remaining);
            output[split][index] = low;
            remaining = high;
        }
        output[cuts.len()][index] = remaining;
    }
    output
}
fn make(
    rate: u32,
    channels: usize,
    params: MultibandExpanderPluginParams,
    solo: Option<usize>,
) -> MultibandExpanderPlugin {
    let mut plugin = MultibandExpanderPlugin::try_from_params(channels, params, rate).unwrap();
    plugin.initialize(f64::from(rate)).unwrap();
    if let Some(band) = solo {
        plugin
            .set_parameter(
                ParameterId::from(format!("band_{band}_solo")),
                ParameterValue::Bool(true),
            )
            .unwrap();
    }
    plugin
}
fn render(
    plugin: &mut MultibandExpanderPlugin,
    rate: u32,
    channels: usize,
    input: &[f32],
    block: usize,
) -> Vec<f32> {
    let mut output = input.to_vec();
    for samples in output.chunks_mut(block * channels) {
        let frames = samples.len() / channels;
        let counts = sotf_host::test_utils::measure_heap_activity(|| {
            assert_eq!(
                plugin
                    .process_in_place(samples, &ProcessContext::new(rate, frames))
                    .unwrap(),
                frames
            );
        });
        assert_eq!(counts, (0, 0));
    }
    assert!(output.iter().all(|sample| sample.is_finite()));
    output
}
fn peak(signal: &[f64], channels: usize, ch: usize) -> usize {
    signal
        .chunks_exact(channels)
        .enumerate()
        .max_by(|a, b| a.1[ch].abs().total_cmp(&b.1[ch].abs()))
        .unwrap()
        .0
}
fn transfer(signal: &[f64], channels: usize, frequency: f64, rate: u32) -> (f64, f64) {
    signal
        .chunks_exact(channels)
        .enumerate()
        .fold((0.0, 0.0), |(re, im), (n, x)| {
            let phase = 2.0 * PI * frequency * n as f64 / f64::from(rate);
            (re + x[0] * phase.cos() * 2.0, im - x[0] * phase.sin() * 2.0)
        })
}

fn neutral(
    count: usize,
    cuts: &[f64],
    spectral: bool,
    lookahead: f32,
) -> MultibandExpanderPluginParams {
    MultibandExpanderPluginParams {
        num_bands: count,
        crossover_frequencies: cuts.iter().map(|&x| x as f32).collect(),
        processing_mode: if spectral { "spectral" } else { "time_domain" }.into(),
        ratio: 1.0,
        mix: 1.0,
        lookahead_ms: lookahead,
        sidechain_hpf_hz: Some(0.0),
        auto_makeup: Some(false),
        measured_auto_makeup: Some(false),
        ..Default::default()
    }
}

#[test]
fn time_domain_unity_impulses_match_independent_cascade_and_band_timing() {
    let mut cases = 0;
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for count in 1..=5 {
            let cuts = &[120.0, 700.0, 6000.0, 12000.0][..count - 1];
            for lookahead in [0.0, 5.0] {
                for channels in [1, 2, 6] {
                    let params = neutral(count, cuts, false, lookahead);
                    let mut plugin = make(rate, channels, params.clone(), None);
                    let delay = plugin.latency_samples();
                    let mut input = vec![0.0; (8192 + delay) * channels];
                    for (ch, sample) in input[..channels].iter_mut().enumerate() {
                        *sample = 0.5 / (ch + 1) as f32;
                    }
                    let references = bands(&input, channels, rate, cuts);
                    let actual = render(&mut plugin, rate, channels, &input, 257);
                    let mut expected_sum = vec![0.0; input.len()];
                    let mut solo_sum = vec![0.0; input.len()];
                    let mut max_sample = 0.0_f64;
                    let mut max_peak = 0;
                    for (band, source) in references.iter().enumerate() {
                        let mut plugin = make(rate, channels, params.clone(), Some(band));
                        let solo =
                            render(&mut plugin, rate, channels, &input, [1, 73, 1024][band % 3]);
                        let expected: Vec<_> = (0..input.len())
                            .map(|i| i.checked_sub(delay * channels).map_or(0.0, |j| source[j]))
                            .collect();
                        let solo: Vec<_> = solo.into_iter().map(f64::from).collect();
                        for i in 0..input.len() {
                            max_sample = max_sample.max((solo[i] - expected[i]).abs());
                            expected_sum[i] += expected[i];
                            solo_sum[i] += solo[i];
                        }
                        for ch in 0..channels {
                            max_peak = max_peak.max(
                                peak(&solo, channels, ch).abs_diff(peak(&expected, channels, ch)),
                            );
                        }
                    }
                    let actual: Vec<_> = actual.into_iter().map(f64::from).collect();
                    let mut closure = 0.0_f64;
                    for i in 0..actual.len() {
                        max_sample = max_sample.max((actual[i] - expected_sum[i]).abs());
                        closure = closure.max((actual[i] - solo_sum[i]).abs());
                    }
                    let mut complex = 0.0_f64;
                    for frequency in [
                        20.0, 50.0, 120.0, 250.0, 700.0, 1500.0, 3200.0, 6000.0, 10000.0, 16000.0,
                    ] {
                        let a = transfer(&actual, channels, frequency, rate);
                        let b = transfer(&expected_sum, channels, frequency, rate);
                        complex = complex.max((a.0 - b.0).hypot(a.1 - b.1));
                    }
                    println!(
                        "{}",
                        serde_json::json!({"case":"unity_time_impulse","rate":rate,"bands":count,"channels":channels,"lookahead_ms":lookahead,"max_sample_error":max_sample,"max_complex_error":complex,"max_peak_frames":max_peak,"solo_sum_error":closure})
                    );
                    assert!(
                        max_sample < SAMPLE,
                        "sample {max_sample}: {rate}/{count}/{channels}/{lookahead}"
                    );
                    assert!(closure < SAMPLE, "solo sum {closure}");
                    assert!(max_peak <= 1, "peak {max_peak}");
                    assert!(
                        complex < COMPLEX,
                        "complex {complex}: {rate}/{count}/{channels}/{lookahead}"
                    );
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 120);
}

#[test]
fn crossover_adjacent_bursts_and_quiet_onsets_reconstruct_in_both_modes() {
    let mut cases = 0;
    for rate in [44_100, 96_000, 192_000] {
        for count in 1..=5 {
            let cuts = &[120.0, 700.0, 6000.0, 12000.0][..count - 1];
            for channels in [1, 2, 6] {
                for spectral in [false, true] {
                    let params = neutral(count, cuts, spectral, 0.0);
                    let mut plugin = make(rate, channels, params, None);
                    let delay = plugin.latency_samples();
                    assert_eq!(delay, if spectral { 1024 } else { 0 });
                    let mut input = vec![0.0; 8192 * channels];
                    // Quiet musical-like onset at frame zero, then short bursts
                    // immediately below/above every selected crossover frequency.
                    for frame in 0..4096 {
                        let phase = std::f64::consts::TAU * frame as f64 / f64::from(rate);
                        let mut value = 1e-4
                            * (-(frame as f64) / 1200.0).exp()
                            * ((phase * 220.0).sin() + 0.3 * (phase * 440.0).sin());
                        for (i, cut) in cuts.iter().enumerate() {
                            let start = 512 + i * 640;
                            if (start..start + 256).contains(&frame) {
                                let n = (frame - start) as f64;
                                let window = (std::f64::consts::PI * n / 256.0).sin().powi(2);
                                value += 0.2
                                    * window
                                    * ((phase * cut * 0.99).sin() + (phase * cut * 1.01).sin());
                            }
                        }
                        for ch in 0..channels {
                            input[frame * channels + ch] = (value / (ch + 1) as f64) as f32;
                        }
                    }
                    let expected = if spectral {
                        (0..input.len())
                            .map(|i| {
                                i.checked_sub(delay * channels)
                                    .map_or(0.0, |j| f64::from(input[j]))
                            })
                            .collect::<Vec<_>>()
                    } else {
                        let references = bands(&input, channels, rate, cuts);
                        (0..input.len())
                            .map(|i| references.iter().map(|b| b[i]).sum::<f64>())
                            .collect()
                    };
                    let output = render(&mut plugin, rate, channels, &input, 137);
                    let error = output
                        .iter()
                        .zip(&expected)
                        .map(|(&a, &b)| (f64::from(a) - b).abs())
                        .fold(0.0_f64, f64::max);
                    println!(
                        "{}",
                        serde_json::json!({"case":"unity_band_edge_bursts","rate":rate,"bands":count,"channels":channels,"spectral":spectral,"max_sample_error":error})
                    );
                    assert!(
                        error < SAMPLE,
                        "band edges {error}: {rate}/{count}/{channels}/{spectral}"
                    );
                    plugin.reset();
                    assert_eq!(output, render(&mut plugin, rate, channels, &input, 1024));
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 90);
}
