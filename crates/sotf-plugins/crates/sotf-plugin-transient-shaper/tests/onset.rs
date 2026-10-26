//! Bounded onset controls, deterministic fallback and explicit reset semantics.
#![cfg(feature = "qa")]
// Rust guideline compliant 2026-02-21
use sha2::{Digest, Sha256};
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_transient_shaper::onset::{OnsetModel, features};
use sotf_plugin_transient_shaper::{TransientShaperPlugin, TransientShaperPluginParams};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

fn model(bias: f64) -> OnsetModel {
    let raw = serde_json::json!({"format":"sotf-transient-onset-assist-v1","weights":vec![[0.0;8];6],"bias":vec![0.0;8],
        "output_weights":vec![0.0;8],"output_bias":bias,"policy":{"sample_rates":[44100,48000,96000],"analysis_ms":2.0,"control_smoothing_ms":5.0,
        "maximum_attack_modulation":0.25,"confidence":0.9,"added_lookahead_samples":0,"fallback":"independent_native_attack","label":"causal_hop_contains_generated_onset"},
        "provenance":"project-generated onset/steady/noise signals; GPL-3.0-or-later; no external audio","deployment_admitted":false});
    let bytes = serde_json::to_vec(&raw).unwrap();
    let sha: String = Sha256::digest(&bytes)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect();
    OnsetModel::from_json(&bytes, &sha).unwrap()
}

fn make(channels: usize, rate: u32, attack: f32) -> TransientShaperPlugin {
    let mut p = TransientShaperPlugin::from_params(
        channels,
        TransientShaperPluginParams {
            attack,
            sustain: 20.0,
            sensitivity_db: 0.0,
            output_gain_db: 0.0,
            mix: 1.0,
        },
    )
    .unwrap();
    p.initialize(f64::from(rate)).unwrap();
    p
}

fn source(channels: usize) -> Vec<f32> {
    (0..5101 * channels)
        .map(|i| {
            let frame = i / channels;
            let value = ((frame * 17 % 103) as f32 - 51.0) / 1024.0;
            if i % channels == 0 {
                value
            } else {
                value * 0.25
            }
        })
        .collect()
}

fn render(
    p: &mut TransientShaperPlugin,
    input: &[f32],
    channels: usize,
    rate: u32,
    block: usize,
) -> Vec<f32> {
    let mut output = input.to_vec();
    for x in output.chunks_mut(block * channels) {
        let context = ProcessContext::new(rate, x.len() / channels);
        assert_no_allocs_or_deallocs("onset process", || {
            p.process_in_place(x, &context).unwrap();
        });
    }
    output
}

#[test]
fn missing_uncertain_and_unsupported_evidence_keep_exact_classical_output() {
    for rate in [44100, 48000, 96000, 192000] {
        for channels in [1, 2] {
            let x = source(channels);
            let expected = render(&mut make(channels, rate, 60.0), &x, channels, rate, 193);
            for network in [None, Some(model(0.0))] {
                let mut candidate = make(channels, rate, 60.0);
                candidate.configure_onset_assist(network).unwrap();
                assert_eq!(render(&mut candidate, &x, channels, rate, 17), expected);
                assert_eq!(
                    candidate.onset_assist_snapshot().unwrap().attack_multiplier,
                    1.0
                );
            }
            if rate == 192000 {
                let mut candidate = make(channels, rate, 60.0);
                candidate.configure_onset_assist(Some(model(10.0))).unwrap();
                assert_eq!(render(&mut candidate, &x, channels, rate, 17), expected);
            }
        }
    }
}

#[test]
fn selected_controls_preserve_detectors_channels_and_callback_boundaries() {
    for rate in [44100, 48000, 96000] {
        for bias in [-10.0, 10.0] {
            let x = source(2);
            let mut candidate = make(2, rate, 60.0);
            candidate.configure_onset_assist(Some(model(bias))).unwrap();
            let actual = render(&mut candidate, &x, 2, rate, 193);
            let mut classical = make(2, rate, 60.0);
            let baseline = render(&mut classical, &x, 2, rate, 17);
            assert_ne!(actual, baseline);
            assert_eq!(
                candidate.onset_classical_envelopes(),
                classical.onset_classical_envelopes()
            );
            let snapshot = candidate.onset_assist_snapshot().unwrap();
            assert!((0.75..=1.25).contains(&snapshot.attack_multiplier));
            assert_eq!(snapshot.counters.frames, 5101);
            assert_eq!(snapshot.counters.hops, 5101 / snapshot.hop_frames as u64);
            assert_eq!(
                snapshot.counters.hops,
                snapshot.counters.onset
                    + snapshot.counters.nononset
                    + snapshot.counters.uncertain
                    + snapshot.counters.unavailable
                    + snapshot.counters.invalid_or_silent
            );
            for pair in actual.as_chunks::<2>().0 {
                assert_eq!(pair[1], pair[0] * 0.25);
            }
            assert_no_allocs_or_deallocs("onset reset", || candidate.reset());
            assert_eq!(render(&mut candidate, &x, 2, rate, 17), actual);
            assert_eq!(candidate.latency_samples(), 0);
        }
    }
}

#[test]
fn initialize_preserves_history_while_explicit_reset_starts_a_fresh_epoch() {
    let x = source(1);
    let mut a = make(1, 48000, 60.0);
    a.configure_onset_assist(Some(model(10.0))).unwrap();
    let mut b = make(1, 48000, 60.0);
    b.configure_onset_assist(Some(model(10.0))).unwrap();
    let first = render(&mut a, &x, 1, 48000, 193);
    render(&mut b, &x, 1, 48000, 193);
    let before = a.onset_assist_snapshot().unwrap();
    a.initialize(48000.0).unwrap();
    assert_eq!(a.onset_assist_snapshot().unwrap().counters, before.counters);
    assert_eq!(
        render(&mut a, &x, 1, 48000, 17),
        render(&mut b, &x, 1, 48000, 193)
    );
    let fast = a.onset_classical_envelopes().0.to_vec();
    a.initialize(96000.0).unwrap();
    assert_eq!(a.onset_classical_envelopes().0, fast);
    assert!(a.configure_onset_assist(None).is_err());
    a.initialize(48000.0).unwrap();
    a.reset();
    assert_eq!(a.onset_assist_snapshot().unwrap().counters.frames, 0);
    assert_eq!(render(&mut a, &x, 1, 48000, 137), first);
}

#[test]
fn bounded_controls_do_not_create_audio_tails_or_change_native_peak_protection() {
    let x = source(2).iter().map(|x| x * 16.0).collect::<Vec<_>>();
    for attack in [-100.0, -60.0, 0.0, 60.0, 100.0] {
        for sustain in [-100.0, 100.0] {
            let mut p = TransientShaperPlugin::from_params(
                2,
                TransientShaperPluginParams {
                    attack,
                    sustain,
                    sensitivity_db: 0.0,
                    output_gain_db: 12.0,
                    mix: 1.0,
                },
            )
            .unwrap();
            p.initialize(48000.0).unwrap();
            p.configure_onset_assist(Some(model(10.0))).unwrap();
            let out = render(&mut p, &x, 2, 48000, 193);
            assert!(out.iter().all(|x| x.is_finite() && x.abs() < 2.0));
            let zero = render(&mut p, &vec![0.0; 1024], 2, 48000, 17);
            assert!(zero.iter().all(|x| *x == 0.0));
            assert_eq!(p.tail_length(), sotf_host::plugin::TailLength::Finite(0));
        }
    }
}

#[test]
fn feature_loading_and_invalid_source_domains_are_explicit() {
    let mut unprepared = TransientShaperPlugin::new(1);
    assert!(unprepared.configure_onset_assist(None).is_err());
    let mut wide = make(3, 48000, 60.0);
    assert!(wide.configure_onset_assist(None).is_err());
    let x = features(0.01, 0.1, 0.01, 0.0, 0.1, 0.1, 0.1).unwrap();
    for (a, b) in x.into_iter().zip([0.8, 0.0, 0.5, 0.0, 0.0, 0.5]) {
        assert!((a - b).abs() < 1e-12);
    }
    assert_eq!(model(0.0).predict(&x), Some(0.5));
    assert!(features(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0).is_none());
    assert!(model(0.0).predict(&[f64::NAN; 6]).is_none());
    assert!(OnsetModel::from_json(b"{}", &"0".repeat(64)).is_err());
    let mut a = make(1, 48000, 60.0);
    a.configure_onset_assist(Some(model(10.0))).unwrap();
    let mut b = make(1, 48000, 60.0);
    b.configure_onset_assist(Some(model(10.0))).unwrap();
    let mut invalid = [f32::NAN];
    assert!(
        a.process_in_place(&mut invalid, &ProcessContext::new(48000, 1))
            .is_err()
    );
    let x = source(1);
    assert_eq!(
        render(&mut a, &x, 1, 48000, 17),
        render(&mut b, &x, 1, 48000, 193)
    );
}

#[test]
fn overloaded_windows_cannot_supply_previous_energy_to_model_reentry() {
    let mut x = source(1);
    x.truncate(96 * 6);
    x[96 * 3] = 1.2;
    let mut candidate = make(1, 48000, 60.0);
    candidate.configure_onset_assist(Some(model(10.0))).unwrap();
    let mut classical = make(1, 48000, 60.0);
    let actual = render(&mut candidate, &x, 1, 48000, 17);
    let expected = render(&mut classical, &x, 1, 48000, 193);
    assert_eq!(actual[288..575], expected[288..575]);
    let counters = candidate.onset_assist_snapshot().unwrap().counters;
    assert_eq!(counters.hops, 6);
    assert_eq!(counters.invalid_or_silent, 2);
    assert_eq!(counters.onset, 4);
}

#[test]
fn zero_attack_leaves_native_sustain_output_exactly_unchanged() {
    for rate in [44100, 48000, 96000] {
        let x = source(2);
        let mut candidate = make(2, rate, 0.0);
        candidate.configure_onset_assist(Some(model(10.0))).unwrap();
        assert_eq!(
            render(&mut candidate, &x, 2, rate, 17),
            render(&mut make(2, rate, 0.0), &x, 2, rate, 193)
        );
    }
}

#[test]
fn frozen_classifier_matches_independent_python_probabilities() {
    let bytes = include_bytes!("../../../models/transient-onset-assist/model.synthetic.json");
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../models/transient-onset-assist/parity.synthetic.json"
    ))
    .unwrap();
    let model = OnsetModel::from_json(bytes, fixture["model_sha256"].as_str().unwrap()).unwrap();
    let mut maximum = 0.0_f64;
    for point in fixture["points"].as_array().unwrap() {
        let row: Vec<_> = point["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_f64().unwrap())
            .collect();
        let x: [f64; 6] = row.try_into().unwrap();
        maximum = maximum.max(
            (f64::from(model.predict(&x).unwrap()) - point["prediction"].as_f64().unwrap()).abs(),
        );
    }
    println!("frozen onset probability parity maximum={maximum}");
    assert!(maximum < 2e-7);
}
