//! Recorded speech/noise and instrument measurements without discarded warm-up.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_speech_denoiser::{
    SpeechDenoiserModel, SpeechDenoiserPlugin, SpeechDenoiserPluginParams,
};
use std::time::Instant;

const RATE: u32 = 48_000;
const MODELS: [SpeechDenoiserModel; 3] = [
    SpeechDenoiserModel::RnnoiseFull,
    SpeechDenoiserModel::RnnoiseLegacyLq,
    SpeechDenoiserModel::RnnoiseLegacySh,
];
const SPEECH: [(&str, &[u8]); 3] = [
    (
        "1272-135031-0000",
        include_bytes!("fixtures/recorded/speech-0.pcm"),
    ),
    (
        "1462-170142-0000",
        include_bytes!("fixtures/recorded/speech-1.pcm"),
    ),
    (
        "174-168635-0000",
        include_bytes!("fixtures/recorded/speech-2.pcm"),
    ),
];

fn decode(bytes: &[u8]) -> Vec<f32> {
    let (samples, remainder) = bytes.as_chunks::<2>();
    assert!(remainder.is_empty());
    let mut samples: Vec<_> = samples
        .iter()
        .map(|s| f32::from(i16::from_le_bytes(*s)) / 32768.0)
        .collect();
    // Exercise a final partial model frame without selecting by model output.
    samples.truncate(samples.len() - 17);
    samples
}

fn energy(samples: &[f32]) -> f64 {
    samples.iter().map(|x| f64::from(*x).powi(2)).sum()
}

fn quality(estimate: &[f32], reference: &[f32]) -> serde_json::Value {
    assert_eq!(estimate.len(), reference.len());
    let reference_energy = energy(reference);
    let estimated_energy = energy(estimate);
    assert!(reference_energy > 1e-12);
    let dot = estimate
        .iter()
        .zip(reference)
        .map(|(a, b)| f64::from(*a) * f64::from(*b))
        .sum::<f64>();
    let projection = dot / reference_energy;
    let target_energy = projection * projection * reference_energy;
    let error = estimate
        .iter()
        .zip(reference)
        .map(|(a, b)| (f64::from(*a) - f64::from(*b)).powi(2))
        .sum::<f64>();
    let residual = estimate
        .iter()
        .zip(reference)
        .map(|(a, b)| (f64::from(*a) - projection * f64::from(*b)).powi(2))
        .sum::<f64>();
    let si_sdr =
        (target_energy > 1e-30).then(|| 10.0 * (target_energy / residual.max(1e-30)).log10());
    serde_json::json!({"projection":projection,
        "gain_db":10.0*(estimated_energy.max(1e-30)/reference_energy).log10(),
        "sdr_db":10.0*(reference_energy/error.max(1e-30)).log10(),
        "si_sdr_db":si_sdr,
        "correlation":dot/(estimated_energy*reference_energy).max(1e-60).sqrt()})
}

#[test]
fn measurement_distinguishes_gain_loss_distortion_and_silence() {
    let reference = [1.0, 1.0, -1.0, -1.0];
    let attenuated = [0.1, 0.1, -0.1, -0.1];
    let q = quality(&attenuated, &reference);
    assert!((q["projection"].as_f64().unwrap() - 0.1).abs() < 1e-7);
    assert!((q["gain_db"].as_f64().unwrap() + 20.0).abs() < 1e-5);
    assert!(q["si_sdr_db"].as_f64().unwrap() > 100.0);
    assert!(q["sdr_db"].as_f64().unwrap() < 1.0);
    let noisy = [2.0, 0.0, 0.0, -2.0];
    assert!(
        quality(&noisy, &reference)["si_sdr_db"]
            .as_f64()
            .unwrap()
            .abs()
            < 1e-12
    );
    assert!(quality(&[0.0; 4], &reference)["si_sdr_db"].is_null());
}

fn mix(clean: &[f32], snr: f64) -> Vec<f32> {
    let mut state = 0x9812abcd_u32;
    let mut low = 0.0_f32;
    let noise: Vec<_> = clean
        .iter()
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let white = (f64::from(state) / f64::from(u32::MAX) * 2.0 - 1.0) as f32;
            low += 0.05 * (white - low);
            low + 0.15 * white
        })
        .collect();
    let scale = (energy(clean) / (energy(&noise) * 10.0_f64.powf(snr / 10.0))).sqrt() as f32;
    let result: Vec<_> = clean
        .iter()
        .zip(noise)
        .map(|(s, n)| s + n * scale)
        .collect();
    assert!(result.iter().all(|v| v.abs() < 1.0));
    result
}

fn render(
    source: &[f32],
    channels: usize,
    model: SpeechDenoiserModel,
    strength: f32,
    blocks: &[usize],
) -> (Vec<f32>, usize, serde_json::Value) {
    let mut p = SpeechDenoiserPlugin::from_params(
        channels,
        SpeechDenoiserPluginParams {
            enabled: true,
            strength,
            model,
        },
    );
    p.initialize(f64::from(RATE)).unwrap();
    let latency = p.latency_samples();
    let mut result = source.to_vec();
    let mut offset = 0;
    let mut call = 0;
    let mut times =
        Vec::with_capacity(source.len() / channels / blocks.iter().copied().min().unwrap() + 1);
    while offset < source.len() / channels {
        let frames = blocks[call % blocks.len()].min(source.len() / channels - offset);
        let start = Instant::now();
        p.process_in_place(
            &mut result[offset * channels..(offset + frames) * channels],
            &ProcessContext::new(RATE, frames),
        )
        .unwrap();
        times.push(start.elapsed().as_nanos() as u64);
        offset += frames;
        call += 1;
    }
    let mut tail = vec![0.0; 480 * channels];
    let mut complete = false;
    for _ in 0..16 {
        let step = p.drain(&mut tail, &ProcessContext::new(RATE, 0)).unwrap();
        result.extend_from_slice(&tail[..step.frames * channels]);
        if step.complete {
            complete = true;
            break;
        }
    }
    assert!(complete);
    assert_eq!(result.len(), source.len() + latency * channels);
    assert!(result.iter().all(|v| v.is_finite()));
    times.sort_unstable();
    let stats = serde_json::json!({"process_callbacks":times.len(),"p50_ns":times[times.len()/2],
        "p95_ns":times[(times.len()-1)*95/100],"max_ns":times[times.len()-1],
        "ordinary_frames":blocks,"last_process_frames_may_be_partial":true,"drain_timing_included":false});
    (result, latency, stats)
}

#[test]
fn recorded_models_report_speech_quality_and_instrument_false_suppression() {
    let instruments = [
        (
            "violin",
            include_bytes!("../../sotf-plugin-de-esser/tests/fixtures/recorded/violin.pcm")
                .as_slice(),
        ),
        (
            "cymbal",
            include_bytes!("../../sotf-plugin-de-esser/tests/fixtures/recorded/cymbal.pcm")
                .as_slice(),
        ),
    ];
    let mut cases = 0;
    for model in MODELS {
        for (index, (id, bytes)) in SPEECH.into_iter().chain(instruments).enumerate() {
            let mut clean = decode(bytes);
            if index >= 3 {
                for sample in &mut clean {
                    *sample *= 0.2;
                }
            }
            let conditions: Vec<_> = if index < 3 {
                vec![None, Some(-10.0), Some(0.0), Some(10.0)]
            } else {
                vec![None]
            };
            for snr in conditions {
                let source = snr.map_or_else(|| clean.clone(), |snr| mix(&clean, snr));
                let (output, latency, timing) = render(&source, 1, model, 1.0, &[128]);
                let aligned = &output[latency..latency + source.len()];
                let (dry, dry_latency, _) =
                    render(&source, 1, model, 0.0, &[137, 1, 479, 481, 8193]);
                assert_eq!(&dry[..dry_latency], vec![0.0; dry_latency]);
                assert_eq!(&dry[dry_latency..], source);
                if snr.is_none() || snr == Some(0.0) {
                    let (partitioned, _, _) =
                        render(&source, 1, model, 1.0, &[137, 1, 479, 481, 8193]);
                    assert_eq!(partitioned, output);
                }
                assert!(
                    energy(aligned) > 1e-12,
                    "{id} model={model:?} collapsed to silence"
                );
                let onset = 4800.min(clean.len());
                println!(
                    "{}",
                    serde_json::json!({"measurement":"recorded_speech_model","model":model.label(),
                    "fixture":id,"source_kind":if index<3 {"speech"} else {"instrument"},
                    "snr_db":snr,"frames":source.len(),"latency_frames":latency,
                    "input":quality(&source,&clean),"output":quality(aligned,&clean),
                    "first_100ms":quality(&aligned[..onset],&clean[..onset]),"timing":timing,
                    "no_skipped_warmup":true,"model_training_overlap":"unknown"})
                );
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 42);
}

#[test]
fn recorded_stereo_keeps_polarity_level_ratio_and_channel_order() {
    for model in MODELS {
        for (id, bytes) in SPEECH {
            let clean = decode(bytes);
            for ratio in [1.0, -1.0, 0.25, 0.0] {
                let stereo: Vec<_> = clean.iter().flat_map(|v| [*v, *v * ratio]).collect();
                let (output, latency, _) =
                    render(&stereo, 2, model, 1.0, &[137, 1, 479, 481, 8193]);
                let error = output
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|f| (f[1] - ratio * f[0]).abs())
                    .fold(0.0_f32, f32::max);
                assert!(
                    error < 3e-6,
                    "{id} model={model:?} ratio={ratio} error={error}"
                );
                assert!(
                    output
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .skip(latency)
                        .any(|f| f[0].abs() > 1e-6)
                );
                println!(
                    "{}",
                    serde_json::json!({"measurement":"recorded_stereo_ratio","fixture":id,
                    "model":model.label(),"ratio":ratio,"maximum_error":error})
                );
            }
        }
        let left = decode(SPEECH[0].1);
        let right = decode(SPEECH[1].1);
        let source: Vec<_> = left.iter().zip(right).flat_map(|(l, r)| [*l, r]).collect();
        let swapped: Vec<_> = source
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|f| [f[1], f[0]])
            .collect();
        let (a, _, _) = render(&source, 2, model, 1.0, &[128]);
        let (b, _, _) = render(&swapped, 2, model, 1.0, &[128]);
        let error = a
            .as_chunks::<2>()
            .0
            .iter()
            .zip(b.as_chunks::<2>().0)
            .map(|(a, b)| (a[0] - b[1]).abs().max((a[1] - b[0]).abs()))
            .fold(0.0_f32, f32::max);
        assert!(error < 3e-6, "model={model:?} channel order error={error}");
        println!(
            "{}",
            serde_json::json!({"measurement":"recorded_stereo_swap","model":model.label(),"maximum_error":error})
        );
    }
}
