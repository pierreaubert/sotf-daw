//! Complete-response linear convolution references, including non-grid designs.

use crate::{BinauralDecoderPlugin, RoomModel};
use realfft::RealFftPlanner;
use rustfft::num_complex::Complex;
use sofa_reader::SofaWriter;
use sotf_host::sofa::{SofaFile, SourcePosition};
use sotf_host::{Plugin, ProcessContext};
use std::path::PathBuf;

struct Fixture {
    directory: PathBuf,
    path: PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
fn fixture(rate: u32, length: usize) -> (Fixture, SofaFile) {
    let directory = std::env::var_os("SOTF_AUDIT_TMP")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!(
            "binaural-support-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("hrtf.sofa");
    let positions = vec![
        SourcePosition::new(15.0, -10.0, 1.0),
        SourcePosition::new(75.0, -10.0, 1.0),
        SourcePosition::new(45.0, 30.0, 1.0),
    ];
    let mut ir = vec![0.0; 3 * 2 * length];
    for m in 0..3 {
        for ear in 0..2 {
            ir[(m * 2 + ear) * length + 2 + m + ear] = 0.6 + ear as f32 * 0.1;
            ir[(m * 2 + ear) * length + length - 1] = -0.04 * (m + 1) as f32;
        }
    }
    let mut w = SofaWriter::new();
    w.add_attribute_str("Conventions", "SOFA");
    w.add_attribute_str("SOFAConventions", "SimpleFreeFieldHRIR");
    for (name, size) in [("I", 1), ("M", 3), ("R", 2), ("C", 3), ("N", length)] {
        w.add_dimension(name, size);
    }
    w.add_variable_f64("Data.SamplingRate", &["I"]);
    w.write_f64("Data.SamplingRate", &[f64::from(rate)])
        .unwrap();
    w.add_variable_f64("SourcePosition", &["M", "C"]);
    w.write_f64(
        "SourcePosition",
        &[15.0, -10.0, 1.0, 75.0, -10.0, 1.0, 45.0, 30.0, 1.0],
    )
    .unwrap();
    w.add_variable_f64("Data.IR", &["M", "R", "N"]);
    w.write_f64(
        "Data.IR",
        &ir.iter().map(|x| f64::from(*x)).collect::<Vec<_>>(),
    )
    .unwrap();
    w.finish(&path).unwrap();
    (
        Fixture { directory, path },
        SofaFile {
            sample_rate: f64::from(rate),
            num_measurements: 3,
            ir_length: length,
            positions,
            impulse_responses: ir,
            convention: "SimpleFreeFieldHRIR".into(),
            data_sample_rate: Some(f64::from(rate)),
        },
    )
}

fn real_idft(spectrum: &[Complex<f32>], n: usize) -> Vec<f64> {
    (0..n)
        .map(|t| {
            let mut sum = f64::from(spectrum[0].re)
                + f64::from(spectrum[n / 2].re) * if t.is_multiple_of(2) { 1.0 } else { -1.0 };
            for (k, h) in spectrum.iter().enumerate().take(n / 2).skip(1) {
                let phase = 2.0 * std::f64::consts::PI * k as f64 * t as f64 / n as f64;
                sum += 2.0 * (f64::from(h.re) * phase.cos() - f64::from(h.im) * phase.sin());
            }
            sum / n as f64
        })
        .collect()
}

#[test]
fn non_grid_native_sofa_matches_complete_causal_fir_across_callbacks_and_eof() {
    for rate in [44100, 48000, 96000] {
        for n in [64, 256, 1024] {
            for length in [8, n] {
                let (file, sofa) = fixture(rate, length);
                for eq in [false, true] {
                    for near in [0.0, 0.75] {
                        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(n);
                        let target = SourcePosition::new(45.0, 0.0, 1.0);
                        let nearest = sofa.find_three_nearest(&target);
                        let weights = crate::hrtf::calculate_vbap_gains(&target, &nearest, &sofa);
                        assert!(
                            weights.iter().filter(|g| **g > 0.05).count() > 1,
                            "fixture must interpolate"
                        );
                        let (left, right) = crate::hrtf::interpolate_hrtf_frequency_domain(
                            &nearest,
                            &weights,
                            &sofa,
                            n,
                            f64::from(rate),
                            &fft,
                            near,
                            45.0,
                            0.0,
                        );
                        let maximum = left
                            .iter()
                            .chain(&right)
                            .map(|h| h.norm())
                            .fold(0.0_f32, f32::max);
                        let gain = if maximum > 0.95 { 0.95 / maximum } else { 1.0 };
                        let equalizer = eq.then(|| {
                            crate::filter::compute_diffuse_field_eq(&sofa, n, f64::from(rate), &fft)
                                .unwrap()
                        });
                        let impulse: [Vec<f64>; 2] = std::array::from_fn(|ear| {
                            let mut h = if ear == 0 {
                                left.clone()
                            } else {
                                right.clone()
                            };
                            for (k, v) in h.iter_mut().enumerate() {
                                *v *= gain;
                                if let Some(e) = &equalizer {
                                    *v *= e[ear][k];
                                }
                            }
                            real_idft(&h, n)
                        });
                        for shape in 0..3 {
                            let mut p = BinauralDecoderPlugin::new(
                                1,
                                n,
                                Some(file.path.clone()),
                                1.0,
                                near,
                                eq,
                                120.0,
                                2.0,
                                0.0,
                                RoomModel {
                                    max_order: 0,
                                    ..Default::default()
                                },
                            );
                            p.smoothing.head_yaw_deg.reset(-45.0);
                            p.initialize(f64::from(rate)).unwrap();
                            // Publication scheduling is covered separately; hold this exact prepared transfer.
                            p.hrtf_update_tx.take();
                            p.hrtf_update_thread.take().unwrap().join().unwrap();
                            assert_eq!(p.latency_samples(), n);
                            if shape == 0 {
                                let loaded = p.state.load_full();
                                let prepared = crate::hrtf::prepare_hrtf_spectra(
                                    &sofa,
                                    n,
                                    f64::from(rate),
                                    &fft,
                                );
                                let rotated =
                                    BinauralDecoderPlugin::compute_head_rotated_hrtf_state(
                                        &loaded,
                                        &p.config,
                                        &prepared,
                                        &p.fft,
                                        &[],
                                        (-45.0, 0.0, 0.0),
                                    )
                                    .unwrap();
                                for (actual, expected) in rotated
                                    .hrtf_filters_freq
                                    .iter()
                                    .flatten()
                                    .zip(loaded.hrtf_filters_freq.iter().flatten())
                                {
                                    assert!(
                                        (*actual - *expected).norm() < 2.0e-5,
                                        "worker/load transfer mismatch"
                                    );
                                }
                                p.set_parameter(
                                    "hrtf_file".into(),
                                    sotf_host::ParameterValue::String(
                                        file.path.to_string_lossy().into_owned(),
                                    ),
                                )
                                .unwrap();
                                let reloaded = p.state.load_full();
                                for (actual, expected) in reloaded
                                    .hrtf_filters_freq
                                    .iter()
                                    .flatten()
                                    .zip(loaded.hrtf_filters_freq.iter().flatten())
                                {
                                    assert!(
                                        (*actual - *expected).norm() < 2.0e-5,
                                        "replacement transfer mismatch"
                                    );
                                }
                                p.hrtf_update_tx.take();
                                p.hrtf_update_thread.take().unwrap().join().unwrap();
                            }
                            let hop = n / 4;
                            let mut input = vec![0.0; hop * 23 + 3];
                            for index in [hop - 1, hop * 3 - 1, hop * 20 - 1, hop * 23 + 2] {
                                input[index] = 0.25;
                            }
                            let mut reference = vec![0.0; (input.len() + n + n) * 2];
                            for (i, sample) in input.iter().enumerate() {
                                for (ear, kernel) in impulse.iter().enumerate() {
                                    for (tap, h) in kernel.iter().enumerate() {
                                        reference[(i + n + tap) * 2 + ear] +=
                                            f64::from(*sample) * h;
                                    }
                                }
                            }
                            let mut actual = Vec::new();
                            let mut offset = 0;
                            let mut part = 0;
                            while offset < input.len() {
                                let frames = match shape {
                                    0 => input.len(),
                                    1 => 1,
                                    _ => [3, 17, 5, hop + 1][part % 4],
                                }
                                .min(input.len() - offset);
                                let mut out = vec![0.0; frames * 2];
                                p.process(
                                    &input[offset..offset + frames],
                                    &mut out,
                                    &ProcessContext::new(rate, frames),
                                )
                                .unwrap();
                                actual.extend(out);
                                offset += frames;
                                part += 1;
                            }
                            loop {
                                let frames = match shape {
                                    0 => hop,
                                    1 => 1,
                                    _ => 7,
                                };
                                let mut out = vec![0.0; frames * 2];
                                let result = p
                                    .drain(&mut out, &ProcessContext::new(rate, frames))
                                    .unwrap();
                                actual.extend_from_slice(&out[..result.frames * 2]);
                                if result.complete {
                                    break;
                                }
                            }
                            assert!(
                                actual[..(n + hop - 1) * 2].iter().all(|x| *x == 0.0),
                                "output precedes the first source impulse"
                            );
                            let mut error = 0.0_f64;
                            for (i, x) in actual.iter().enumerate() {
                                error = error.max(
                                    (f64::from(*x) - reference.get(i).copied().unwrap_or(0.0))
                                        .abs(),
                                );
                            }
                            for x in reference.iter().skip(actual.len()) {
                                error = error.max(x.abs());
                            }
                            println!(
                                "LINEAR_SUPPORT rate={rate} design={n} ir={length} eq={eq} near={near} shape={shape} max_error={error}"
                            );
                            assert!(
                                error < 2.0e-5,
                                "complete causal response {rate}/{n}/{eq}/{near}/{shape}: {error}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn spectral_frame_tail_survives_switch_to_linear_before_eof() {
    use crate::types::BinauralState;
    use std::sync::Arc;
    for frames in [16, 19, 32, 35] {
        let make = || {
            let mut p = BinauralDecoderPlugin::new(
                1,
                64,
                None,
                1.0,
                0.0,
                false,
                120.0,
                2.0,
                0.0,
                RoomModel {
                    max_order: 0,
                    ..Default::default()
                },
            );
            p.config.crossfade_mode_index = 1;
            p.initialize(48000.0).unwrap();
            let bins = p.config.freq_size;
            let mut filters = vec![Complex::new(0.0, 0.0); bins * 2];
            for ear in 0..2 {
                for k in 0..bins {
                    let w = -2.0 * std::f32::consts::PI * k as f32 / 128.0;
                    filters[ear * bins + k] =
                        Complex::new(0.6, 0.0) + Complex::from_polar(0.25, w * 2.0);
                }
            }
            p.state.store(Arc::new(BinauralState {
                hrtf_filters_freq: vec![filters],
                design_diffuse_field_eq: None,
                _hrtf_data: None,
            }));
            p
        };
        let source: Vec<f32> = (0..frames)
            .map(|i| ((i * 37 % 19) as f32 - 9.0) * 0.015)
            .collect();
        let mut tested = make();
        let mut reference = make();
        let mut actual = vec![0.0; frames * 2];
        let mut expected = vec![0.0; frames * 2];
        let context = ProcessContext::new(48000, frames);
        tested.process(&source, &mut actual, &context).unwrap();
        reference.process(&source, &mut expected, &context).unwrap();
        assert_eq!(actual, expected);
        for p in [&mut tested, &mut reference] {
            p.set_parameter("crossfade_mode".into(), sotf_host::ParameterValue::Int(0))
                .unwrap();
        }
        let mut continuation = vec![0.0; 256 * 2];
        reference
            .process(
                &vec![0.0; 256],
                &mut continuation,
                &ProcessContext::new(48000, 256),
            )
            .unwrap();
        expected.extend(continuation);
        loop {
            let mut output = [0.0; 14];
            let result = tested
                .drain(&mut output, &ProcessContext::new(48000, 7))
                .unwrap();
            actual.extend_from_slice(&output[..result.frames * 2]);
            if result.complete {
                break;
            }
        }
        assert!(
            expected[(frames + 127) * 2..]
                .iter()
                .any(|x| x.abs() > 1.0e-5),
            "fixture must exercise an existing spectral frame beyond FIR support"
        );
        for (i, value) in expected.iter().enumerate() {
            assert!(
                (actual.get(i).copied().unwrap_or(0.0) - value).abs() < 2.0e-5,
                "spectral EOF {frames}/{i}"
            );
        }
    }
}
