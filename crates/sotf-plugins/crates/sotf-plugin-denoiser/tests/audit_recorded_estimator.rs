//! Preselected recorded-source measurements retain startup and estimator tradeoffs.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_denoiser::{DenoiserPlugin, DenoiserPluginParams};

const RATE: u32 = 48_000;
const SOURCES: [(&str, &[u8], usize, f32); 6] = [
    (
        "speech-1272",
        include_bytes!("../../sotf-plugin-speech-denoiser/tests/fixtures/recorded/speech-0.pcm"),
        0,
        1.0,
    ),
    (
        "speech-1462",
        include_bytes!("../../sotf-plugin-speech-denoiser/tests/fixtures/recorded/speech-1.pcm"),
        0,
        1.0,
    ),
    (
        "speech-174",
        include_bytes!("../../sotf-plugin-speech-denoiser/tests/fixtures/recorded/speech-2.pcm"),
        0,
        1.0,
    ),
    (
        "phoneme-s",
        include_bytes!("../../sotf-plugin-de-esser/tests/fixtures/recorded/s.pcm"),
        12000,
        0.2,
    ),
    (
        "violin",
        include_bytes!("../../sotf-plugin-de-esser/tests/fixtures/recorded/violin.pcm"),
        0,
        0.2,
    ),
    (
        "cymbal",
        include_bytes!("../../sotf-plugin-de-esser/tests/fixtures/recorded/cymbal.pcm"),
        0,
        0.2,
    ),
];

fn energy(samples: &[f32]) -> f64 {
    samples.iter().map(|&x| f64::from(x).powi(2)).sum()
}

fn metrics(output: &[f32], clean: &[f32]) -> serde_json::Value {
    assert_eq!(output.len(), clean.len());
    let clean_energy = energy(clean);
    assert!(clean_energy > 1e-12);
    let output_energy = energy(output);
    let dot: f64 = output
        .iter()
        .zip(clean)
        .map(|(&a, &b)| f64::from(a) * f64::from(b))
        .sum();
    let error: f64 = output
        .iter()
        .zip(clean)
        .map(|(&a, &b)| (f64::from(a) - f64::from(b)).powi(2))
        .sum();
    serde_json::json!({"clean_energy":clean_energy,"output_energy":output_energy,"dot":dot,"error_energy":error,
        "projection":dot/clean_energy,"sdr_db":10.0*(clean_energy/error.max(1e-30)).log10()})
}

fn mix(clean: &[f32], condition: &str) -> Vec<f32> {
    if condition == "clean" {
        return clean.to_vec();
    }
    let mut state = 0x9a7f2513_u32;
    let mut low = 0.0_f32;
    let mut noise: Vec<f32> = (0..clean.len())
        .map(|i| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let white = (f64::from(state) / f64::from(u32::MAX) * 2.0 - 1.0) as f32;
            low += 0.05 * (white - low);
            let envelope = if condition == "step" && i >= clean.len() / 3 && i < clean.len() * 2 / 3
            {
                4.0
            } else {
                0.25
            };
            (low + 0.15 * white) * envelope
        })
        .collect();
    // Whole-clip 0 dB added-noise SNR, with the same nonstationary boundaries
    // for every source. This normalization uses source/noise, never output.
    let scale = (energy(clean) / energy(&noise)).sqrt() as f32;
    for (noise, &signal) in noise.iter_mut().zip(clean) {
        *noise = signal + *noise * scale;
    }
    noise
}

fn processor(channels: usize, low_latency: bool) -> DenoiserPlugin {
    let mut p = DenoiserPlugin::from_params(
        channels,
        DenoiserPluginParams {
            low_latency,
            spatial_denoise: false,
            polyphonic_detection: false,
            psychoacoustic_masking: false,
            ..Default::default()
        },
    );
    p.initialize(f64::from(RATE)).unwrap();
    p
}

fn render(p: &mut DenoiserPlugin, input: &[f32], channels: usize, parts: &[usize]) -> Vec<f32> {
    let latency = p.latency_samples();
    let mut output = input.to_vec();
    let mut at = 0;
    let mut call = 0;
    while at < input.len() / channels {
        let frames = parts[call % parts.len()].min(input.len() / channels - at);
        p.process_in_place(
            &mut output[at * channels..(at + frames) * channels],
            &ProcessContext::new(RATE, frames),
        )
        .unwrap();
        at += frames;
        call += 1;
    }
    let mut tail = vec![0.0; p.drain_output_frames_max() * channels];
    let mut complete = false;
    for _ in 0..32 {
        let step = p.drain(&mut tail, &ProcessContext::new(RATE, 0)).unwrap();
        output.extend_from_slice(&tail[..step.frames * channels]);
        if step.complete {
            complete = true;
            break;
        }
    }
    assert!(complete);
    let source_frames = input.len() / channels;
    let hop = latency / 2;
    let support_end = 2 * latency + ((source_frames - 1) / hop) * hop;
    assert_eq!(output.len(), support_end * channels);
    assert!(output.iter().all(|x| x.is_finite()));
    output[latency * channels..].to_vec()
}

#[test]
fn recorded_sources_report_clean_retention_and_stationary_or_step_noise_tradeoffs() {
    let mut cases = 0;
    for (id, bytes, crop, gain) in SOURCES {
        let clean: Vec<f32> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .skip(crop)
            .map(|s| f32::from(i16::from_le_bytes(*s)) / 32768.0 * gain)
            .collect();
        assert!(clean.len() > 24_000);
        for condition in ["clean", "stationary", "step"] {
            let input = mix(&clean, condition);
            for low_latency in [false, true] {
                let mut p = processor(1, low_latency);
                let output = render(&mut p, &input, 1, &[257]);
                p.reset();
                assert_eq!(output, render(&mut p, &input, 1, &[1, 31, 4096]));
                let mut windows = Vec::new();
                for (label, start, end) in [
                    ("onset", 0, 4800),
                    ("early", 4800, 24000),
                    ("body", 24000, clean.len()),
                    ("full", 0, clean.len()),
                ] {
                    windows.push(serde_json::json!({"label":label,"start":start,"end":end,
                        "input":metrics(&input[start..end],&clean[start..end]),"output":metrics(&output[start..end],&clean[start..end])}));
                }
                println!(
                    "{}",
                    serde_json::json!({"measurement":"recorded_estimator","source":id,"condition":condition,
                    "low_latency":low_latency,"frames":clean.len(),"latency_frames":p.latency_samples(),"aligned_after_source_energy":energy(&output[clean.len()..]),"windows":windows})
                );
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 36);
}

#[test]
fn recorded_stereo_polarity_and_channel_order_match_mono_through_eof() {
    for (_, bytes, _, _) in SOURCES.into_iter().take(3) {
        let clean: Vec<f32> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|s| f32::from(i16::from_le_bytes(*s)) / 32768.0)
            .collect();
        let input = mix(&clean, "step");
        let stereo: Vec<f32> = input.iter().flat_map(|&x| [x, -x]).collect();
        for low_latency in [false, true] {
            let mono = render(&mut processor(1, low_latency), &input, 1, &[257]);
            let pair = render(&mut processor(2, low_latency), &stereo, 2, &[31, 4096]);
            for (frame, sample) in pair.as_chunks::<2>().0.iter().zip(mono) {
                assert_eq!(frame[0], sample);
                assert_eq!(frame[1], -sample);
            }
        }
    }
}
