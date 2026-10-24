//! Recorded model checks around anti-correlated channel-energy crossings.

// Rust guideline compliant 2026-02-21
use plugins_denoiser::rnnoise::{RnnoiseBackend, RnnoiseModelId};

const FRAME: usize = 480;
const LATENCY: usize = 960;
const SOURCES: [(&str, &[u8]); 3] = [
    (
        "1272",
        include_bytes!("../../sotf-plugin-speech-denoiser/tests/fixtures/recorded/speech-0.pcm"),
    ),
    (
        "1462",
        include_bytes!("../../sotf-plugin-speech-denoiser/tests/fixtures/recorded/speech-1.pcm"),
    ),
    (
        "174",
        include_bytes!("../../sotf-plugin-speech-denoiser/tests/fixtures/recorded/speech-2.pcm"),
    ),
];

fn model(id: RnnoiseModelId) -> Box<nnnoiseless::DenoiseState> {
    match id.embedded_bytes() {
        None => nnnoiseless::DenoiseState::new(),
        Some(bytes) => {
            nnnoiseless::DenoiseState::from_model(nnnoiseless::parse_rnnn_model(bytes).unwrap())
                .unwrap()
        }
    }
}

// Frozen pre-change detector, expressed on interleaved input independently of
// the production helper and its temporal-reference state. Preserve its original
// per-frame dominant-energy / canonical-peak selection as the quality control.
fn legacy_detector(input: &[f32], detector: &mut [f32]) {
    let mut left_energy = 0.0_f32;
    let mut right_energy = 0.0_f32;
    let mut cross = 0.0_f32;
    for frame in input.as_chunks::<2>().0 {
        left_energy += frame[0] * frame[0];
        right_energy += frame[1] * frame[1];
        cross += frame[0] * frame[1];
    }
    let polarity = if cross < 0.0 { -1.0 } else { 1.0 };
    let mut energy = 0.0_f32;
    let mut peak = 0.0_f32;
    for frame in input.as_chunks::<2>().0 {
        let value = frame[0] + polarity * frame[1];
        energy += value * value;
        if value.abs() > peak.abs() {
            peak = value;
        }
    }
    let sign = if polarity < 0.0
        && (right_energy > left_energy || (right_energy == left_energy && peak < 0.0))
    {
        -1.0
    } else {
        1.0
    };
    let scale = if energy > 1e-20 {
        (0.5 * (left_energy + right_energy) / energy).sqrt()
    } else {
        0.0
    };
    for (out, frame) in detector.iter_mut().zip(input.as_chunks::<2>().0) {
        *out = ((frame[0] + polarity * frame[1]) * (scale * sign)).clamp(-1.0, 1.0) * 32768.0;
    }
}

fn render_legacy_left(input: &[f32], id: RnnoiseModelId) -> Vec<f32> {
    let mut detector = model(id);
    let mut channel = model(id);
    let mut detector_input = [0.0; FRAME];
    let mut channel_input = [0.0; FRAME];
    let mut output = [0.0; FRAME];
    let mut result = vec![0.0; FRAME];
    for block in input.as_chunks::<{ FRAME * 2 }>().0 {
        legacy_detector(block, &mut detector_input);
        let analysis = detector.process_frame_with_analysis(&mut output, &detector_input);
        for (sample, frame) in channel_input.iter_mut().zip(block.as_chunks::<2>().0) {
            *sample = frame[0] * 32768.0;
        }
        channel.process_frame_with_band_gains(&mut output, &channel_input, &analysis.band_gains);
        result.extend(output.iter().map(|sample| sample / 32768.0));
    }
    result.truncate(input.len() / 2);
    result
}

fn render_candidate_left(input: &[f32], id: RnnoiseModelId) -> Vec<f32> {
    let mut backend = RnnoiseBackend::new();
    backend.initialize_with_model(48_000, 2, id).unwrap();
    let mut output = input.to_vec();
    for block in output.chunks_mut(257 * 2) {
        assert_eq!(
            backend.process(block, block.len() / 2, 2, false),
            block.len() / 2
        );
    }
    output
        .as_chunks::<2>()
        .0
        .iter()
        .map(|frame| frame[0])
        .collect()
}

fn quality(output: &[f32], clean: &[f32]) -> (f64, f64, f64) {
    let energy: f64 = clean.iter().map(|&x| f64::from(x).powi(2)).sum();
    let dot: f64 = output
        .iter()
        .zip(clean)
        .map(|(&a, &b)| f64::from(a) * f64::from(b))
        .sum();
    let projection = dot / energy;
    let error: f64 = output
        .iter()
        .zip(clean)
        .map(|(&a, &b)| (f64::from(a) - f64::from(b)).powi(2))
        .sum();
    let residual: f64 = output
        .iter()
        .zip(clean)
        .map(|(&a, &b)| (f64::from(a) - projection * f64::from(b)).powi(2))
        .sum();
    (
        10.0 * (energy / error.max(1e-30)).log10(),
        10.0 * (projection.powi(2) * energy / residual.max(1e-30)).log10(),
        projection,
    )
}

#[test]
fn temporal_reference_retains_recorded_model_quality_across_energy_crossings() {
    let mut cases = 0;
    let mut failures = 0;
    for (speaker, bytes) in SOURCES {
        let clean: Vec<f32> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&s| f32::from(i16::from_le_bytes(s)) / 32768.0)
            .collect();
        assert_eq!(clean.len() % FRAME, 0);
        for id in [
            RnnoiseModelId::BundledFull,
            RnnoiseModelId::LegacyLq,
            RnnoiseModelId::LegacySh,
        ] {
            for regime in [
                "positive_control",
                "negative_constant_control",
                "slow_energy_crossing",
                "near_tie_flutter",
            ] {
                let mut input = vec![0.0; (clean.len() + LATENCY) * 2];
                for (frame, &sample) in clean.iter().enumerate() {
                    let ratio = match regime {
                        "positive_control" => 1.0,
                        "negative_constant_control" => -0.9,
                        "slow_energy_crossing" => -(0.8 + 0.4 * frame as f32 / clean.len() as f32),
                        _ => {
                            if (frame / FRAME / 7).is_multiple_of(2) {
                                -0.999
                            } else {
                                -1.001
                            }
                        }
                    };
                    input[frame * 2] = sample;
                    input[frame * 2 + 1] = sample * ratio;
                }
                let candidate = render_candidate_left(&input, id);
                let legacy = render_legacy_left(&input, id);
                if regime.ends_with("control") {
                    assert_eq!(
                        candidate, legacy,
                        "unchanged control {speaker}/{id:?}/{regime}"
                    );
                }
                let candidate = quality(&candidate[LATENCY..LATENCY + clean.len()], &clean);
                let legacy = quality(&legacy[LATENCY..LATENCY + clean.len()], &clean);
                let ordinary_delta = candidate.0 - legacy.0;
                let si_delta = candidate.1 - legacy.1;
                failures += usize::from(ordinary_delta < -0.25 || si_delta < -0.25);
                println!(
                    "TEMPORAL {{\"speaker\":\"{speaker}\",\"model\":\"{}\",\"regime\":\"{regime}\",\"ordinary_sdr_delta_db\":{ordinary_delta},\"si_sdr_delta_db\":{si_delta},\"legacy_projection\":{},\"candidate_projection\":{}}}",
                    id.label(),
                    legacy.2,
                    candidate.2
                );
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 36);
    assert_eq!(
        failures, 0,
        "retain the predeclared 0.25 dB quality regression guard"
    );
}
