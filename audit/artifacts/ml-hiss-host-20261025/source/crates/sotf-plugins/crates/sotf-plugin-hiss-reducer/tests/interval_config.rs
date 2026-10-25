//! Structural review setup and retained hosted diagnostics.

// Rust guideline compliant 2026-02-21
use sotf_host::{CountingAlloc, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_hiss_reducer::interval_config::{IntervalReviewConfig, IntervalReviewPreparation};
use sotf_plugin_hiss_reducer::snapshot::ProfileSnapshot;
use sotf_plugin_hiss_reducer::{HissReducerPlugin, HissReducerPluginParams};
use std::sync::Arc;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

fn config() -> IntervalReviewConfig {
    IntervalReviewConfig {
        model_path: std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../models/hiss-intervals/model.synthetic.json"),
        sha256: "bae4c80e241f85e98edda3d017bc48a964e9496d10cd436b58dad9fe978c32de".into(),
        capture_setup_id: "synthetic-development-chain".into(),
        maximum_endpoint_age_frames: 48000,
    }
}

fn snapshot(p: &HissReducerPlugin) -> Arc<ProfileSnapshot> {
    p.get_data().unwrap().downcast().unwrap()
}

#[test]
fn legacy_defaults_and_unavailable_configuration_preserve_classical_audio() {
    let legacy: HissReducerPluginParams = serde_json::from_str("{}").unwrap();
    assert!(legacy.interval_review.is_none());
    assert!(
        serde_json::to_value(&legacy)
            .unwrap()
            .get("interval_review")
            .is_none()
    );
    let mut missing = config();
    if cfg!(feature = "interval-proposals") {
        missing.model_path = missing
            .model_path
            .with_file_name("missing-review-model.json");
    }
    let parameters = HissReducerPluginParams {
        interval_review: Some(missing.clone()),
        ..legacy.clone()
    };
    let mut configured = HissReducerPlugin::try_from_params(1, parameters).unwrap();
    let mut classical = HissReducerPlugin::from_params(1, legacy);
    configured.initialize(48000.0).unwrap();
    classical.initialize(48000.0).unwrap();
    assert!(matches!(
        snapshot(&configured).interval_review_preparation().unwrap(),
        IntervalReviewPreparation::Unavailable { .. }
    ));
    assert_eq!(configured.persisted_params().interval_review, Some(missing));
    let mut a = vec![0.01; 48000];
    let mut b = a.clone();
    let context = ProcessContext::new(48000.0, a.len());
    sotf_host::test_utils::assert_no_allocs_or_deallocs("unavailable review fallback", || {
        configured.process_in_place(&mut a, &context).unwrap();
        classical.process_in_place(&mut b, &context).unwrap();
    });
    assert_eq!(a, b);
    assert!(!configured.has_captured_profile());
}

#[test]
fn structural_identity_validation_is_transactional() {
    for invalid_rate in [f64::NAN, f64::INFINITY, 0.0, -1.0] {
        assert!(config().validate(invalid_rate).is_err());
    }
    let mut p = HissReducerPlugin::new(1);
    let snapshot = snapshot(&p);
    let mut digest = config();
    digest.sha256 = "not-a-digest".into();
    let mut path = config();
    path.model_path = std::path::PathBuf::new();
    let mut setup = config();
    setup.capture_setup_id = "x".repeat(129);
    for invalid in [digest, path, setup] {
        assert!(
            p.configure_interval_review_from_config(Some(invalid))
                .is_err()
        );
        assert!(matches!(
            snapshot.interval_review_preparation().unwrap(),
            IntervalReviewPreparation::Disabled
        ));
        assert!(p.persisted_params().interval_review.is_none());
    }
    let mut invalid = config();
    invalid.maximum_endpoint_age_frames = 480001;
    assert!(
        p.configure_interval_review_from_config(Some(invalid))
            .is_err()
    );
    let mut value = serde_json::to_value(config()).unwrap();
    value["automatic_capture"] = serde_json::json!(true);
    assert!(serde_json::from_value::<IntervalReviewConfig>(value).is_err());
}

#[cfg(feature = "interval-proposals")]
#[test]
fn retained_host_reader_observes_final_result_reset_and_reconfiguration() {
    use sotf_host::test_utils::assert_no_allocs_or_deallocs;
    use std::time::{Duration, Instant};
    let mut p = HissReducerPlugin::try_from_params(
        1,
        HissReducerPluginParams {
            interval_review: Some(config()),
            ..Default::default()
        },
    )
    .unwrap();
    p.initialize(48000.0).unwrap();
    let original_snapshot = snapshot(&p);
    let IntervalReviewPreparation::Ready(reader) =
        original_snapshot.interval_review_preparation().unwrap()
    else {
        panic!("model not prepared");
    };
    assert_eq!(
        reader.context().capture_setup_id,
        "synthetic-development-chain"
    );
    let mut state = 20811026_u32;
    let mut audio: Vec<_> = (0..48000)
        .map(|_| {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            ((f64::from(state) / 4294967296.0 - 0.5) * 0.02) as f32
        })
        .collect();
    let context = ProcessContext::new(48000.0, audio.len()).with_sample_position(900);
    assert_no_allocs_or_deallocs("hosted review callback", || {
        p.process_in_place(&mut audio, &context)
            .map(|_| ())
            .unwrap()
    });
    // No subsequent process/get_data callback is needed for worker publication.
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let mut observation = None;
        assert_no_allocs_or_deallocs("retained review reader", || {
            observation = reader.status().unwrap().observation
        });
        if let Some(value) = observation {
            assert_eq!((value.start_frame, value.end_frame), (900, 48900));
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!p.has_captured_profile());
    let epoch = reader.status().unwrap().epoch;
    assert_no_allocs_or_deallocs("hosted review reset", || p.reset());
    assert!(reader.status().unwrap().epoch > epoch);
    assert!(reader.status().unwrap().observation.is_none());
    assert!(Arc::ptr_eq(&original_snapshot, &snapshot(&p)));
    let mut unavailable = config();
    unavailable.model_path = unavailable.model_path.with_file_name("missing-model.json");
    p.configure_interval_review_from_config(Some(unavailable))
        .unwrap();
    assert!(!reader.status().unwrap().worker_alive);
    assert!(matches!(
        original_snapshot.interval_review_preparation().unwrap(),
        IntervalReviewPreparation::Unavailable { .. }
    ));
    p.configure_interval_review_from_config(None).unwrap();
    assert!(matches!(
        original_snapshot.interval_review_preparation().unwrap(),
        IntervalReviewPreparation::Disabled
    ));
}

#[cfg(feature = "interval-proposals")]
#[test]
fn model_is_read_once_and_reopen_refuses_changed_or_oversized_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let mut chosen = config();
    let raw = std::fs::read(&chosen.model_path).unwrap();
    chosen.model_path = temp.path().join("model.json");
    std::fs::write(&chosen.model_path, &raw).unwrap();
    let params = HissReducerPluginParams {
        interval_review: Some(chosen.clone()),
        ..Default::default()
    };
    let mut original = HissReducerPlugin::try_from_params(1, params.clone()).unwrap();
    std::fs::write(&chosen.model_path, b"{}").unwrap();
    original.initialize(48000.0).unwrap();
    assert!(matches!(
        snapshot(&original).interval_review_preparation().unwrap(),
        IntervalReviewPreparation::Ready(_)
    ));
    let reopen = HissReducerPlugin::try_from_params(1, original.persisted_params()).unwrap();
    assert!(matches!(
        snapshot(&reopen).interval_review_preparation().unwrap(),
        IntervalReviewPreparation::Unavailable { .. }
    ));
    std::fs::write(&chosen.model_path, vec![b' '; 1_048_577]).unwrap();
    let oversized = HissReducerPlugin::try_from_params(1, params).unwrap();
    assert!(matches!(
        snapshot(&oversized).interval_review_preparation().unwrap(),
        IntervalReviewPreparation::Unavailable { .. }
    ));
}
