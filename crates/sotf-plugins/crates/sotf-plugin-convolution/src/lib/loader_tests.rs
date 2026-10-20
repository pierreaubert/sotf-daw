//! Bounded worker backlog and cancelled-request ownership tests.

// Rust guideline compliant 2026-02-21
use super::*;

#[test]
fn selection_storm_is_bounded_and_only_latest_queued_request_prepares() {
    let (tx, rx) = mpsc::sync_channel(IR_LOAD_QUEUE_CAPACITY);
    let mut plugin = ConvolutionPlugin::new(12, 192_000);
    for selection in 0..IR_LOAD_QUEUE_CAPACITY {
        plugin
            .enqueue_ir_load(format!("ir-{selection}"), &tx)
            .unwrap();
    }
    let generation = plugin.desired_generation;
    let lifetime = Arc::downgrade(plugin.ir_load_lifetime.as_ref().unwrap());
    // No worker is running: capacity refusal must preserve the last queued job.
    for _ in 0..1024 {
        assert!(
            plugin
                .enqueue_ir_load("overflow".into(), &tx)
                .unwrap_err()
                .contains("full")
        );
        assert_eq!(plugin.desired_generation, generation);
        assert_eq!(plugin.load_status(), ConvolutionLoadStatus::Loading);
        assert!(plugin.completion_pending);
        assert_eq!(lifetime.strong_count(), 1);
    }
    let mut prepared = Vec::new();
    for _ in 0..IR_LOAD_QUEUE_CAPACITY {
        rx.try_recv().unwrap().complete(|request| {
            assert_eq!(request.channels, 12);
            assert_eq!(request.sample_rate, 192_000.0);
            assert!(request.use_nupc);
            assert!(!request.true_stereo);
            prepared.push(request.path.clone());
            Err("deterministic preparation failure".into())
        });
    }
    assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
    assert_eq!(prepared, [format!("ir-{}", IR_LOAD_QUEUE_CAPACITY - 1)]);
    let context = ProcessContext::new(192_000, 1);
    let mut output = [0.0; 12];
    sotf_host::assert_no_allocs("bounded storm failure adoption", || {
        plugin.process_in_place(&mut output, &context).unwrap();
    });
    assert!(!plugin.completion_pending);
    assert_eq!(plugin.load_status(), ConvolutionLoadStatus::Failed);
    assert_eq!(plugin.desired_generation, generation);
}

#[test]
fn full_and_disconnected_submission_preserve_a_ready_prior_completion() {
    let (tx, rx) = mpsc::sync_channel(1);
    let mut plugin = ConvolutionPlugin::new(2, 48_000);
    plugin.enqueue_ir_load("prior".into(), &tx).unwrap();
    rx.try_recv()
        .unwrap()
        .complete(|_| Err("prior failure".into()));
    let mut other = ConvolutionPlugin::new(2, 48_000);
    other.enqueue_ir_load("occupy queue".into(), &tx).unwrap();
    let generation = plugin.desired_generation;
    let state = Arc::clone(&plugin.state);
    let token = Arc::downgrade(plugin.ir_load_lifetime.as_ref().unwrap());
    assert!(
        plugin
            .enqueue_ir_load("refused".into(), &tx)
            .unwrap_err()
            .contains("full")
    );
    drop(rx);
    assert!(
        plugin
            .enqueue_ir_load("disconnected".into(), &tx)
            .unwrap_err()
            .contains("unavailable")
    );
    assert_eq!(plugin.desired_generation, generation);
    assert_eq!(token.strong_count(), 1);
    assert!(plugin.completion_pending);
    assert_eq!(plugin.load_status(), ConvolutionLoadStatus::Loading);
    assert!(Arc::ptr_eq(&state, &plugin.state));
    let mut output = [0.0; 2];
    sotf_host::assert_no_allocs("prior completion after refusal", || {
        plugin
            .process_in_place(&mut output, &ProcessContext::new(48_000, 1))
            .unwrap();
    });
    assert_eq!(plugin.load_status(), ConvolutionLoadStatus::Failed);
    assert!(!plugin.completion_pending);
    assert!(Arc::ptr_eq(&state, &plugin.state));
}

#[test]
fn cancellation_during_preparation_discards_result_before_publication() {
    let (tx, rx) = mpsc::sync_channel(2);
    let mut plugin = ConvolutionPlugin::new(1, 48_000);
    plugin.enqueue_ir_load("first".into(), &tx).unwrap();
    let old_rx = plugin.ir_load_result_rx.take().unwrap();
    rx.try_recv().unwrap().complete(|request| {
        assert_eq!(request.path, "first");
        plugin.enqueue_ir_load("replacement".into(), &tx).unwrap();
        Err("cancelled during build".into())
    });
    assert!(matches!(
        old_rx.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));
    let replacement = rx.try_recv().unwrap();
    assert_eq!(replacement.path, "replacement");
    replacement.complete(|_| Err("replacement failure".into()));
    assert_eq!(
        plugin
            .ir_load_result_rx
            .as_ref()
            .unwrap()
            .try_recv()
            .unwrap()
            .generation,
        2
    );
}

#[test]
fn clear_and_plugin_destruction_cancel_queued_preparation() {
    for clear in [true, false] {
        let (tx, rx) = mpsc::sync_channel(1);
        let mut plugin = ConvolutionPlugin::new(1, 48_000);
        plugin.enqueue_ir_load("cancelled".into(), &tx).unwrap();
        if clear {
            plugin.clear_ir_state().unwrap();
            assert_eq!(plugin.load_status(), ConvolutionLoadStatus::Idle);
            assert!(!plugin.completion_pending);
        }
        let request = rx.try_recv().unwrap();
        if clear {
            request.complete(|_| panic!("cleared request must not decode or plan"));
            drop(plugin);
        } else {
            drop(plugin);
            request.complete(|_| panic!("destroyed plugin must not decode or plan"));
        }
    }
}

#[test]
fn synchronous_load_cancels_older_worker_only_after_success() {
    use std::io::Write;
    struct Fixture(std::path::PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let clock = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let fixture = Fixture(std::env::temp_dir().join(format!(
        "sotf-convolution-loader-sync-{}-{clock}.wav",
        std::process::id()
    )));
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&38_u32.to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&48_000_u32.to_le_bytes());
    wav.extend_from_slice(&96_000_u32.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&2_u32.to_le_bytes());
    wav.extend_from_slice(&8192_i16.to_le_bytes());
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&fixture.0)
        .unwrap()
        .write_all(&wav)
        .unwrap();
    let (tx, rx) = mpsc::sync_channel(1);
    let mut plugin = ConvolutionPlugin::new(1, 48_000);
    plugin.enqueue_ir_load("first".into(), &tx).unwrap();
    let token = Arc::downgrade(plugin.ir_load_lifetime.as_ref().unwrap());
    let missing = fixture.0.with_extension("missing");
    assert!(plugin.load_ir(missing.to_str().unwrap()).is_err());
    assert_eq!(token.strong_count(), 1);
    assert!(plugin.completion_pending);
    plugin.load_ir(fixture.0.to_str().unwrap()).unwrap();
    assert_eq!(token.strong_count(), 0);
    assert!(!plugin.completion_pending);
    assert_eq!(plugin.load_status(), ConvolutionLoadStatus::Ready);
    assert_eq!(plugin.max_ir_frames, 1);
    rx.try_recv()
        .unwrap()
        .complete(|_| panic!("synchronous replacement cancelled worker"));
    let state = Arc::clone(&plugin.state);
    let mut input = [0.25; 17];
    sotf_host::assert_no_allocs("synchronous replacement after cancellation", || {
        plugin
            .process_in_place(&mut input, &ProcessContext::new(48_000, 17))
            .unwrap();
    });
    assert!(Arc::ptr_eq(&state, &plugin.state));
    assert_eq!(plugin.load_status(), ConvolutionLoadStatus::Ready);
}

#[test]
fn busy_worker_skips_superseded_backlog_after_releasing_inflight_build() {
    use std::time::Duration;
    let (tx, rx) = mpsc::sync_channel(IR_LOAD_QUEUE_CAPACITY);
    let (started_tx, started_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let mut plugin = ConvolutionPlugin::new(2, 48_000);
    plugin.enqueue_ir_load("inflight".into(), &tx).unwrap();
    let old_rx = plugin.ir_load_result_rx.take().unwrap();
    let worker = std::thread::spawn(move || {
        let mut prepared = Vec::new();
        rx.recv_timeout(Duration::from_secs(10))
            .unwrap()
            .complete(|request| {
                prepared.push(request.path.clone());
                started_tx.send(()).unwrap();
                resume_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                Err("superseded inflight failure".into())
            });
        for _ in 0..IR_LOAD_QUEUE_CAPACITY {
            rx.recv_timeout(Duration::from_secs(10))
                .unwrap()
                .complete(|request| {
                    prepared.push(request.path.clone());
                    Err("latest failure".into())
                });
        }
        prepared
    });
    started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    for index in 0..IR_LOAD_QUEUE_CAPACITY {
        plugin
            .enqueue_ir_load(format!("queued-{index}"), &tx)
            .unwrap();
    }
    assert!(plugin.enqueue_ir_load("overflow".into(), &tx).is_err());
    resume_tx.send(()).unwrap();
    assert_eq!(
        worker.join().unwrap(),
        [
            "inflight".to_string(),
            format!("queued-{}", IR_LOAD_QUEUE_CAPACITY - 1)
        ]
    );
    assert!(matches!(
        old_rx.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));
    let completion = plugin
        .ir_load_result_rx
        .as_ref()
        .unwrap()
        .try_recv()
        .unwrap();
    assert_eq!(completion.generation, plugin.desired_generation);
    assert_eq!(completion.result.err().as_deref(), Some("latest failure"));
}
