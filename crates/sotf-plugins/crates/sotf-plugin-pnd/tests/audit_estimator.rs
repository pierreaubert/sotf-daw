//! Known-phase wow/flutter, vibrato and weak-reference observation checks.
//!
//! Fixed acceptance bound: 12 cents at confidence >= 0.5, with >= 90% accepted
//! settled frames. Analytic frequency is evaluated at the Hann window center.
//! Lone octave-only references must stay below 0.5 confidence.

// Rust guideline compliant 2026-02-21
use sotf_plugin_pnd::analysis::PndAnalyzer;
use std::f64::consts::TAU;

#[global_allocator]
static ALLOCATOR: sotf_host::CountingAlloc = sotf_host::CountingAlloc;
const N: usize = 2048;
const H: usize = N / 4;

#[test]
fn known_sines_resolve_fractional_hann_bins_independently_of_level_and_phase() {
    let mut cases = 0;
    let mut maximum = 0.0_f64;
    for rate in [44_100_u32, 48_000, 96_000, 192_000] {
        for bin in [5.0, 9.0, 20.0, 100.0] {
            for fraction in [-0.49, -0.35, -0.1, 0.0, 0.1, 0.35, 0.49] {
                for phase in [0.0, 0.7, std::f64::consts::FRAC_PI_2] {
                    for amplitude in [1e-6, 0.3] {
                        let frequency = (bin + fraction) * f64::from(rate) / N as f64;
                        let reference = frequency / 1.01;
                        let mut analyzer = PndAnalyzer::new(N, f64::from(rate), 50.0);
                        let input: Vec<_> = (0..4 * N)
                            .map(|n| {
                                (amplitude
                                    * (TAU * frequency * n as f64 / f64::from(rate) + phase).sin())
                                    as f32
                            })
                            .collect();
                        analyzer.analyze(&input);
                        let (ratio, confidence) =
                            analyzer.estimate_against_reference(reference as f32);
                        let cents = (1200.0 * (f64::from(ratio) / 1.01).log2()).abs();
                        maximum = maximum.max(cents);
                        assert!(
                            confidence >= 0.5,
                            "confidence {confidence}: {rate}/{bin}/{fraction}/{amplitude}/{phase}"
                        );
                        assert!(
                            cents < 1.0,
                            "sub-bin {cents} cents: {rate}/{bin}/{fraction}/{amplitude}/{phase}"
                        );
                        cases += 1;
                    }
                }
            }
        }
    }
    assert_eq!(cases, 672);
    println!(
        "{}",
        serde_json::json!({"case":"hann_fractional_bin_matrix","cases":cases,"max_error_cents":maximum})
    );
}

#[derive(Clone, Copy, Debug)]
enum Motion {
    Offset,
    Wow,
    Flutter,
    Vibrato,
}
impl Motion {
    fn ratio(self, t: f64) -> f64 {
        match self {
            Self::Offset => 1.01,
            Self::Wow => 1.0 + 0.01 * (TAU * 0.5 * t).sin(),
            Self::Flutter => 1.0 + 0.003 * (TAU * 6.0 * t).sin(),
            Self::Vibrato => 2.0_f64.powf(20.0 * (TAU * 5.0 * t).sin() / 1200.0),
        }
    }
}

#[test]
fn harmonic_reference_trajectory_accuracy_survives_fundamental_and_octave_dominance() {
    let mut cases = 0;
    for rate in [44_100_u32, 96_000] {
        for reference in [220.0, 440.0, 880.0] {
            for fundamental in [0.001, 0.03, 1.0, 4.0] {
                for motion in [
                    Motion::Offset,
                    Motion::Wow,
                    Motion::Flutter,
                    Motion::Vibrato,
                ] {
                    let mut analyzer = PndAnalyzer::new(N, f64::from(rate), 50.0);
                    let frames = rate as usize;
                    let mut phase = 0.0_f64;
                    let input: Vec<_> = (0..frames)
                        .map(|n| {
                            let x = 0.1 * fundamental * phase.sin()
                                + 0.2 * (2.0 * phase + 0.3).sin()
                                + 0.1 * (3.0 * phase - 0.2).sin();
                            phase += TAU * reference * motion.ratio(n as f64 / f64::from(rate))
                                / f64::from(rate);
                            x as f32
                        })
                        .collect();
                    let (mut accepted, mut total, mut max_error) = (0_usize, 0_usize, 0.0_f64);
                    for (index, block) in input.as_chunks::<H>().0.iter().enumerate() {
                        let mut estimate = (1.0, 0.0);
                        let heap = sotf_host::test_utils::measure_heap_activity(|| {
                            analyzer.analyze(block);
                            estimate = analyzer.estimate_against_reference(reference as f32);
                        });
                        assert_eq!(heap, (0, 0));
                        let end = (index + 1) * H;
                        if end < 4 * N {
                            continue;
                        }
                        total += 1;
                        if estimate.1 >= 0.5 {
                            accepted += 1;
                            let time = (end as f64 - 1.0 - (N - 1) as f64 / 2.0) / f64::from(rate);
                            let expected = motion.ratio(time);
                            let error = (1200.0 * (f64::from(estimate.0) / expected).log2()).abs();
                            max_error = max_error.max(error);
                        }
                    }
                    let coverage = accepted as f64 / total as f64;
                    println!(
                        "{}",
                        serde_json::json!({"case":"reference_motion","rate":rate,"reference_hz":reference,"fundamental_scale":fundamental,"motion":format!("{motion:?}"),"accepted_fraction":coverage,"max_error_cents":max_error})
                    );
                    assert!(
                        coverage >= 0.9,
                        "coverage {coverage}: {rate}/{reference}/{fundamental}/{motion:?}"
                    );
                    assert!(
                        max_error < 12.0,
                        "error {max_error}: {rate}/{reference}/{fundamental}/{motion:?}"
                    );
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 96);
}

#[test]
fn absent_reference_does_not_fold_an_octave_into_authority() {
    for rate in [44_100_u32, 96_000] {
        for (octave, upper) in [(0.5, 0.0), (2.0, 0.0), (2.0, 0.5)] {
            let reference = 440.0;
            let mut analyzer = PndAnalyzer::new(N, f64::from(rate), 50.0);
            let input: Vec<_> = (0..rate as usize)
                .map(|n| {
                    let phase = TAU * reference * octave * 1.01 * n as f64 / f64::from(rate);
                    (0.3 * (phase.sin() + upper * (2.0 * phase).sin())) as f32
                })
                .collect();
            let mut maximum = 0.0_f32;
            for block in input.as_chunks::<H>().0 {
                analyzer.analyze(block);
                let (ratio, confidence) = analyzer.estimate_against_reference(reference as f32);
                maximum = maximum.max(confidence);
                assert!(
                    confidence < 0.5,
                    "octave {octave} gained confidence {confidence} at{rate}:ratio{ratio}"
                );
            }
            println!(
                "{}",
                serde_json::json!({"case":"absent_octave_reference","rate":rate,"octave":octave,"max_confidence":maximum,"upper_harmonic_scale":upper})
            );
        }
    }
}
