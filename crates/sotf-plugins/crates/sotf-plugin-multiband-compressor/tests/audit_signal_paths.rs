//! Independent analog-prototype and dynamics trajectories for audit entry 34.
//!
//! Bounds fixed before execution: sample/solo closure 2e-5, complex 2e-3,
//! transient peak index one frame, envelope 0.12 dB, stereo gain 0.15 dB.
//! LR4 references use independently derived f64 Tustin coefficients and DF-I
//! state, never production coefficients, filters, detector, or fast math.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_multiband_compressor::{
    MultibandCompressorPlugin, MultibandCompressorPluginParams,
};
use std::f64::consts::{PI, SQRT_2};

#[global_allocator]
static ALLOCATOR: sotf_host::CountingAlloc = sotf_host::CountingAlloc;
const SAMPLE: f64 = 2e-5;
const COMPLEX: f64 = 2e-3;
const ENVELOPE_DB: f64 = 0.12;
const STEREO_DB: f64 = 0.15;

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
    params: MultibandCompressorPluginParams,
    solo: Option<usize>,
) -> MultibandCompressorPlugin {
    let mut plugin = MultibandCompressorPlugin::try_from_params(channels, params, rate).unwrap();
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
    plugin: &mut MultibandCompressorPlugin,
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

#[test]
fn no_reduction_impulses_reconstruct_cascade_and_preserve_interband_timing() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for count in 1..=5 {
            let cuts = &[120.0, 700.0, 6000.0, 12000.0][..count - 1];
            for lookahead in [0.0, 5.0] {
                for (channels, ms_mode) in [(1, false), (2, false), (2, true)] {
                    let delay = if lookahead == 0.0 {
                        0
                    } else {
                        (lookahead * f64::from(rate) / 1000.0_f64).round() as usize
                    };
                    let mut input = vec![0.0; (8192 + delay) * channels];
                    input[0] = 0.5;
                    if channels == 2 {
                        input[1] = -0.125;
                    }
                    let reference = bands(&input, channels, rate, cuts);
                    let params = MultibandCompressorPluginParams {
                        num_bands: count,
                        crossover_frequencies: cuts.iter().map(|&x| x as f32).collect(),
                        ratio: 1.0,
                        mix: 1.0,
                        per_band_lookahead_ms: lookahead as f32,
                        ms_mode,
                        ..Default::default()
                    };
                    let mut combined = make(rate, channels, params.clone(), None);
                    assert_eq!(combined.latency_samples(), delay);
                    let combined = render(&mut combined, rate, channels, &input, 257);
                    let mut sum = vec![0.0; input.len()];
                    let mut oracle_sum = vec![0.0; input.len()];
                    let (mut maximum, mut peak_error) = (0.0_f64, 0_usize);
                    for (band, base) in reference.iter().enumerate() {
                        let mut plugin = make(rate, channels, params.clone(), Some(band));
                        let actual =
                            render(&mut plugin, rate, channels, &input, [1, 73, 1024][band % 3]);
                        let expected: Vec<_> = (0..input.len())
                            .map(|i| {
                                i.checked_sub(delay * channels)
                                    .map_or(0.0, |source| base[source])
                            })
                            .collect();
                        for ((index, &sample), &target) in actual.iter().enumerate().zip(&expected)
                        {
                            maximum = maximum.max((f64::from(sample) - target).abs());
                            sum[index] += f64::from(sample);
                            oracle_sum[index] += target;
                        }
                        let actual: Vec<_> = actual.into_iter().map(f64::from).collect();
                        for ch in 0..channels {
                            peak_error = peak_error.max(
                                peak(&actual, channels, ch).abs_diff(peak(&expected, channels, ch)),
                            );
                        }
                    }
                    let closure = combined
                        .iter()
                        .zip(&sum)
                        .map(|(&x, &s)| (f64::from(x) - s).abs())
                        .fold(0.0_f64, f64::max);
                    let combined: Vec<_> = combined.into_iter().map(f64::from).collect();
                    maximum = maximum.max(
                        combined
                            .iter()
                            .zip(&oracle_sum)
                            .map(|(&x, &y)| (x - y).abs())
                            .fold(0.0_f64, f64::max),
                    );
                    let mut complex = 0.0_f64;
                    for frequency in [
                        20.0, 50.0, 120.0, 250.0, 700.0, 1500.0, 3200.0, 6000.0, 10000.0, 16000.0,
                    ] {
                        let a = transfer(&combined, channels, frequency, rate);
                        let b = transfer(&oracle_sum, channels, frequency, rate);
                        complex = complex.max((a.0 - b.0).hypot(a.1 - b.1));
                    }
                    assert!(
                        maximum < SAMPLE,
                        "sample error {maximum}: {rate}/{count}/{lookahead}/{channels}/{ms_mode}"
                    );
                    assert!(closure < SAMPLE, "solo reconstruction error {closure}");
                    assert!(
                        complex < COMPLEX,
                        "complex error {complex}: {rate}/{count}/{lookahead}"
                    );
                    assert!(
                        peak_error <= 1,
                        "interband transient peak displaced {peak_error} frames"
                    );
                    println!(
                        "{}",
                        serde_json::json!({"case":"unity_cascade_impulse","rate":rate,"bands":count,"lookahead_ms":lookahead,
                "channels":channels,"ms_mode":ms_mode,"max_sample_error":maximum,"solo_sum_error":closure,
                "max_complex_error":complex,"max_peak_error_frames":peak_error})
                    );
                }
            }
        }
    }
}

fn reduction(level: f64, threshold: f64, ratio: f64, knee: f64) -> f64 {
    let slope = 1.0 - 1.0 / ratio;
    if knee == 0.0 {
        (level - threshold).max(0.0) * slope
    } else if level < threshold - knee / 2.0 {
        0.0
    } else if level > threshold + knee / 2.0 {
        (level - threshold) * slope
    } else {
        (level - threshold + knee / 2.0).powi(2) / (2.0 * knee) * slope
    }
}
struct EnvelopeParams {
    rms: bool,
    link: f64,
    knee: f64,
    attack: f64,
    release: f64,
    threshold: f64,
}
fn envelopes(band: &[f64], channels: usize, rate: u32, params: EnvelopeParams) -> Vec<f64> {
    let EnvelopeParams {
        rms,
        link,
        knee,
        attack,
        release,
        threshold,
    } = params;
    let window = (0.01 * f64::from(rate)).round() as usize;
    let mut energies = vec![vec![0.0; window]; channels];
    let mut sums = vec![0.0; channels];
    let mut state = vec![0.0; channels];
    let mut output = vec![0.0; band.len()];
    let attack = (-1.0 / (attack * 0.001 * f64::from(rate))).exp();
    let release = (-1.0 / (release * 0.001 * f64::from(rate))).exp();
    let mut levels = vec![0.0; channels];
    for (frame, samples) in band.chunks_exact(channels).enumerate() {
        for (ch, &sample) in samples.iter().enumerate() {
            let energy = sample * sample;
            sums[ch] += energy - energies[ch][frame % window];
            energies[ch][frame % window] = energy;
            levels[ch] = if rms {
                (sums[ch].max(0.0) / window as f64).sqrt()
            } else {
                sample.abs()
            };
        }
        let linked = 20.0
            * levels
                .iter()
                .copied()
                .fold(0.0_f64, f64::max)
                .max(1e-10)
                .log10();
        for ch in 0..channels {
            let per = 20.0 * levels[ch].max(1e-10).log10();
            let target = reduction(per * (1.0 - link) + linked * link, threshold, 4.0, knee);
            let coefficient = if target > state[ch] { attack } else { release };
            state[ch] = target + coefficient * (state[ch] - target);
            output[frame * channels + ch] = state[ch];
        }
    }
    output
}

#[test]
fn near_threshold_modulation_separates_detector_knee_attack_release_and_link() {
    for rate in [44_100, 96_000] {
        for rms in [false, true] {
            for knee in [0.0, 6.0] {
                for attack in [0.1, 10.0] {
                    for release in [20.0, 200.0] {
                        for link in [0.0, 0.5, 1.0] {
                            let input: Vec<_> = (0..rate as usize / 2)
                                .flat_map(|n| {
                                    let t = n as f64 / f64::from(rate);
                                    let m = 1.5 * (2.0 * PI * 3.7 * t).sin()
                                        + 0.25 * (2.0 * PI * 53.0 * t).sin();
                                    [
                                        10.0_f64.powf((-20.0 + m) / 20.0) as f32,
                                        10.0_f64.powf((-20.0 - m) / 20.0) as f32,
                                    ]
                                })
                                .collect();
                            let params = MultibandCompressorPluginParams {
                                num_bands: 1,
                                ratio: 4.0,
                                threshold_db: -20.0,
                                knee_db: knee as f32,
                                attack_ms: attack as f32,
                                release_ms: release as f32,
                                link_amount: link as f32,
                                detection_mode: Some(if rms { "RMS" } else { "Peak" }.into()),
                                ..Default::default()
                            };
                            let mut plugin = make(rate, 2, params, None);
                            let actual = render(&mut plugin, rate, 2, &input, 257);
                            let band: Vec<_> = input.iter().copied().map(f64::from).collect();
                            let expected = envelopes(
                                &band,
                                2,
                                rate,
                                EnvelopeParams {
                                    rms,
                                    link,
                                    knee,
                                    attack,
                                    release,
                                    threshold: -20.0,
                                },
                            );
                            let mut worst = 0.0_f64;
                            for ((&sample, &source), &gr) in
                                actual.iter().zip(&input).zip(&expected)
                            {
                                let measured = -20.0
                                    * (f64::from(sample) / f64::from(source))
                                        .abs()
                                        .max(1e-12)
                                        .log10();
                                worst = worst.max((measured - gr).abs());
                            }
                            assert!(
                                worst < ENVELOPE_DB,
                                "near-threshold GR error {worst}: {rate}/{rms}/{knee}/{attack}/{release}/{link}"
                            );
                            if link == 1.0 {
                                for (actual, input) in actual
                                    .as_chunks::<2>()
                                    .0
                                    .iter()
                                    .zip(input.as_chunks::<2>().0)
                                {
                                    let delta = 20.0
                                        * ((f64::from(actual[0]) / f64::from(input[0]))
                                            / (f64::from(actual[1]) / f64::from(input[1])))
                                        .abs()
                                        .log10();
                                    assert!(
                                        delta.abs() < 0.001,
                                        "linked near-threshold image drift {delta}"
                                    );
                                }
                            }
                            println!(
                                "{}",
                                serde_json::json!({"case":"near_threshold_modulation","rate":rate,"rms":rms,"knee_db":knee,
                "attack_ms":attack,"release_ms":release,"link":link,"max_envelope_error_db":worst})
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn linked_multiband_transients_match_per_band_independent_gain_trajectories() {
    for rate in [48_000, 96_000] {
        for rms in [false, true] {
            for link in [0.0, 0.5, 1.0] {
                let input: Vec<_> = (0..rate as usize / 4)
                    .flat_map(|n| {
                        let t = n as f64 / f64::from(rate);
                        let m = 1.5 * (2.0 * PI * 7.3 * t).sin();
                        let wave = |phase: f64| {
                            [100.0, 1000.0, 6000.0]
                                .iter()
                                .enumerate()
                                .map(|(i, &f)| {
                                    0.09 * (2.0 * PI * f * t + phase * (i + 1) as f64).sin()
                                })
                                .sum::<f64>()
                        };
                        [
                            (wave(0.0) * 10.0_f64.powf(m / 20.0)) as f32,
                            (wave(0.37) * 10.0_f64.powf(-m / 20.0)) as f32,
                        ]
                    })
                    .collect();
                let reference = bands(&input, 2, rate, &[400.0, 3000.0]);
                let params = MultibandCompressorPluginParams {
                    num_bands: 3,
                    crossover_frequencies: vec![400.0, 3000.0],
                    ratio: 4.0,
                    threshold_db: -24.0,
                    knee_db: 3.0,
                    attack_ms: 5.0,
                    release_ms: 100.0,
                    link_amount: link as f32,
                    detection_mode: Some(if rms { "RMS" } else { "Peak" }.into()),
                    ..Default::default()
                };
                let mut sum = vec![0.0; input.len()];
                for (band, source) in reference.iter().enumerate() {
                    let expected = envelopes(
                        source,
                        2,
                        rate,
                        EnvelopeParams {
                            rms,
                            link,
                            knee: 3.0,
                            attack: 5.0,
                            release: 100.0,
                            threshold: -24.0,
                        },
                    );
                    let mut plugin = make(rate, 2, params.clone(), Some(band));
                    let actual = render(&mut plugin, rate, 2, &input, [1, 73, 1024][band]);
                    let (mut worst, mut checked) = (0.0_f64, 0_usize);
                    for (i, ((&sample, &source), &gr)) in
                        actual.iter().zip(source).zip(&expected).enumerate()
                    {
                        sum[i] += f64::from(sample);
                        if source.abs() < 0.01 {
                            continue;
                        }
                        let measured =
                            -20.0 * (f64::from(sample) / source).abs().max(1e-12).log10();
                        worst = worst.max((measured - gr).abs());
                        checked += 1;
                    }
                    assert!(checked > 1000);
                    assert!(
                        worst < STEREO_DB,
                        "multiband linked trajectory {worst}: {rate}/{rms}/{link}/{band}"
                    );
                    println!(
                        "{}",
                        serde_json::json!({"case":"linked_multiband_trajectory","rate":rate,"rms":rms,
                "link":link,"band":band,"checked_samples":checked,"max_gain_error_db":worst})
                    );
                }
                let mut plugin = make(rate, 2, params, None);
                let combined = render(&mut plugin, rate, 2, &input, 257);
                let closure = combined
                    .iter()
                    .zip(&sum)
                    .map(|(&x, &y)| (f64::from(x) - y).abs())
                    .fold(0.0_f64, f64::max);
                assert!(closure < SAMPLE, "active solo/sum closure {closure}");
            }
        }
    }
}
