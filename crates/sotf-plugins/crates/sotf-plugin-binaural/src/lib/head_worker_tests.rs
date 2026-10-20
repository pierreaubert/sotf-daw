//! Deterministic worker handoff tests; computation is gated with rendezvous channels.

use crate::{
    BinauralDecoderPlugin,
    head_tracking::{HeadRequests, run_head_worker},
    state::HrtfState,
    types::BinauralState,
};
use rustfft::num_complex::Complex;
use sotf_host::{Plugin, ProcessContext};
use std::sync::{
    Arc,
    mpsc::{SyncSender, sync_channel},
};
use std::time::Duration;

fn filters(value: f32) -> Arc<BinauralState> {
    Arc::new(BinauralState {
        hrtf_filters_freq: vec![vec![Complex::new(value, 0.0); 66]; 2],
        diffuse_field_eq_filter: None,
        _hrtf_data: None,
    })
}

fn publish(requests: &HeadRequests, tx: &SyncSender<(f32, f32, f32)>, angle: f32) {
    requests.publish((angle, -angle, angle + 1000.0));
    match tx.try_send((angle, -angle, angle + 1000.0)) {
        Ok(()) | Err(std::sync::mpsc::TrySendError::Full(_)) => {}
        Err(error) => panic!("worker disconnected: {error}"),
    }
}

#[test]
fn busy_worker_coalesces_queued_poses_and_keeps_completed_motion_progress() {
    let requests = HeadRequests::new();
    let initial = filters(0.0);
    let state = HrtfState::new(initial);
    let (tx, rx) = sync_channel(1);
    let (started, ready) = sync_channel(0);
    let (go, resume) = sync_channel(0);
    let worker_requests = Arc::clone(&requests);
    let worker_state = Arc::clone(&state);
    let handle = std::thread::spawn(move || {
        run_head_worker(rx, worker_requests, worker_state, |pose| {
            started.send(pose).unwrap();
            resume.recv().unwrap();
            Ok(filters(pose.angles.0))
        })
    });
    publish(&requests, &tx, 1.0);
    let first = ready.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(first.angles.0, 1.0);
    for angle in 2..=100 {
        publish(&requests, &tx, angle as f32);
    }
    go.send(()).unwrap();
    let second = ready.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(
        second.angles.0, 100.0,
        "queued older poses were not coalesced"
    );
    assert_eq!(
        state.load().hrtf_filters_freq[0][0].re,
        1.0,
        "motion starved an in-flight result"
    );
    for angle in 101..=200 {
        publish(&requests, &tx, angle as f32);
    }
    go.send(()).unwrap();
    let third = ready.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(third.angles.0, 200.0);
    assert_eq!(state.load().hrtf_filters_freq[0][0].re, 100.0);
    go.send(()).unwrap();
    drop(tx);
    handle.join().unwrap();
    assert_eq!(state.load().hrtf_filters_freq[0][0].re, 200.0);
}

#[test]
fn reset_and_shutdown_cancel_in_flight_resource_epochs() {
    for stop in [false, true] {
        let requests = HeadRequests::new();
        let initial = filters(0.0);
        let state = HrtfState::new(Arc::clone(&initial));
        let (tx, rx) = sync_channel(1);
        let (started, ready) = sync_channel(0);
        let (go, resume) = sync_channel(0);
        let worker_requests = Arc::clone(&requests);
        let worker_state = Arc::clone(&state);
        let handle = std::thread::spawn(move || {
            run_head_worker(rx, worker_requests, worker_state, |pose| {
                started.send(pose).unwrap();
                resume.recv().unwrap();
                Ok(filters(pose.angles.0))
            })
        });
        publish(&requests, &tx, 1.0);
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        if stop {
            requests.stop();
        } else {
            requests.invalidate();
        }
        publish(&requests, &tx, 2.0);
        go.send(()).unwrap();
        if stop {
            drop(tx);
            handle.join().unwrap();
            assert!(Arc::ptr_eq(&state.load(), &initial));
        } else {
            let next = ready.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(next.angles.0, 2.0);
            assert!(
                Arc::ptr_eq(&state.load(), &initial),
                "prior epoch became audible"
            );
            go.send(()).unwrap();
            drop(tx);
            handle.join().unwrap();
            assert_eq!(state.load().hrtf_filters_freq[0][0].re, 2.0);
        }
    }
}

#[test]
fn older_completed_results_cannot_overwrite_newer_publications() {
    let requests = HeadRequests::new();
    let state = HrtfState::new(filters(0.0));
    requests.publish((1.0, 0.0, 0.0));
    let old = requests.snapshot().unwrap();
    requests.publish((2.0, 0.0, 0.0));
    let new = requests.snapshot().unwrap();
    assert!(state.store_head(filters(2.0), &requests, new));
    assert!(!state.store_head(filters(1.0), &requests, old));
    assert_eq!(state.load().hrtf_filters_freq[0][0].re, 2.0);
    requests.invalidate();
    assert!(!state.store_head(filters(3.0), &requests, new));
}

fn decoder() -> BinauralDecoderPlugin {
    let params =
        serde_json::from_value(serde_json::json!({"input_channels":2,"fft_size":64})).unwrap();
    let mut plugin = BinauralDecoderPlugin::try_from_params(params).unwrap();
    plugin.initialize(48_000.0).unwrap();
    plugin
}

#[test]
fn audio_adoption_rejects_old_epochs_and_eof_keeps_audible_filter_owners() {
    let mut tested = decoder();
    let mut baseline = decoder();
    let audible = Arc::clone(&tested.crossfade.current_state_snapshot);
    tested.head_requests.start();
    tested.head_requests.publish((10.0, 0.0, 0.0));
    let pose = tested.head_requests.snapshot().unwrap();
    assert!(
        tested
            .state
            .store_head(filters(3.0), &tested.head_requests, pose)
    );
    tested.head_requests.invalidate();
    tested.process_audio_block();
    assert!(Arc::ptr_eq(
        &tested.crossfade.current_state_snapshot,
        &audible
    ));
    tested.reset();
    baseline.reset();
    for p in [&mut tested, &mut baseline] {
        p.process(
            &[0.25; 40],
            &mut [0.0; 40],
            &ProcessContext::new(48_000, 20),
        )
        .unwrap();
    }
    let context = ProcessContext::new(48_000, 0);
    let mut a = [0.0; 34];
    let mut b = [0.0; 34];
    let x = tested.drain(&mut a[..2], &context).unwrap();
    let y = baseline.drain(&mut b[..2], &context).unwrap();
    assert_eq!((x.frames, x.complete), (y.frames, y.complete));
    assert_eq!(a[..2], b[..2]);
    tested.state.store(filters(7.0));
    loop {
        let x = tested.drain(&mut a, &context).unwrap();
        let y = baseline.drain(&mut b, &context).unwrap();
        assert_eq!((x.frames, x.complete), (y.frames, y.complete));
        assert_eq!(a[..x.frames * 2], b[..y.frames * 2]);
        if x.complete {
            break;
        }
    }
    assert!(Arc::ptr_eq(
        &tested.crossfade.current_state_snapshot,
        &audible
    ));
}

#[test]
fn concurrent_mailbox_snapshots_never_mix_pose_coordinates() {
    let requests = HeadRequests::new();
    let reader = Arc::clone(&requests);
    let (started, ready) = sync_channel(0);
    let handle = std::thread::spawn(move || {
        let mut reads = 0;
        started.send(()).unwrap();
        while !reader.stopped() {
            if let Some(pose) = reader.snapshot() {
                assert_eq!(pose.angles.1, -pose.angles.0);
                assert_eq!(pose.angles.2, pose.angles.0 + 1000.0);
                reads += 1;
            }
        }
        reads
    });
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    for value in 1..=100_000 {
        requests.publish((value as f32, -(value as f32), value as f32 + 1000.0));
        if value & 1023 == 0 {
            std::thread::yield_now();
        }
    }
    requests.stop();
    let reads = handle.join().unwrap();
    assert!(reads > 0, "concurrent reader did not observe any pose");
    eprintln!("HEAD_MAILBOX published=100000 coherent_snapshots={reads}");
}
