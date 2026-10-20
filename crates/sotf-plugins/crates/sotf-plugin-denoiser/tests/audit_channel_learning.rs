//! Independent-channel noise learning must not depend on channel traversal order.

// Rust guideline compliant 2026-02-21
use sotf_host::ProcessContext;
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_plugin_denoiser::{DenoiserPlugin, DenoiserPluginParams};

fn processor(channels: usize, low_latency: bool, rate: u32) -> DenoiserPlugin {
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
    p.initialize(f64::from(rate)).unwrap();
    p
}

fn render(
    p: &mut DenoiserPlugin,
    source: &[f32],
    channels: usize,
    rate: u32,
    block: usize,
) -> Vec<f32> {
    let mut output = source.to_vec();
    for chunk in output.chunks_mut(block * channels) {
        let frames = chunk.len() / channels;
        p.process_in_place(chunk, &ProcessContext::new(rate, frames))
            .unwrap();
    }
    let mut tail = vec![0.0; p.drain_output_frames_max() * channels];
    for _ in 0..16 {
        let step = p.drain(&mut tail, &ProcessContext::new(rate, 0)).unwrap();
        output.extend_from_slice(&tail[..step.frames * channels]);
        if step.complete {
            return output;
        }
    }
    panic!("denoiser drain failed to finish")
}

fn source(rate: u32, recorded: bool) -> Vec<f32> {
    let bytes = include_bytes!("../../sotf-plugin-de-esser/tests/fixtures/recorded/s.pcm");
    let mut state = 0x12345678_u32;
    let mut noise = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state as f64 / f64::from(u32::MAX) * 2.0 - 1.0
    };
    (0..rate as usize)
        .flat_map(|frame| {
            // The recorded condition starts in the manifest's /s/ region, without
            // a noise-only lead-in. It is one attributed speaker, not a corpus.
            let speech = if recorded && frame + 12000 < bytes.len() / 2 {
                let i = (frame + 12000) * 2;
                f32::from(i16::from_le_bytes([bytes[i], bytes[i + 1]])) / 32768.0 * 0.2
            } else {
                0.0
            };
            let amplitude = if frame > rate as usize / 4 && frame < rate as usize * 3 / 4 {
                0.1
            } else {
                0.001
            };
            [
                speech + noise() as f32 * amplitude,
                noise() as f32 * (0.002 + 0.003 * frame as f32 / rate as f32),
            ]
        })
        .collect()
}

#[test]
fn independent_noise_learning_is_channel_order_invariant() {
    for rate in [44_100, 48_000, 96_000] {
        for low_latency in [false, true] {
            for recorded in [false, true] {
                if recorded && rate != 48_000 {
                    continue;
                }
                let input = source(rate, recorded);
                let swapped: Vec<f32> = input
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .flat_map(|x| [x[1], x[0]])
                    .collect();
                let mut a = processor(2, low_latency, rate);
                let mut b = processor(2, low_latency, rate);
                let out = render(&mut a, &input, 2, rate, 257);
                let reversed = render(&mut b, &swapped, 2, rate, 257);
                assert_eq!(out.len(), reversed.len());
                let mut error = 0.0_f32;
                for (a, b) in out
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .zip(reversed.as_chunks::<2>().0.iter())
                {
                    error = error.max((a[0] - b[1]).abs()).max((a[1] - b[0]).abs());
                }
                println!(
                    "channel_swap rate={rate} low_latency={low_latency} recorded={recorded} maximum_error={error:e}"
                );
                assert_eq!(
                    error, 0.0,
                    "noise learning changed under a pure channel permutation"
                );
                for channel in 0..2 {
                    let mono: Vec<_> = input
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|x| x[channel])
                        .collect();
                    let mut p = processor(1, low_latency, rate);
                    let expected = render(&mut p, &mono, 1, rate, 73);
                    assert_eq!(out.len(), expected.len() * 2);
                    assert!(
                        out.as_chunks::<2>()
                            .0
                            .iter()
                            .zip(expected)
                            .all(|(a, b)| a[channel] == b),
                        "stereo differs from independent mono channel {channel}"
                    );
                }
            }
        }
    }
}

#[test]
fn recorded_speech_at_start_reports_aligned_quality_and_noise_tracking() {
    let rate = 48_000;
    let input = source(rate, true);
    let bytes = include_bytes!("../../sotf-plugin-de-esser/tests/fixtures/recorded/s.pcm");
    let clean: Vec<f64> = (0..rate as usize)
        .map(|frame| {
            let i = (frame + 12000) * 2;
            f64::from(i16::from_le_bytes([bytes[i], bytes[i + 1]])) / 32768.0 * 0.2
        })
        .collect();
    for low_latency in [false, true] {
        let mut p = processor(2, low_latency, rate);
        let latency = p.latency_samples();
        let output = render(&mut p, &input, 2, rate, 257);
        for partition in [1, 4096] {
            p.reset();
            assert_eq!(render(&mut p, &input, 2, rate, partition), output);
        }
        let aligned = &output[latency * 2..(latency + rate as usize) * 2];
        let energy = clean.iter().map(|v| v * v).sum::<f64>();
        let error = |source: &[f32]| {
            source
                .as_chunks::<2>()
                .0
                .iter()
                .zip(&clean)
                .map(|(frame, clean)| (f64::from(frame[0]) - clean).powi(2))
                .sum::<f64>()
        };
        let input_sdr = 10.0 * (energy / error(&input)).log10();
        let output_sdr = 10.0 * (energy / error(aligned)).log10();
        let onset = rate as usize / 10;
        let onset_energy = clean[..onset].iter().map(|v| v * v).sum::<f64>();
        let projection = aligned
            .as_chunks::<2>()
            .0
            .iter()
            .take(onset)
            .zip(&clean)
            .map(|(frame, clean)| f64::from(frame[0]) * clean)
            .sum::<f64>()
            / onset_energy;
        assert!(input_sdr.is_finite() && output_sdr.is_finite() && projection.is_finite());
        assert!(onset_energy > 1e-6);
        println!(
            "{}",
            serde_json::json!({"measurement":"recorded_denoiser_onset",
            "rate":rate,"low_latency":low_latency,"latency_frames":latency,
            "input_sdr_db":input_sdr,"output_sdr_db":output_sdr,
            "first_100ms_clean_projection":projection,
            "note":"one recorded speaker with deterministic added noise; no population or perceptual quality gate"})
        );
        for (start, end) in [(4800, 9600), (14400, 21600), (40800, 45600)] {
            let energy = |source: &[f32]| {
                source
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .skip(start)
                    .take(end - start)
                    .map(|frame| f64::from(frame[1]).powi(2))
                    .sum::<f64>()
            };
            let gain_db = 10.0 * (energy(aligned) / energy(&input)).log10();
            assert!(gain_db.is_finite());
            println!(
                "{}",
                serde_json::json!({"measurement":"independent_noise_channel",
                "low_latency":low_latency,"start_frame":start,"end_frame":end,"gain_db":gain_db})
            );
        }
    }
}
