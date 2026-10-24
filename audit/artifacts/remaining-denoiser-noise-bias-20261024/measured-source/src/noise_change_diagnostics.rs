//! Qualify a common-change score without changing production noise estimates.
//!
//! Exact tails apply only to real white Gaussian noise, proper interior bins
//! and one raw periodogram from a disjoint sample window. Other controls are
//! empirical counterexamples, never evidence for an exact Gaussian tail.

// Rust guideline compliant 2026-02-21
use crate::{DenoiserPlugin, DenoiserPluginParams};
use sotf_host::{ParametricInPlacePlugin, ProcessContext};

const RATE: usize = 48_000;
const LAG: usize = 4;

struct RawReference {
    ring: [Vec<f32>; LAG],
    stamps: [usize; LAG],
    seen: usize,
    previous: Option<usize>,
}

impl RawReference {
    fn new(bins: usize) -> Self {
        Self {
            ring: std::array::from_fn(|_| vec![0.0; bins]),
            stamps: [0; LAG],
            seen: 0,
            previous: None,
        }
    }

    // Caller supplies consecutive, complete source windows and raw pre-gain
    // powers. No bootstrap, minimum, averaged PSD or adaptive selection enters.
    fn push_full_raw_frame(&mut self, frame: usize, powers: &[f32]) -> Option<(usize, f64)> {
        if let Some(previous) = self.previous {
            assert_eq!(frame, previous + 1);
        }
        self.previous = Some(frame);
        let slot = self.seen % LAG;
        let result = if self.seen >= LAG {
            let reference = self.stamps[slot];
            assert_eq!(frame - reference, LAG);
            let score = powers
                .iter()
                .zip(&self.ring[slot])
                .map(|(&now, &old)| {
                    let (now, old) = (f64::from(now), f64::from(old));
                    if now + old > 0.0 {
                        (now - old) / (now + old)
                    } else {
                        0.0
                    }
                })
                .sum::<f64>()
                / powers.len() as f64;
            Some((reference, score))
        } else {
            None
        };
        self.ring[slot].copy_from_slice(powers);
        self.stamps[slot] = frame;
        self.seen += 1;
        result
    }

    fn reset(&mut self) {
        for row in &mut self.ring {
            row.fill(0.0);
        }
        self.stamps.fill(0);
        self.seen = 0;
        self.previous = None;
    }
}

fn threshold(size: usize) -> f64 {
    // Exact rational Irwin-Hall oracle, fixed before measuring these controls.
    // Hourly familywise budget .001 via per-frame union bound, Gaussian only.
    match size {
        2048 => 0.354_810_059_064_831_8,
        512 => 0.685_890_030_027_867_1,
        _ => unreachable!(),
    }
}

struct Random {
    state: u32,
    spare: Option<f32>,
}
impl Random {
    fn new(seed: u32) -> Self {
        assert_ne!(seed, 0);
        Self {
            state: seed,
            spare: None,
        }
    }
    fn word(&mut self) -> u32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;
        self.state
    }
    fn uniform(&mut self) -> f32 {
        (f64::from(self.word()) / f64::from(u32::MAX) * 2.0 - 1.0) as f32
    }
    fn gaussian(&mut self) -> f32 {
        if let Some(value) = self.spare.take() {
            return value;
        }
        let u = (f64::from(self.word()) + 0.5) / 4_294_967_296.0;
        let angle = std::f64::consts::TAU * (f64::from(self.word()) + 0.5) / 4_294_967_296.0;
        let radius = (-2.0 * u.ln()).sqrt();
        self.spare = Some((radius * angle.sin()) as f32);
        (radius * angle.cos()) as f32
    }
}

fn wanted_envelope(time: f64) -> f64 {
    ((time - 0.25) / 0.02).clamp(0.0, 1.0) * ((0.77 - time) / 0.02).clamp(0.0, 1.0)
}

fn synthetic(condition: &str, seed: u32) -> Vec<f32> {
    let mut random = Random::new(seed);
    let mut low95 = 0.001 * random.gaussian();
    let mut low995 = 0.01 * random.gaussian();
    let mut high_pass = 0.0_f64;
    let mut low_pass = 0.0_f64;
    let hp = (-std::f64::consts::TAU * 1000.0 / RATE as f64).exp();
    let lp = (-std::f64::consts::TAU * 6000.0 / RATE as f64).exp();
    (0..RATE)
        .map(|frame| {
            let t = frame as f64 / RATE as f64;
            let normal = random.gaussian();
            low95 = 0.95 * low95 + (1.0_f32 - 0.95_f32.powi(2)).sqrt() * 0.001 * normal;
            low995 = 0.995 * low995 + (1.0_f32 - 0.995_f32.powi(2)).sqrt() * 0.01 * normal;
            let step = frame > 12000 && frame < 36000;
            match condition {
                "stationary_gaussian" => 0.01 * normal,
                "stationary_uniform" => 0.01 * 3.0_f32.sqrt() * random.uniform(),
                "stationary_ar95" => 10.0 * low95,
                "stationary_ar995" => low995,
                "gaussian_step_10db" => {
                    0.001
                        * normal
                        * if step {
                            10.0_f32.powf(10.0 / 20.0)
                        } else {
                            1.0
                        }
                }
                "gaussian_step_20db" => 0.001 * normal * if step { 10.0 } else { 1.0 },
                "gaussian_step_40db" => 0.001 * normal * if step { 100.0 } else { 1.0 },
                "uniform_step_40db" => {
                    0.001 * 3.0_f32.sqrt() * random.uniform() * if step { 100.0 } else { 1.0 }
                }
                "ar95_step_40db" => low95 * if step { 100.0 } else { 1.0 },
                "wanted_harmonic" => {
                    let signal = (1..=12)
                        .map(|h| (std::f64::consts::TAU * 137.0 * h as f64 * t).sin() / h as f64)
                        .sum::<f64>();
                    (0.02 * wanted_envelope(t) * signal) as f32 + 0.0001 * normal
                }
                "wanted_unvoiced_bandpass" => {
                    high_pass = hp * high_pass + (1.0 - hp) * f64::from(normal);
                    let high = f64::from(normal) - high_pass;
                    low_pass = lp * low_pass + (1.0 - lp) * high;
                    (0.02 * wanted_envelope(t) * low_pass) as f32 + 0.0001 * random.gaussian()
                }
                _ => unreachable!(),
            }
        })
        .collect()
}

fn process_trace(source: &[f32], low_latency: bool, reduction: f32) -> Vec<(usize, usize, f64)> {
    let mut plugin = DenoiserPlugin::from_params(
        1,
        DenoiserPluginParams {
            low_latency,
            reduction_db: reduction,
            spatial_denoise: false,
            polyphonic_detection: false,
            psychoacoustic_masking: false,
            ..Default::default()
        },
    );
    plugin.initialize(RATE as f64).unwrap();
    let size = plugin.config.fft_size;
    let hop = plugin.config.hop_size;
    let first = (100.0 * size as f64 / RATE as f64).ceil() as usize;
    let last = (4000.0 * size as f64 / RATE as f64).floor() as usize;
    let bins: Vec<usize> = (first..=last).step_by(2).collect();
    assert_eq!(bins.len(), if low_latency { 21 } else { 83 });
    let mut powers = vec![0.0_f32; bins.len()];
    let mut reference = RawReference::new(bins.len());
    let mut trace = Vec::new();
    for (index, input) in source.chunks(hop).enumerate() {
        let mut block = input.to_vec();
        let frames = block.len();
        plugin
            .process_in_place(&mut block, &ProcessContext::new(RATE as u32, frames))
            .unwrap();
        if frames != hop || plugin.mcra.frame_counter[0] < plugin.mcra.bootstrap_frames {
            continue;
        }
        let frame = index + 1;
        assert!(
            frame * hop >= size,
            "raw window must contain complete source support"
        );
        for (out, &bin) in powers.iter_mut().zip(&bins) {
            *out = plugin.decision_directed.prev_power[0][bin];
        }
        if let Some((old, score)) = reference.push_full_raw_frame(frame, &powers) {
            assert!(score.is_finite() && (-1.0..=1.0).contains(&score));
            trace.push((frame, old, score));
        }
    }
    trace
}

#[test]
fn fixed_raw_reference_lifecycle_reset_grouping_and_heap_are_exact() {
    for bins in [21, 83] {
        let run = |groups: &[usize]| {
            let mut reference = RawReference::new(bins);
            let mut out = Vec::new();
            let mut frame = 10;
            for &group in groups {
                for _ in 0..group {
                    let powers = vec![frame as f32; bins];
                    if let Some(value) = reference.push_full_raw_frame(frame, &powers) {
                        assert_eq!(value.0, frame - LAG);
                        let expected = 4.0 / (2 * frame - LAG) as f64;
                        assert!((value.1 - expected).abs() < 1e-14);
                        out.push(value);
                    }
                    frame += 1;
                }
            }
            out
        };
        assert_eq!(run(&[1; 32]), run(&[3, 7, 2, 11, 9]));
        let mut reference = RawReference::new(bins);
        let powers = vec![1.0; bins];
        #[cfg(feature = "qa")]
        sotf_host::test_utils::assert_no_allocs_or_deallocs("raw reference lifecycle", || {
            for frame in 0..12 {
                let _ = reference.push_full_raw_frame(frame, &powers);
            }
            reference.reset();
            for frame in 100..104 {
                assert!(reference.push_full_raw_frame(frame, &powers).is_none());
            }
            assert_eq!(
                reference.push_full_raw_frame(104, &powers),
                Some((100, 0.0))
            );
            reference.reset();
        });
        #[cfg(not(feature = "qa"))]
        {
            for frame in 0..12 {
                let _ = reference.push_full_raw_frame(frame, &powers);
            }
            reference.reset();
            for frame in 100..104 {
                assert!(reference.push_full_raw_frame(frame, &powers).is_none());
            }
            assert_eq!(
                reference.push_full_raw_frame(104, &powers),
                Some((100, 0.0))
            );
        }
    }
    let source = synthetic("gaussian_step_40db", 0x1234_5678);
    for low in [false, true] {
        assert_eq!(
            process_trace(&source, low, 0.0),
            process_trace(&source, low, 10.0),
            "applied gain must never select the raw reference"
        );
    }
    println!(
        "raw_reference_lifecycle bins=21,83 fixed_lag=4 reset=true grouping_exact=true gain_independent=true heap_allocations=0 heap_frees=0"
    );
}

#[test]
fn common_change_score_reports_fixed_empirical_controls_without_a_rebase() {
    let conditions = [
        "stationary_gaussian",
        "stationary_uniform",
        "stationary_ar95",
        "stationary_ar995",
        "gaussian_step_10db",
        "gaussian_step_20db",
        "gaussian_step_40db",
        "uniform_step_40db",
        "ar95_step_40db",
        "wanted_harmonic",
        "wanted_unvoiced_bandpass",
    ];
    let mut reports = 0;
    for ordinal in 1_u32..=64 {
        let seed = ordinal.wrapping_mul(0x9E37_79B9);
        for condition in conditions {
            let source = synthetic(condition, seed);
            for low in [false, true] {
                let size = if low { 512 } else { 2048 };
                let hop = size / 2;
                let trace = process_trace(&source, low, 10.0);
                let events:Vec<_>=trace.iter().filter(|(_,_,s)|*s>=threshold(size)).map(|(now,old,score)|serde_json::json!({"frame":now,"reference_frame":old,"window_end_time":*now as f64*hop as f64/RATE as f64,"reference_end_time":*old as f64*hop as f64/RATE as f64,"score":score})).collect();
                println!(
                    "COMMON_CHANGE {}",
                    serde_json::json!({"condition":condition,"ordinal":ordinal,"seed":seed,"low_latency":low,"fft_size":size,"hop":hop,"threshold":threshold(size),"comparisons":trace.len(),"events":events,"production_rebase":false})
                );
                reports += 1;
            }
        }
    }
    const RECORDED: [(&str, &[u8], f32); 8] = [
        (
            "speech-1272",
            include_bytes!(
                "../../sotf-plugin-speech-denoiser/tests/fixtures/recorded/speech-0.pcm"
            ),
            1.0,
        ),
        (
            "speech-1462",
            include_bytes!(
                "../../sotf-plugin-speech-denoiser/tests/fixtures/recorded/speech-1.pcm"
            ),
            1.0,
        ),
        (
            "speech-174",
            include_bytes!(
                "../../sotf-plugin-speech-denoiser/tests/fixtures/recorded/speech-2.pcm"
            ),
            1.0,
        ),
        (
            "s",
            include_bytes!("../../sotf-plugin-de-esser/tests/fixtures/recorded/s.pcm"),
            0.25,
        ),
        (
            "sh",
            include_bytes!("../../sotf-plugin-de-esser/tests/fixtures/recorded/sh.pcm"),
            0.25,
        ),
        (
            "p",
            include_bytes!("../../sotf-plugin-de-esser/tests/fixtures/recorded/p.pcm"),
            0.25,
        ),
        (
            "violin",
            include_bytes!("../../sotf-plugin-de-esser/tests/fixtures/recorded/violin.pcm"),
            0.25,
        ),
        (
            "cymbal",
            include_bytes!("../../sotf-plugin-de-esser/tests/fixtures/recorded/cymbal.pcm"),
            0.25,
        ),
    ];
    let manifest: serde_json::Value = serde_json::from_str(include_str!(
        "../../sotf-plugin-de-esser/tests/fixtures/recorded/manifest.json"
    ))
    .unwrap();
    for (id, bytes, gain) in RECORDED {
        let mut random = Random::new(0xA341_316C);
        let source: Vec<f32> = bytes
            .chunks_exact(2)
            .take(192000)
            .map(|b| {
                f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0 * gain
                    + 0.0001 * random.gaussian()
            })
            .collect();
        let regions = manifest["fixtures"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["id"] == id)
            .map(|v| v["regions"].clone())
            .unwrap_or(serde_json::Value::Null);
        for low in [false, true] {
            let size = if low { 512 } else { 2048 };
            let hop = size / 2;
            let trace = process_trace(&source, low, 10.0);
            let events:Vec<_>=trace.iter().filter(|(_,_,s)|*s>=threshold(size)).map(|(now,old,score)|serde_json::json!({"frame":now,"reference_frame":old,"window_end_time":*now as f64*hop as f64/RATE as f64,"source_window_center":(*now-1) as f64*hop as f64/RATE as f64,"score":score})).collect();
            println!(
                "COMMON_CHANGE {}",
                serde_json::json!({"condition":"recorded_wanted","source":id,"low_latency":low,"fft_size":size,"hop":hop,"threshold":threshold(size),"comparisons":trace.len(),"events":events,"fixed_manifest_regions":regions,"source_gain":gain,"stationary_gaussian_bed_rms":0.0001,"production_rebase":false})
            );
            reports += 1;
        }
    }
    assert_eq!(reports, 1424);
    println!(
        "common_change_diagnostic reports={reports} gaussian_exact_tail_scope_only=true uniform_colored_unvoiced_empirical=true production_rebase=false"
    );
}
