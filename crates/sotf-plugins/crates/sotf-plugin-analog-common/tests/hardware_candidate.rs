//! Native qualification, full-history fallback and loader contract regression tests.

#![cfg(feature = "qa")]

// Rust guideline compliant 2026-02-21
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sotf_host::CountingAlloc;
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_plugin_analog_common::AnalogColorStage;
use sotf_plugin_analog_common::hardware_candidate::{HardwareCandidate, HardwareQualification};
use std::fmt::Write;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;
const FIXTURE: &[u8] = include_bytes!("../../../models/analog-hardware/candidate.synthetic.json");

fn digest(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(&mut result, "{byte:02x}").unwrap();
    }
    result
}
fn model() -> Value {
    serde_json::from_slice(FIXTURE).unwrap()
}
fn load(value: &Value) -> HardwareCandidate {
    let bytes = serde_json::to_vec(value).unwrap();
    HardwareCandidate::from_json(
        &bytes,
        &digest(&bytes),
        &serde_json::to_vec(&value["context"]).unwrap(),
    )
    .unwrap()
}
fn classical(id: u32) -> AnalogColorStage {
    let mut stage = AnalogColorStage::new(1);
    stage.set_model_id(id).unwrap();
    stage.set_color(0.73).unwrap();
    stage.set_drive_db(6.0).unwrap();
    stage.set_character(0.31).unwrap();
    stage.set_output_trim_db(-3.0).unwrap();
    stage.prepare(48_000, 127).unwrap();
    stage.reset();
    stage
}
fn input() -> Vec<f32> {
    (0..5101)
        .map(|i| 0.1 * ((i * 13 % 97) as f32 - 48.0) / 48.0)
        .collect()
}

#[test]
fn missing_disabled_and_rejected_candidates_keep_exact_classical_history() {
    for id in 0..6 {
        for present in [false, true] {
            let mut qualified =
                HardwareQualification::new(classical(id), present.then(|| load(&model())), 48_000)
                    .unwrap();
            let mut reference = classical(id);
            let mut actual = input();
            let mut expected = actual.clone();
            assert_no_allocs_or_deallocs("disabled/missing candidate", || {
                qualified.process(&mut actual, 48_000).unwrap()
            });
            let count = expected.len();
            reference.process_interleaved(&mut expected, count).unwrap();
            assert_eq!(actual, expected);
            assert_eq!(qualified.counters().disabled, actual.len() as u64);
            qualified.set_enabled(true);
            if !present {
                let mut actual = input();
                let mut expected = actual.clone();
                assert_no_allocs_or_deallocs("unavailable enabled candidate", || {
                    qualified.process(&mut actual, 48_000).unwrap()
                });
                let count = expected.len();
                reference.process_interleaved(&mut expected, count).unwrap();
                assert_eq!(actual, expected);
                assert_eq!(qualified.counters().unavailable, actual.len() as u64);
            }
        }
    }
}

#[test]
fn out_of_domain_history_uses_continuous_fallback_then_recovers() {
    let mut qualified =
        HardwareQualification::new(classical(4), Some(load(&model())), 48_000).unwrap();
    qualified.set_enabled(true);
    let mut reference = classical(4);
    let mut x = input();
    for (index, value) in [
        (17, 2.0),
        (103, f32::NAN),
        (200, f32::INFINITY),
        (500, -16.0),
    ] {
        x[index] = value;
    }
    let mut expected = x.clone();
    let count = expected.len();
    reference.process_interleaved(&mut expected, count).unwrap();
    let mut y = x.clone();
    assert_no_allocs_or_deallocs("unsupported neural history", || {
        qualified.process(&mut y, 48_000).unwrap()
    });
    for start in [17, 103, 200, 500] {
        assert_eq!(&y[start..start + 3], &expected[start..start + 3]);
    }
    assert!(y.iter().all(|v| v.is_finite()));
    assert_eq!(qualified.counters().out_of_domain, 12);
    assert_eq!(qualified.counters().neural, y.len() as u64 - 12);
}

#[test]
fn callbacks_reset_toggle_history_and_rejected_rate_are_deterministic() {
    let mut actual =
        HardwareQualification::new(classical(5), Some(load(&model())), 48_000).unwrap();
    let mut other = HardwareQualification::new(classical(5), Some(load(&model())), 48_000).unwrap();
    actual.set_enabled(true);
    other.set_enabled(true);
    let mut a = input();
    let mut b = a.clone();
    for block in a.chunks_mut(193) {
        assert_no_allocs_or_deallocs("candidate callback", || {
            actual.process(block, 48_000).unwrap()
        });
    }
    assert_no_allocs_or_deallocs("candidate single block", || {
        other.process(&mut b, 48_000).unwrap()
    });
    assert_eq!(a, b);
    assert_eq!(actual.counters(), other.counters());
    let before = actual.counters();
    let mut wrong = [0.1, 0.2, 0.3];
    let original = wrong;
    assert_no_allocs_or_deallocs("rate rejection", || {
        assert!(actual.process(&mut wrong, 96_000).is_err())
    });
    assert_eq!(wrong, original);
    assert_eq!(actual.counters(), before);
    assert_no_allocs_or_deallocs("candidate populated reset", || actual.reset());
    let mut a = input();
    actual.process(&mut a, 48_000).unwrap();
    assert_eq!(a, b);
    actual.reset();
    other.reset();
    let mut a = input();
    let mut b = a.clone();
    actual.set_enabled(false);
    actual.process(&mut a[..101], 48_000).unwrap();
    actual.set_enabled(true);
    actual.process(&mut a[101..], 48_000).unwrap();
    other.process(&mut b, 48_000).unwrap();
    assert_eq!(&a[101..], &b[101..]);
}

#[test]
fn explicit_hash_context_calibration_schema_policy_and_software_bounds_reject() {
    let value = model();
    let context = serde_json::to_vec(&value["context"]).unwrap();
    assert!(HardwareCandidate::from_json(FIXTURE, &"0".repeat(64), &context).is_err());
    let mut changed = value["context"].clone();
    changed["hardware"]["unit_id"] = json!("different-unit");
    assert!(
        HardwareCandidate::from_json(
            FIXTURE,
            &digest(FIXTURE),
            &serde_json::to_vec(&changed).unwrap()
        )
        .is_err()
    );
    for (path, replacement) in [
        (vec!["deployment_admitted"], json!(true)),
        (vec!["global_output_bound"], json!(0)),
        (vec!["architecture", "taps"], json!(0)),
        (vec!["architecture", "hidden"], json!(33)),
        (vec!["hidden_weights"], json!([[1.0]])),
        (vec!["context", "channels"], json!(2)),
        (vec!["context", "calibration", "shared_clock"], json!(false)),
        (
            vec!["context", "rights", "training_authorized"],
            json!(false),
        ),
        (vec!["policy", "maximum_nmse_db"], json!(-5.0)),
    ] {
        let mut invalid = value.clone();
        let mut field = &mut invalid;
        for key in path {
            field = &mut field[key];
        }
        *field = replacement;
        let bytes = serde_json::to_vec(&invalid).unwrap();
        assert!(
            HardwareCandidate::from_json(
                &bytes,
                &digest(&bytes),
                &serde_json::to_vec(&invalid["context"]).unwrap()
            )
            .is_err()
        );
    }
    assert!(
        HardwareCandidate::from_json(&vec![b' '; 1024 * 1024 + 1], &digest(FIXTURE), &context)
            .is_err()
    );
}

#[test]
fn numeric_context_equivalence_preserves_large_integer_identity() {
    let mut value = model();
    value["context"]["hardware"]["controls"]["gain"] = json!(2.0);
    let bytes = serde_json::to_vec(&value).unwrap();
    let mut expected = value["context"].clone();
    expected["hardware"]["controls"]["gain"] = json!(2);
    assert!(
        HardwareCandidate::from_json(
            &bytes,
            &digest(&bytes),
            &serde_json::to_vec(&expected).unwrap()
        )
        .is_ok()
    );
    value["context"]["capture_counter"] = json!(9007199254740993_u64);
    let bytes = serde_json::to_vec(&value).unwrap();
    expected = value["context"].clone();
    expected["capture_counter"] = json!(9007199254740992.0_f64);
    assert!(
        HardwareCandidate::from_json(
            &bytes,
            &digest(&bytes),
            &serde_json::to_vec(&expected).unwrap()
        )
        .is_err()
    );
}

#[test]
fn maximum_dimensions_process_without_heap_and_mono_rate_contract_is_explicit() {
    let mut value = model();
    value["architecture"]["taps"] = json!(64);
    value["architecture"]["hidden"] = json!(32);
    value["history_frames"] = json!(63);
    value["linear"] = json!(vec![0.0; 64]);
    value["hidden_weights"] = json!(vec![vec![0.0; 32]; 64]);
    value["hidden_bias"] = json!(vec![0.0; 32]);
    value["output_weights"] = json!(vec![0.0; 32]);
    value["output_bias"] = json!(0.0);
    value["global_output_bound"] = json!(0.0);
    value["hammerstein"]["weights"] = json!(vec![0.0; 321]);
    let candidate = load(&value);
    assert_eq!(candidate.taps(), 64);
    let mut qualified = HardwareQualification::new(classical(0), Some(candidate), 48_000).unwrap();
    qualified.set_enabled(true);
    let mut x = input();
    assert_no_allocs_or_deallocs("maximum candidate", || {
        qualified.process(&mut x, 48_000).unwrap()
    });
    assert!(x.iter().all(|v| *v == 0.0));
    assert!(HardwareQualification::new(AnalogColorStage::new(2), None, 48_000).is_err());
    assert!(HardwareQualification::new(classical(0), Some(load(&model())), 96_000).is_err());
}
