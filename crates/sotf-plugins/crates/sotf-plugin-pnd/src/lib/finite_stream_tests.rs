//! EOS dispatch uses the complete adaptive history and freezes its controller.

// Rust guideline compliant 2026-02-21
use super::consts::{PV_FFT_SIZE as N, PV_HOP_SIZE as H, PV_LATENCY_FRAMES as D};
use super::phase_vocoder::PhaseVocoder;
use super::{PndPlugin, PndPluginParams};
use sotf_host::{ParameterId, ParameterValue, Plugin, ProcessContext};

const RATE: u32 = 48_000;

#[derive(Debug, PartialEq)]
struct AnalyzerSnapshot {
    generation: u64,
    confidence: f32,
    matched: usize,
    peaks: usize,
    matches: Vec<(f32, f32)>,
}

#[derive(Debug, PartialEq)]
struct ControllerSnapshot {
    ratio: f64,
    drift: f64,
    generation: u64,
    transition: bool,
    strength: f32,
    current_strength: f32,
    consensus: Vec<(f32, f32)>,
    cadence: usize,
    analyzers: Vec<AnalyzerSnapshot>,
}
fn snapshot(p: &PndPlugin) -> ControllerSnapshot {
    ControllerSnapshot {
        ratio: p.current_ratio,
        drift: p.last_drift_ratio,
        generation: p.last_analysis_generation,
        transition: p.reference_transition_pending,
        strength: p.correction_strength,
        current_strength: p.correction_strength_current,
        consensus: p.channel_consensus_scratch.clone(),
        cadence: p.cache_update_counter,
        analyzers: p
            .analyzers
            .iter()
            .map(|a| AnalyzerSnapshot {
                generation: a.analysis_generation(),
                confidence: a.confidence(),
                matched: a.matched_partials(),
                peaks: a.total_peaks(),
                matches: a.current_matched_peaks().to_vec(),
            })
            .collect(),
    }
}

// This scheduler never invokes plugin drain or its fixed-path dispatch. The
// cloned vocoder includes every pre-EOF phase, onset, source and OLA history.
fn continue_zeros(vocoder: &mut PhaseVocoder, frames: usize, ratio: f32, formant: f32) -> Vec<f32> {
    let mut output = Vec::with_capacity(frames * vocoder.channels.len());
    for _ in 0..frames {
        for channel in &mut vocoder.channels {
            channel.input_buf[channel.input_fill] = 0.0;
            channel.input_fill += 1;
            if channel.input_fill == N {
                channel.process_hop_with_formant_strength(ratio, formant);
            }
            let value = if channel.output_fill == 0 {
                0.0
            } else {
                let index = channel.output_read % channel.output_accum.len();
                let value = channel.output_accum[index];
                channel.output_accum[index] = 0.0;
                channel.output_read += 1;
                channel.output_fill -= 1;
                value
            };
            output.push(value);
        }
    }
    output
}

fn learned(drift: f32, formants: bool) -> (PndPlugin, usize) {
    let mut p = PndPlugin::from_params(
        2,
        PndPluginParams {
            reference_frequency_hz: 440.0,
            drift_smoothing: 0.001,
            confidence_threshold: 0.2,
            formant_preservation: formants,
            formant_strength: 0.8,
            ..Default::default()
        },
    )
    .unwrap();
    p.initialize(RATE).unwrap();
    let frames = 24_113;
    let input: Vec<_> = (0..frames)
        .flat_map(|i| {
            let phase = std::f32::consts::TAU * 440.0 * drift * i as f32 / RATE as f32;
            let tone = 0.2 * phase.sin() + 0.1 * (2.0 * phase).sin();
            [tone, -tone * 0.7]
        })
        .collect();
    let mut offset = 0;
    for block in [257, 17, 1024].into_iter().cycle() {
        let n = block.min(frames - offset);
        if n == 0 {
            break;
        }
        p.process(
            &input[offset * 2..(offset + n) * 2],
            &mut vec![0.0; n * 2],
            &ProcessContext::new(RATE, n),
        )
        .unwrap();
        offset += n;
    }
    assert!(
        (p.current_ratio - 1.0).abs() > 0.005,
        "ratio={}",
        p.current_ratio
    );
    (p, frames)
}

#[test]
fn adaptive_history_matches_fixed_continuation_and_estimators_freeze() {
    for drift in [0.98, 1.02] {
        for formants in [false, true] {
            let (mut p, frames) = learned(drift, formants);
            p.set_parameter(
                ParameterId::from("correction_strength"),
                ParameterValue::Float(0.1),
            )
            .unwrap();
            // One real final-marker sample advances smoothing, but cannot reach
            // the new target. EOF must latch the effective current value.
            p.process(&[0.8, -0.4], &mut [0.0; 2], &ProcessContext::new(RATE, 1))
                .unwrap();
            assert!((p.correction_strength_current - p.correction_strength).abs() > 0.8);
            let q =
                (1.0 + (p.current_ratio - 1.0) * f64::from(p.correction_strength_current)) as f32;
            assert!((q - 1.0).abs() > 0.005);
            let formant = if formants { p.formant_strength } else { 0.0 };
            let before = snapshot(&p);
            let mut oracle = p.vocoder.as_ref().unwrap().clone();
            let accepted = frames + 1;
            let remaining = D + ((accepted - 1) / H) * H + N - accepted;
            let expected = continue_zeros(&mut oracle, remaining, q, formant);
            let mut output = Vec::new();
            for capacity in [1, 7, 512, 2048].into_iter().cycle() {
                let mut block = vec![123.0; capacity * 2];
                let result = p.drain(&mut block, &ProcessContext::new(RATE, 0)).unwrap();
                output.extend_from_slice(&block[..result.frames * 2]);
                assert_eq!(snapshot(&p), before);
                if result.complete {
                    break;
                }
            }
            assert_eq!(output, expected, "drift={drift}, formants={formants}");
            assert!(output.iter().any(|sample| sample.abs() > 0.01));
            assert!(
                continue_zeros(&mut oracle, N * 2, q, formant)
                    .iter()
                    .all(|v| *v == 0.0)
            );
        }
    }
}

#[test]
fn zero_windows_cannot_synthesize_energy_from_old_phase_or_envelope_memory() {
    let (p, _) = learned(1.02, true);
    for q in [0.95, 1.0, 1.0 / 0.95] {
        for formant in [0.0, 1.0] {
            let mut oracle = p.vocoder.as_ref().unwrap().clone();
            assert!(
                oracle
                    .channels
                    .iter()
                    .any(|ch| ch.prev_magnitude.iter().any(|x| *x > 0.0))
            );
            continue_zeros(&mut oracle, D + N, q, formant);
            assert!(
                continue_zeros(&mut oracle, N * 2, q, formant)
                    .iter()
                    .all(|v| *v == 0.0)
            );
        }
    }
}

#[test]
fn failed_reinitialize_preserves_learned_history_and_success_restarts_it() {
    let (mut p, _) = learned(0.98, true);
    let before = snapshot(&p);
    let mut oracle = p.vocoder.as_ref().unwrap().clone();
    assert!(p.initialize(800).is_err()); // configured 440 Hz reference is above Nyquist
    assert_eq!(snapshot(&p), before);
    let q = (1.0 + (p.current_ratio - 1.0) * f64::from(p.correction_strength_current)) as f32;
    let expected = continue_zeros(&mut oracle, 7, q, p.formant_strength);
    let mut output = [0.0; 14];
    p.drain(&mut output, &ProcessContext::new(RATE, 0)).unwrap();
    assert_eq!(output.as_slice(), expected);
    p.initialize(RATE).unwrap();
    assert_eq!(p.current_ratio, 1.0);
    assert_eq!(p.last_drift_ratio, 1.0);
    assert_eq!(p.correction_strength_current, p.correction_strength);
    assert_eq!(p.last_analysis_generation, 0);
    assert!(p.analyzers.iter().all(|a| a.analysis_generation() == 0));
}
