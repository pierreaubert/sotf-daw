//! Shared control, stream, and realtime contracts across all six color models.

// Rust guideline compliant 2026-02-21
use sotf_host::CountingAlloc;
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_plugin_analog_common::{AnalogColorStage, MODEL_NAMES, model_param_spec};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

fn stage(id: u32, rate: u32, channels: usize, maximum: usize) -> AnalogColorStage {
    let mut p = AnalogColorStage::new(channels);
    p.set_model_id(id).unwrap();
    p.set_drive_db(6.0).unwrap();
    p.set_character(0.31).unwrap();
    p.set_color(0.73).unwrap();
    p.set_output_trim_db(-3.0).unwrap();
    p.prepare(rate, maximum).unwrap();
    p
}

fn signal(frames: usize, channels: usize) -> Vec<f32> {
    (0..frames * channels)
        .map(|i| {
            let t = (i / channels) as f32;
            0.2 * (t * (0.031 + (i % channels) as f32 * 0.007)).sin() + 0.03
        })
        .collect()
}

fn maximum_error(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    assert!(a.iter().chain(b).all(|x| x.is_finite()));
    a.iter()
        .zip(b)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max)
}

#[test]
fn unknown_schema_default_uses_documented_harmonics_fallback() {
    for id in 0..MODEL_NAMES.len() as u32 {
        assert_eq!(model_param_spec(id, "Analog").default_usize(), id as usize);
    }
    for id in [MODEL_NAMES.len() as u32, u32::MAX] {
        assert_eq!(model_param_spec(id, "Analog").default_usize(), 0);
    }
}

#[test]
fn rejected_preparation_preserves_layout_for_later_model_selection() {
    for id in 0..MODEL_NAMES.len() as u32 {
        let mut actual = stage(id, 48_000, 2, 127);
        let mut expected = stage(id, 48_000, 2, 127);
        for invalid in [f64::MIN_POSITIVE, 0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(actual.prepare(invalid, 1).is_err());
        }
        let next = (id + 1) % MODEL_NAMES.len() as u32;
        actual
            .set_model_id(next)
            .expect("rejected prepare must preserve usable layout");
        expected.set_model_id(next).unwrap();
        let mut a = signal(1025, 2);
        let mut b = a.clone();
        actual.process_interleaved(&mut a, 1025).unwrap();
        expected.process_interleaved(&mut b, 1025).unwrap();
        assert_eq!(maximum_error(&a, &b), 0.0, "model {id}");
    }
}

#[test]
fn cold_and_reset_zero_color_preserve_nominal_audio_despite_wet_controls() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for channels in [1, 2, 8] {
            for id in 0..MODEL_NAMES.len() as u32 {
                let mut p = stage(id, rate, channels, 127);
                let input = signal(1025, channels);
                for (drive, character, trim) in [(-60.0, 0.0, -60.0), (36.0, 1.0, 24.0)] {
                    p.set_drive_db(drive).unwrap();
                    p.set_character(character).unwrap();
                    p.set_output_trim_db(trim).unwrap();
                    p.set_color(0.0).unwrap();
                    p.reset();
                    let mut actual = input.clone();
                    assert_no_allocs_or_deallocs("zero color", || {
                        p.process_interleaved(&mut actual, 1025).unwrap();
                    });
                    assert_eq!(
                        maximum_error(&actual, &input),
                        0.0,
                        "{id}/{rate}/{channels}"
                    );
                    p.set_color(1.0).unwrap();
                    p.reset();
                    p.process_interleaved(&mut actual, 1025).unwrap();
                }
            }
        }
    }
}

#[test]
fn chunking_live_controls_and_reset_match_individual_frames_without_heap_activity() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for channels in [1, 2, 8] {
            for id in 0..MODEL_NAMES.len() as u32 {
                let mut actual = stage(id, rate, channels, 127);
                let mut expected = stage(id, rate, channels, 1);
                for epoch in 0..3 {
                    let mut a = signal(1025, channels);
                    let mut b = a.clone();
                    assert_no_allocs_or_deallocs("valid controls and oversized processing", || {
                        for p in [&mut actual, &mut expected] {
                            p.set_drive_db([-12.0, 24.0, 0.0][epoch]).unwrap();
                            p.set_color([0.0, 1.0, 0.37][epoch]).unwrap();
                            p.set_character([0.0, 1.0, 0.5][epoch]).unwrap();
                            p.set_output_trim_db([-12.0, 6.0, 0.0][epoch]).unwrap();
                        }
                        actual.process_interleaved(&mut a, 1025).unwrap();
                        for frame in b.chunks_exact_mut(channels) {
                            expected.process_interleaved(frame, 1).unwrap();
                        }
                    });
                    assert_eq!(maximum_error(&a, &b), 0.0, "{id}/{rate}/{channels}/{epoch}");
                }
                let mut fresh = stage(id, rate, channels, 512);
                fresh.set_drive_db(0.0).unwrap();
                fresh.set_color(0.37).unwrap();
                fresh.set_character(0.5).unwrap();
                fresh.set_output_trim_db(0.0).unwrap();
                fresh.reset();
                let mut a = signal(2049, channels);
                let mut b = a.clone();
                assert_no_allocs_or_deallocs("populated reset", || actual.reset());
                actual.process_interleaved(&mut a, 2049).unwrap();
                fresh.process_interleaved(&mut b, 2049).unwrap();
                assert_eq!(maximum_error(&a, &b), 0.0, "reset {id}/{rate}/{channels}");
                assert_eq!(actual.latency_samples(), 0);
            }
        }
    }
}

#[test]
fn rejected_controls_and_geometry_preserve_audio_and_running_state() {
    for id in 0..MODEL_NAMES.len() as u32 {
        let mut actual = stage(id, 48_000, 2, 127);
        let mut expected = stage(id, 48_000, 2, 127);
        let mut a = signal(257, 2);
        let mut b = a.clone();
        actual.process_interleaved(&mut a, 257).unwrap();
        expected.process_interleaved(&mut b, 257).unwrap();
        for invalid in [f32::NAN, f32::NEG_INFINITY, f32::INFINITY, -100.0, 100.0] {
            assert!(actual.set_drive_db(invalid).is_err());
            assert!(actual.set_color(invalid).is_err());
            assert!(actual.set_character(invalid).is_err());
            assert!(actual.set_output_trim_db(invalid).is_err());
        }
        assert!(actual.prepare(96_000, 0).is_err());
        let sentinel = a.clone();
        assert!(actual.process_interleaved(&mut a, 258).is_err());
        assert_eq!(maximum_error(&a, &sentinel), 0.0);
        assert!(actual.process_interleaved(&mut a, usize::MAX).is_err());
        assert_eq!(maximum_error(&a, &sentinel), 0.0);
        actual.process_interleaved(&mut a, 257).unwrap();
        expected.process_interleaved(&mut b, 257).unwrap();
        assert_eq!(maximum_error(&a, &b), 0.0, "model {id}");
    }
}

#[test]
fn nonfinite_and_extreme_input_match_independently_sanitized_input() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for id in 0..MODEL_NAMES.len() as u32 {
            let mut actual = stage(id, rate, 2, 127);
            let mut expected = stage(id, rate, 2, 127);
            let mut a = signal(2048, 2);
            a[..8].copy_from_slice(&[
                f32::NAN,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::MAX,
                -f32::MAX,
                16.0,
                -16.0,
                0.0,
            ]);
            let mut b: Vec<_> = a
                .iter()
                .map(|x| {
                    if x.is_finite() {
                        x.clamp(-16.0, 16.0)
                    } else {
                        0.0
                    }
                })
                .collect();
            assert_no_allocs_or_deallocs("sanitized processing", || {
                actual.process_interleaved(&mut a, 2048).unwrap();
            });
            expected.process_interleaved(&mut b, 2048).unwrap();
            assert_eq!(maximum_error(&a, &b), 0.0, "{id}/{rate}");
            assert!(a.iter().all(|x| x.abs() <= 128.0));
            a.copy_from_slice(&signal(2048, 2));
            b.copy_from_slice(&a);
            actual.process_interleaved(&mut a, 2048).unwrap();
            expected.process_interleaved(&mut b, 2048).unwrap();
            assert_eq!(maximum_error(&a, &b), 0.0, "recovery {id}/{rate}");
        }
    }
}

#[test]
fn decibel_trim_scales_wet_contribution_before_color_blend() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for id in 0..MODEL_NAMES.len() as u32 {
            for color in [0.0, 0.37, 1.0] {
                let input = signal(4096, 2);
                let mut unity = stage(id, rate, 2, 127);
                unity.set_color(1.0).unwrap();
                unity.set_output_trim_db(0.0).unwrap();
                unity.reset();
                let mut wet = input.clone();
                unity.process_interleaved(&mut wet, 4096).unwrap();
                for trim in [-60.0, -6.0, 6.0, 24.0] {
                    let mut actual = stage(id, rate, 2, 127);
                    actual.set_color(color).unwrap();
                    actual.set_output_trim_db(trim).unwrap();
                    actual.reset();
                    let mut a = input.clone();
                    actual.process_interleaved(&mut a, 4096).unwrap();
                    let gain = 10.0_f64.powf(f64::from(trim) / 20.0);
                    let expected: Vec<_> = input
                        .iter()
                        .zip(&wet)
                        .map(|(&dry, &w)| {
                            (f64::from(dry)
                                + f64::from(color) * (gain * f64::from(w) - f64::from(dry)))
                                as f32
                        })
                        .collect();
                    assert!(
                        maximum_error(&a, &expected) < 1e-5,
                        "{id}/{rate}/{color}/{trim}"
                    );
                }
            }
        }
    }
}

#[test]
fn drive_decibels_equal_the_same_gain_applied_to_input_at_full_color() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for id in 0..MODEL_NAMES.len() as u32 {
            // Binary gains commute exactly with the Console's f32 input
            // highpass; arbitrary gains reorder rounding in recursive state.
            // The independent dB conversion still verifies amplitude units.
            for drive in [-6.020_6, 6.020_6] {
                let mut actual = stage(id, rate, 2, 127);
                let mut reference = stage(id, rate, 2, 512);
                for (p, db) in [(&mut actual, drive), (&mut reference, 0.0)] {
                    p.set_color(1.0).unwrap();
                    p.set_drive_db(db).unwrap();
                    p.reset();
                }
                let gain = 10.0_f64.powf(f64::from(drive) / 20.0);
                let mut a = signal(4096, 2);
                let mut b: Vec<_> = a.iter().map(|x| (f64::from(*x) * gain) as f32).collect();
                actual.process_interleaved(&mut a, 4096).unwrap();
                reference.process_interleaved(&mut b, 4096).unwrap();
                let error = maximum_error(&a, &b);
                assert!(error < 2e-5, "{id}/{rate}/{drive}: {error}");
            }
        }
    }
}

#[test]
fn nonzero_color_follows_physical_time_fade_and_live_zero_is_immediate() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for id in 0..MODEL_NAMES.len() as u32 {
            let frames = rate as usize / 5;
            let mut actual = stage(id, rate, 2, 127);
            let mut wet = stage(id, rate, 2, 512);
            for p in [&mut actual, &mut wet] {
                p.set_color(1.0).unwrap();
                p.reset();
                let mut warm = signal(1024, 2);
                p.process_interleaved(&mut warm, 1024).unwrap();
            }
            actual.set_color(0.37).unwrap();
            let input = signal(frames, 2);
            let mut a = input.clone();
            let mut w = input.clone();
            actual.process_interleaved(&mut a, frames).unwrap();
            wet.process_interleaved(&mut w, frames).unwrap();
            // Independent continuous-time solution for a 10 ms exponential.
            // The implementation snaps to target below 1e-6 and rounds to f32.
            let expected: Vec<_> = input
                .iter()
                .zip(&w)
                .enumerate()
                .map(|(i, (&dry, &wet))| {
                    let amount = f64::from(0.37_f32)
                        + (1.0 - f64::from(0.37_f32))
                            * (-((i / 2 + 1) as f64) / (0.010 * f64::from(rate))).exp();
                    (f64::from(dry) + amount * f64::from(wet - dry)) as f32
                })
                .collect();
            assert!(maximum_error(&a, &expected) < 1e-4, "fade {id}/{rate}");
            actual.set_color(0.0).unwrap();
            a.copy_from_slice(&input);
            actual.process_interleaved(&mut a, frames).unwrap();
            assert_eq!(maximum_error(&a, &input), 0.0);
        }
    }
}

#[test]
fn large_multichannel_blocks_preserve_sentinels_and_match_small_callbacks() {
    for id in 0..MODEL_NAMES.len() as u32 {
        let channels = 32;
        let frames = 8193;
        let mut actual = stage(id, 192_000, channels, 512);
        let mut expected = stage(id, 192_000, channels, 127);
        let mut a = signal(frames, channels);
        let mut b = a.clone();
        a.extend_from_slice(&[123.0, -123.0, 456.0]);
        assert_no_allocs_or_deallocs("32 channel oversized block", || {
            actual.process_interleaved(&mut a, frames).unwrap();
        });
        expected.process_interleaved(&mut b, frames).unwrap();
        assert_eq!(maximum_error(&a[..b.len()], &b), 0.0);
        assert_eq!(&a[b.len()..], &[123.0, -123.0, 456.0]);
        let saved = a.clone();
        assert_no_allocs_or_deallocs("zero frame block", || {
            actual.process_interleaved(&mut a, 0).unwrap();
        });
        assert_eq!(maximum_error(&a, &saved), 0.0);
    }
}

#[test]
fn model_specific_recursive_decay_is_measured_against_unexcited_baseline() {
    // Two seconds exceeds 16 time constants of the slowest default explicit
    // state (Transformer's 120 ms flux). This is a measurement horizon, not a
    // declaration of finite support: the recursive models retain infinite tails.
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for id in 0..MODEL_NAMES.len() as u32 {
            let mut actual = stage(id, rate, 1, 127);
            let mut baseline = stage(id, rate, 1, 512);
            for p in [&mut actual, &mut baseline] {
                p.set_color(1.0).unwrap();
                p.reset();
            }
            let mut excitation = signal(rate as usize / 10, 1);
            let mut silence = vec![0.0; excitation.len()];
            actual
                .process_interleaved(&mut excitation, silence.len())
                .unwrap();
            baseline
                .process_interleaved(&mut silence, excitation.len())
                .unwrap();
            let frames = 2 * rate as usize;
            let mut a = vec![0.0; frames];
            let mut b = a.clone();
            actual.process_interleaved(&mut a, frames).unwrap();
            baseline.process_interleaved(&mut b, frames).unwrap();
            let window = rate as usize / 10;
            let early = maximum_error(&a[..window], &b[..window]);
            let late = maximum_error(&a[frames - window..], &b[frames - window..]);
            assert!(early > 1e-5, "missing excited state {id}/{rate}");
            assert!(late < 1e-5, "decay {id}/{rate}: {late}");
            println!("DECAY model={id} rate={rate} early_peak={early:.9e} late_peak={late:.9e}");
        }
    }
}
