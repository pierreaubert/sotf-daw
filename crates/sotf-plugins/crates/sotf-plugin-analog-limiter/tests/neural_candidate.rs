//! Native ceiling, fallback and state qualification for unadmitted coloration.

#![cfg(feature = "qa")]

// Rust guideline compliant 2026-02-21
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{
    CountingAlloc, ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext,
};
use sotf_plugin_analog_common::hardware_candidate::HardwareCandidate;
use sotf_plugin_analog_limiter::candidate::LimiterQualification;
use sotf_plugin_analog_limiter::{AnalogLimiterPlugin, AnalogLimiterPluginParams};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

fn candidate(stress: bool) -> HardwareCandidate {
    let (bytes, hash): (&[u8], &str) = if stress {
        (
            include_bytes!(
                "../../../models/analog-limiter-candidate/candidate.overdrive.synthetic.json"
            ),
            "4a45728b5b426c6707536510d7f756a51ea0dc2c5389baaa9fd46c42759ab902",
        )
    } else {
        (
            include_bytes!("../../../models/analog-hardware/candidate.synthetic.json"),
            "f4eea207a98774d8f9027cac2d96bb031b736a7dd8358cabf5419f5f551bdf44",
        )
    };
    let value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    HardwareCandidate::from_json(bytes, hash, &serde_json::to_vec(&value["context"]).unwrap())
        .unwrap()
}

fn params(model: &str) -> AnalogLimiterPluginParams {
    AnalogLimiterPluginParams {
        threshold: -6.0,
        release: 37.0,
        lookahead: 3.0,
        analog_model: model.into(),
        analog_color: 0.73,
        analog_drive: 6.0,
        analog_character: 0.31,
        analog_trim: 9.0,
        ..Default::default()
    }
}

fn input() -> Vec<f32> {
    (0..12_301)
        .map(|i| {
            if i > 8193 {
                0.0
            } else {
                ((i * 13 % 97) as f32 - 48.0) / 32.0
            }
        })
        .collect()
}

#[test]
fn disabled_missing_and_rejected_candidates_match_production_for_every_fallback_model() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for model in sotf_plugin_analog_common::MODEL_NAMES {
            for enabled in [false, true] {
                let p = params(model);
                let mut qualified = LimiterQualification::new(p.clone(), None, rate).unwrap();
                qualified.set_enabled(enabled);
                let mut reference = AnalogLimiterPlugin::from_params(1, p).unwrap();
                reference.initialize(f64::from(rate)).unwrap();
                let mut actual = input();
                let mut expected = actual.clone();
                for (a, b) in actual.chunks_mut(193).zip(expected.chunks_mut(193)) {
                    let context = ProcessContext::new(rate, a.len());
                    assert_no_allocs_or_deallocs("missing candidate limiter", || {
                        qualified.process(a, &context).unwrap();
                    });
                    reference.process_in_place(b, &context).unwrap();
                }
                assert_eq!(actual, expected, "{model}, {rate}, {enabled}");
                assert_eq!(qualified.latency_samples(), reference.latency_samples());
                let counts = qualified.counters();
                assert_eq!(counts.neural, 0);
                assert_eq!(counts.unavailable + counts.disabled, actual.len() as u64);
            }
        }
    }
}

#[test]
fn arbitrary_neural_gain_and_explicit_release_never_bypass_the_fully_wet_ceiling() {
    let mut q =
        LimiterQualification::new(params("Harmonics"), Some(candidate(true)), 48_000).unwrap();
    q.set_enabled(true);
    let threshold_id = ParameterId::from("threshold");
    let release_id = ParameterId::from("release");
    for (threshold, release) in [(-6.0, 10.0), (-20.0, 1000.0), (-3.0, 37.0)] {
        assert_no_allocs_or_deallocs("explicit threshold/release update", || {
            q.set_core_parameter(threshold_id.clone(), ParameterValue::Float(threshold))
                .unwrap();
            q.set_core_parameter(release_id.clone(), ParameterValue::Float(release))
                .unwrap();
        });
        let mut samples = [0.01_f32; 317];
        assert_no_allocs_or_deallocs("neural output clamp", || {
            q.process(&mut samples, &ProcessContext::new(48_000, 317))
                .unwrap();
        });
        let ceiling = 10_f32.powf(threshold / 20.0);
        assert!(samples.iter().all(|&s| s == ceiling));
    }
    assert_eq!(q.counters().neural, 951);
    let mix = ParameterId::from("mix");
    q.set_core_parameter(mix.clone(), ParameterValue::Float(0.0))
        .unwrap();
    let mut dry_mix = [0.01; 31];
    q.process(&mut dry_mix, &ProcessContext::new(48_000, 31))
        .unwrap();
    // The production dry-blend contract makes no ceiling promise.
    assert!(dry_mix.iter().all(|&s| s == 2.0));
    q.set_core_parameter(mix, ParameterValue::Float(1.0))
        .unwrap();
    q.process(&mut dry_mix, &ProcessContext::new(48_000, 31))
        .unwrap();
    assert!(dry_mix.iter().all(|&s| s == 10_f32.powf(-3.0 / 20.0)));
}

#[test]
fn trained_candidate_callbacks_and_resets_preserve_core_and_color_state() {
    let p = params("Tape");
    let mut a = LimiterQualification::new(p.clone(), Some(candidate(false)), 48_000).unwrap();
    let mut b = LimiterQualification::new(p.clone(), Some(candidate(false)), 48_000).unwrap();
    a.set_enabled(true);
    b.set_enabled(true);
    let mut actual = input();
    let mut expected = actual.clone();
    for block in actual.chunks_mut(17) {
        assert_no_allocs_or_deallocs("trained limiter callback", || {
            a.process(block, &ProcessContext::new(48_000, block.len()))
                .unwrap();
        });
    }
    b.process(&mut expected, &ProcessContext::new(48_000, actual.len()))
        .unwrap();
    assert_eq!(actual, expected);
    assert_eq!(a.counters(), b.counters());
    assert!(a.counters().neural > 0 && a.counters().out_of_domain > 0);
    let ceiling = 10_f32.powf(-6.0 / 20.0);
    assert!(actual.iter().all(|v| v.is_finite() && v.abs() <= ceiling));
    assert_no_allocs_or_deallocs("populated limiter candidate reset", || a.reset());
    let mut reset = input();
    a.process(&mut reset, &ProcessContext::new(48_000, actual.len()))
        .unwrap();
    assert_eq!(reset, actual);
    // Disabling returns to the continuously advanced production color history.
    let mut reference = AnalogLimiterPlugin::from_params(1, p).unwrap();
    reference.initialize(48_000.0).unwrap();
    let mut warm = input();
    let warm_frames = warm.len();
    reference
        .process_in_place(&mut warm, &ProcessContext::new(48_000, warm_frames))
        .unwrap();
    a.set_enabled(false);
    let mut actual = input();
    let mut expected = actual.clone();
    a.process(&mut actual, &ProcessContext::new(48_000, expected.len()))
        .unwrap();
    reference
        .process_in_place(&mut expected, &ProcessContext::new(48_000, actual.len()))
        .unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn rejected_geometry_and_controls_leave_histories_and_output_unchanged() {
    let p = params("Transformer");
    assert!(LimiterQualification::new(p.clone(), Some(candidate(false)), 96_000).is_err());
    let mut q = LimiterQualification::new(p.clone(), Some(candidate(false)), 48_000).unwrap();
    let mut fresh = LimiterQualification::new(p, Some(candidate(false)), 48_000).unwrap();
    q.set_enabled(true);
    fresh.set_enabled(true);
    let mut samples = [0.01; 7];
    for context in [
        ProcessContext::new(96_000, 7),
        ProcessContext::new(48_000, 8),
    ] {
        assert!(q.process(&mut samples, &context).is_err());
        assert_eq!(samples, [0.01; 7]);
    }
    for (key, value) in [
        ("threshold", ParameterValue::Float(f32::NAN)),
        ("release", ParameterValue::Float(0.0)),
        ("lookahead", ParameterValue::Float(10.0)),
        ("analog_color", ParameterValue::Float(0.0)),
    ] {
        assert!(q.set_core_parameter(ParameterId::from(key), value).is_err());
    }
    let mut a = input();
    let mut b = a.clone();
    q.process(&mut a, &ProcessContext::new(48_000, b.len()))
        .unwrap();
    fresh
        .process(&mut b, &ProcessContext::new(48_000, a.len()))
        .unwrap();
    assert_eq!(a, b);
    assert_eq!(q.counters(), fresh.counters());
}

#[test]
fn learned_color_matches_independent_dense_network_after_actual_native_limiter_core() {
    use sotf_plugin_limiter::{LimiterPlugin, LimiterPluginParams};

    let p = params("Harmonics");
    let mut qualified =
        LimiterQualification::new(p.clone(), Some(candidate(false)), 48_000).unwrap();
    qualified.set_enabled(true);
    let mut core = LimiterPlugin::from_params(
        1,
        LimiterPluginParams {
            threshold_db: p.threshold,
            release_ms: p.release,
            lookahead_ms: p.lookahead,
            soft: p.soft,
            true_peak: p.true_peak,
            mix: p.mix,
            isp_mode: false,
            dual_release: false,
            feed_forward: false,
            link_amount: sotf_plugin_limiter::params::default_link_amount(),
            oversampling: 0,
        },
    );
    core.initialize(48_000.0).unwrap();
    let source: Vec<_> = (0..5101)
        .map(|i| ((i * 13 % 97) as f32 - 48.0) / 2048.0)
        .collect();
    let mut actual = source.clone();
    let mut limited = source;
    qualified
        .process(&mut actual, &ProcessContext::new(48_000, 5101))
        .unwrap();
    core.process_in_place(&mut limited, &ProcessContext::new(48_000, 5101))
        .unwrap();
    let model: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../models/analog-hardware/candidate.synthetic.json"
    ))
    .unwrap();
    let scale = model["input_scale"].as_f64().unwrap();
    let taps = model["architecture"]["taps"].as_u64().unwrap() as usize;
    let hidden = model["architecture"]["hidden"].as_u64().unwrap() as usize;
    let ceiling = 10_f32.powf(p.threshold / 20.0);
    let mut max_error = 0.0_f64;
    for (i, &value) in actual.iter().enumerate() {
        // Offline dense dot products use chronological lookup instead of the
        // production fixed ring. Preserve native f32 core output as network input.
        let history: Vec<_> = (0..taps)
            .map(|k| {
                i.checked_sub(k)
                    .map_or(0.0, |j| f64::from(limited[j]) / scale)
            })
            .collect();
        let linear: f64 = history
            .iter()
            .enumerate()
            .map(|(k, x)| x * model["linear"][k].as_f64().unwrap())
            .sum();
        let nonlinear: f64 = (0..hidden)
            .map(|j| {
                let z = model["hidden_bias"][j].as_f64().unwrap()
                    + history
                        .iter()
                        .enumerate()
                        .map(|(k, x)| x * model["hidden_weights"][k][j].as_f64().unwrap())
                        .sum::<f64>();
                z.tanh() * model["output_weights"][j].as_f64().unwrap()
            })
            .sum();
        let expected = ((model["output_bias"].as_f64().unwrap() + linear + nonlinear) as f32)
            .clamp(-ceiling, ceiling);
        max_error = max_error.max(f64::from((value - expected).abs()));
    }
    assert!(
        max_error < 3e-8,
        "independent neural-chain error {max_error}"
    );
    assert_eq!(qualified.counters().neural, actual.len() as u64);
    println!("independent_neural_chain_max_abs_error={max_error}");
}

#[test]
fn pathological_input_recovery_remains_finite_and_sample_bounded() {
    for enabled in [false, true] {
        let mut q =
            LimiterQualification::new(params("Console Preamp"), Some(candidate(false)), 48_000)
                .unwrap();
        q.set_enabled(enabled);
        let mut x = input();
        for (i, value) in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MAX]
            .into_iter()
            .enumerate()
        {
            x[193 + i * 37] = value;
        }
        for block in x.chunks_mut(17) {
            assert_no_allocs_or_deallocs("pathological candidate input", || {
                q.process(block, &ProcessContext::new(48_000, block.len()))
                    .unwrap();
            });
        }
        let ceiling = 10_f32.powf(-6.0 / 20.0);
        assert!(x.iter().all(|v| v.is_finite() && v.abs() <= ceiling));
    }
}

#[test]
#[ignore = "Local timing evidence; run alone with one test thread"]
fn measure_native_candidate_processing_cost() {
    use std::hint::black_box;
    use std::time::Instant;

    let p = params("Tape");
    let mut production = AnalogLimiterPlugin::from_params(1, p.clone()).unwrap();
    production.initialize(48_000.0).unwrap();
    let source: Vec<_> = (0..65_537)
        .map(|i| ((i * 13 % 97) as f32 - 48.0) / 2048.0)
        .collect();
    let mut samples = source.clone();
    let mut results = serde_json::Map::new();
    for mode in ["production", "missing", "disabled", "enabled"] {
        let mut qualified = LimiterQualification::new(
            p.clone(),
            (mode != "missing").then(|| candidate(false)),
            48_000,
        )
        .unwrap();
        qualified.set_enabled(mode == "enabled" || mode == "missing");
        let mut durations = Vec::new();
        for trial in 0..23 {
            production.reset();
            qualified.reset();
            samples.copy_from_slice(&source);
            let start = Instant::now();
            for block in samples.chunks_mut(193) {
                let context = ProcessContext::new(48_000, block.len());
                if mode == "production" {
                    production
                        .process_in_place(black_box(block), &context)
                        .unwrap();
                } else {
                    qualified.process(black_box(block), &context).unwrap();
                }
            }
            let elapsed = start.elapsed().as_secs_f64();
            black_box(&samples);
            if trial > 1 {
                durations.push(elapsed);
            }
        }
        durations.sort_by(f64::total_cmp);
        results.insert(mode.into(), serde_json::json!({"median_seconds":durations[durations.len()/2],"minimum_seconds":durations[0],"trials":durations.len()}));
    }
    println!(
        "{}",
        serde_json::json!({
            "sample_rate":48000,"frames":source.len(),"callback_frames":193,
            "latency_samples":production.latency_samples(),"added_neural_lookahead_samples":0,
            "qualification_inline_bytes_excluding_heap_owned_storage":std::mem::size_of::<LimiterQualification>(),
            "candidate_taps":3,"candidate_hidden":6,"timings":results,
            "scope":"Local native QA stream measurement only; resets, copies and model loading excluded. Includes continuously running classical fallback. Not portable realtime deadline evidence or complete heap memory accounting."
        })
    );
}
