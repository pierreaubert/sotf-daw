//! Live worker regression checks with explicit source positions and heap guards.
#![cfg(feature = "interval-proposals")]

// Rust guideline compliant 2026-02-21
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{
    CountingAlloc, ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext,
};
use sotf_plugin_hiss_reducer::interval_proposals::{IntervalContext, IntervalError, IntervalModel};
use sotf_plugin_hiss_reducer::interval_worker::IntervalObservation;
use sotf_plugin_hiss_reducer::profile::NoiseProfileData;
use sotf_plugin_hiss_reducer::{HissReducerPlugin, HissReducerPluginParams};
use std::time::{Duration, Instant};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;
const MODEL: &[u8] = include_bytes!("../../../models/hiss-intervals/model.synthetic.json");
const SHA: &str = "bae4c80e241f85e98edda3d017bc48a964e9496d10cd436b58dad9fe978c32de";
const RATE: usize = 48000;

fn model() -> IntervalModel {
    let value: serde_json::Value = serde_json::from_slice(MODEL).unwrap();
    let context: IntervalContext = serde_json::from_value(value["context"].clone()).unwrap();
    IntervalModel::from_bytes(MODEL, SHA, &context).unwrap()
}

fn noise(frames: usize) -> Vec<f32> {
    let mut state = 20791026_u32;
    (0..frames)
        .map(|_| {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            ((f64::from(state) / 4294967296.0 - 0.5) * 0.02) as f32
        })
        .collect()
}

fn plugin(spectral: bool) -> HissReducerPlugin {
    let mut p = HissReducerPlugin::from_params(
        1,
        HissReducerPluginParams {
            spectral_mode: spectral,
            use_captured_profile: true,
            captured_profile: Some(NoiseProfileData {
                format_version: 1,
                sample_rate: 48000.0,
                channels: 1,
                measurement_cutoff_hz: 4000.0,
                floor_db_per_channel: vec![-45.0],
                frames_analyzed: RATE as u64,
                spectral: None,
            }),
            ..HissReducerPluginParams::default()
        },
    );
    p.initialize(48000.0).unwrap();
    p
}

fn process(p: &mut HissReducerPlugin, buffer: &mut [f32], position: u64) {
    let context = ProcessContext::new(48000.0, buffer.len()).with_sample_position(position);
    assert_no_allocs_or_deallocs("Hiss interval review callback", || {
        p.process_in_place(buffer, &context).unwrap();
        let _ = p.interval_review_status();
    });
}

fn wait(p: &HissReducerPlugin) -> IntervalObservation {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let mut found = None;
        assert_no_allocs_or_deallocs("Hiss review result read", || {
            found = p.interval_review_status().unwrap().observation;
        });
        if let Some(value) = found {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "worker did not publish a complete interval"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn worker_matches_direct_model_and_preserves_audio_profiles_and_partitions() {
    let source = noise(RATE + 137);
    let direct = model();
    let expected = direct.evaluate(&source[..RATE], direct.context()).unwrap();
    for spectral in [false, true] {
        for pattern in [&[17, 193, 4096][..], &[3001, 1, 127][..]] {
            let mut baseline = plugin(spectral);
            let mut reviewed = plugin(spectral);
            let before = serde_json::to_value(reviewed.persisted_params()).unwrap();
            reviewed
                .configure_interval_review(Some(model()), RATE as u64)
                .unwrap();
            let mut a = source.clone();
            let mut b = source.clone();
            let mut offset = 0;
            let mut block = 0;
            while offset < a.len() {
                let end = (offset + pattern[block % pattern.len()]).min(a.len());
                process(&mut baseline, &mut a[offset..end], 911 + offset as u64);
                process(&mut reviewed, &mut b[offset..end], 911 + offset as u64);
                offset = end;
                block += 1;
            }
            assert_eq!(a, b);
            let result = wait(&reviewed);
            assert_eq!(
                (result.start_frame, result.end_frame),
                (911, 911 + RATE as u64)
            );
            assert_eq!(result.scores.unwrap(), expected);
            assert_eq!(result.model_sha256, direct.sha256());
            use sha2::{Digest, Sha256};
            let bytes: Vec<_> = source[..RATE]
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect();
            let source_digest: [u8; 32] = Sha256::digest(&bytes).into();
            assert_eq!(result.source_sha256, source_digest);
            let status = reviewed.interval_review_status().unwrap();
            assert_eq!(status.submitted_intervals, 1);
            assert_eq!(status.partial_frames, 137);
            assert_eq!(status.prepared_audio_bytes, 3 * RATE * size_of::<f32>());
            assert_eq!(status.dropped_intervals, 0);
            assert!(!reviewed.is_capturing());
            assert_eq!(
                serde_json::to_value(reviewed.persisted_params()).unwrap(),
                before
            );
        }
    }
}

#[test]
fn explicit_review_applies_exact_original_interval_and_matches_native_capture() {
    let original = noise(RATE);
    for spectral in [false, true] {
        let mut manual = plugin(spectral);
        let trigger = ParameterId::from("learn_noise");
        manual
            .parametric_set_parameter(trigger.clone(), ParameterValue::Bool(true))
            .unwrap();
        let mut direct_audio = original.clone();
        process(&mut manual, &mut direct_audio, 500);
        let expected = manual.persisted_params().captured_profile.unwrap();

        let mut reviewed = plugin(spectral);
        let before = serde_json::to_value(reviewed.persisted_params()).unwrap();
        reviewed
            .configure_interval_review(Some(model()), RATE as u64)
            .unwrap();
        let mut rendered = original.clone();
        process(&mut reviewed, &mut rendered, 500);
        let observation = wait(&reviewed);
        assert!(observation.scores.unwrap().review_candidate);
        assert_eq!(
            serde_json::to_value(reviewed.persisted_params()).unwrap(),
            before
        );

        let mut changed = original.clone();
        changed[0] += 0.001;
        assert!(
            reviewed
                .apply_reviewed_interval_profile(observation, &changed)
                .is_err()
        );
        assert!(
            reviewed
                .apply_reviewed_interval_profile(observation, &original[..RATE - 1])
                .is_err()
        );
        let altered_token = IntervalObservation {
            start_frame: 501,
            ..observation
        };
        assert!(
            reviewed
                .apply_reviewed_interval_profile(altered_token, &original)
                .is_err()
        );
        reviewed
            .parametric_set_parameter(trigger.clone(), ParameterValue::Bool(true))
            .unwrap();
        assert!(
            reviewed
                .apply_reviewed_interval_profile(observation, &original)
                .is_err()
        );
        reviewed
            .parametric_set_parameter(trigger.clone(), ParameterValue::Bool(false))
            .unwrap();
        assert!(!reviewed.is_capturing());
        assert_eq!(
            serde_json::to_value(reviewed.persisted_params()).unwrap(),
            before
        );

        let applied = reviewed
            .apply_reviewed_interval_profile(observation, &original)
            .unwrap();
        assert_eq!(
            serde_json::to_value(&applied).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
        assert_eq!(
            serde_json::to_value(reviewed.persisted_params().captured_profile.unwrap()).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
        assert!(reviewed.persisted_params().use_captured_profile);
        assert!(!reviewed.is_capturing());
        assert!(reviewed.interval_review_status().unwrap().epoch > observation.epoch);
        assert!(
            reviewed
                .interval_review_status()
                .unwrap()
                .observation
                .is_none()
        );
        assert!(
            reviewed
                .apply_reviewed_interval_profile(observation, &original)
                .is_err()
        );
    }
}

#[test]
fn incomplete_windows_age_and_transport_discontinuity_abstain() {
    let mut p = plugin(false);
    p.configure_interval_review(Some(model()), 0).unwrap();
    let input = noise(RATE);
    let mut prefix = input[..RATE - 1].to_vec();
    process(&mut p, &mut prefix, 500);
    let state = p.interval_review_status().unwrap();
    assert_eq!(state.submitted_intervals, 0);
    assert!(state.observation.is_none());
    let mut final_sample = [input[RATE - 1]];
    process(&mut p, &mut final_sample, 500 + RATE as u64 - 1);
    let result = wait(&p);
    assert!(result.scores.unwrap().review_candidate);
    let mut sample = [0.01];
    process(&mut p, &mut sample, 500 + RATE as u64);
    assert!(p.interval_review_status().unwrap().observation.is_none());
    let old_epoch = p.interval_review_status().unwrap().epoch;
    assert!(p.apply_reviewed_interval_profile(result, &input).is_err());
    process(&mut p, &mut sample, 100);
    let state = p.interval_review_status().unwrap();
    assert!(state.epoch > old_epoch);
    assert_eq!(state.partial_frames, 1);
    assert!(state.observation.is_none());
    // A source clock overflow must not publish a future or wrapped interval.
    process(&mut p, &mut sample, u64::MAX);
    assert!(p.interval_review_status().unwrap().observation.is_none());
}

#[test]
fn configuration_reset_and_context_changes_preserve_stored_profile() {
    let mut p = plugin(false);
    assert!(p.interval_review_status().is_none());
    p.configure_interval_review(Some(model()), RATE as u64)
        .unwrap();
    let before = serde_json::to_value(p.persisted_params()).unwrap();
    let mut partial = noise(12345);
    process(&mut p, &mut partial, 0);
    let original_epoch = p.interval_review_status().unwrap().epoch;
    assert!(
        p.configure_interval_review(Some(model()), 10 * RATE as u64 + 1)
            .is_err()
    );
    assert_eq!(p.interval_review_status().unwrap().epoch, original_epoch);
    assert_no_allocs_or_deallocs("Hiss review reset", || p.reset());
    assert!(p.interval_review_status().unwrap().epoch > original_epoch);
    assert_eq!(p.interval_review_status().unwrap().partial_frames, 0);
    assert_eq!(serde_json::to_value(p.persisted_params()).unwrap(), before);
    let cutoff = ParameterId::from("frequency_hz");
    assert_no_allocs_or_deallocs("Hiss review cutoff invalidation", || {
        p.parametric_set_parameter(cutoff.clone(), ParameterValue::Float(6000.0))
            .unwrap();
    });
    assert!(!p.interval_review_status().unwrap().context_matches);
    assert!(
        p.configure_interval_review(Some(model()), RATE as u64)
            .is_err()
    );
    let mut values = p.current_values();
    values.insert(cutoff, ParameterValue::Float(4000.0));
    p.apply_values(values).unwrap();
    assert!(p.interval_review_status().unwrap().context_matches);
    p.initialize(96000.0).unwrap();
    assert!(!p.interval_review_status().unwrap().context_matches);
    p.initialize(48000.0).unwrap();
    assert!(p.interval_review_status().unwrap().context_matches);
    let mut full = noise(RATE);
    process(&mut p, &mut full, 10000);
    let observation = wait(&p);
    assert!(observation.epoch > original_epoch);
    let last_epoch = p.interval_review_status().unwrap().epoch;
    p.configure_interval_review(None, 0).unwrap();
    assert!(p.interval_review_status().is_none());
    p.configure_interval_review(Some(model()), RATE as u64)
        .unwrap();
    assert!(p.interval_review_status().unwrap().epoch > last_epoch);
}

#[test]
fn silent_input_reports_abstention_and_does_not_create_a_profile() {
    let mut p = HissReducerPlugin::new(1);
    assert!(p.configure_interval_review(Some(model()), 0).is_err());
    p.initialize(48000.0).unwrap();
    p.configure_interval_review(Some(model()), 0).unwrap();
    let mut silent = vec![0.0; RATE];
    process(&mut p, &mut silent, 0);
    let rejected = wait(&p);
    assert_eq!(rejected.scores, Err(IntervalError::Excitation));
    assert!(
        p.apply_reviewed_interval_profile(rejected, &silent)
            .is_err()
    );
    assert!(!p.has_captured_profile());
    assert!(!p.is_capturing());
    assert!(silent.iter().all(|value| *value == 0.0));
}

#[test]
fn eof_drain_keeps_native_tail_and_does_not_complete_partial_interval() {
    for spectral in [false, true] {
        let mut baseline = plugin(spectral);
        let mut reviewed = plugin(spectral);
        reviewed
            .configure_interval_review(Some(model()), RATE as u64)
            .unwrap();
        let mut a = noise(RATE - 3);
        let mut b = a.clone();
        process(&mut baseline, &mut a, 77);
        process(&mut reviewed, &mut b, 77);
        assert_eq!(a, b);
        assert_eq!(baseline.latency_samples(), reviewed.latency_samples());
        let status = reviewed.interval_review_status().unwrap();
        assert_eq!(status.submitted_intervals, 0);
        let capacity = reviewed.drain_output_frames_max().max(1);
        let mut tail_a = vec![0.0; capacity];
        let mut tail_b = tail_a.clone();
        let bound = reviewed.drain_call_bound().unwrap().get();
        let context =
            ProcessContext::new(48000.0, capacity).with_sample_position(77 + (RATE - 3) as u64);
        let mut complete = false;
        for _ in 0..bound {
            let mut result_a = None;
            let mut result_b = None;
            assert_no_allocs_or_deallocs("review-enabled native EOF drain", || {
                result_a = Some(baseline.drain(&mut tail_a, &context).unwrap());
                result_b = Some(reviewed.drain(&mut tail_b, &context).unwrap());
            });
            let x = result_a.unwrap();
            let y = result_b.unwrap();
            assert_eq!((x.frames, x.complete), (y.frames, y.complete));
            assert_eq!(&tail_a[..x.frames], &tail_b[..y.frames]);
            if y.complete {
                complete = true;
                break;
            }
        }
        assert!(complete);
        let after = reviewed.interval_review_status().unwrap();
        assert_eq!(after.source_end_frame, status.source_end_frame);
        assert_eq!(after.partial_frames, status.partial_frames);
        assert_eq!(after.submitted_intervals, 0);
        assert!(after.observation.is_none());
    }
}
