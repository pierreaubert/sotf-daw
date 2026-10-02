use serial_test::serial;
use sotf_audio::engine::{
    AudioFrame, OutputAccessMode, PlaybackCommand, PlaybackThread, ProcessingMessage, ThreadEvent,
};
use std::sync::mpsc::{channel, sync_channel};
use std::time::Duration;

fn event_channel() -> (
    crossbeam::channel::Sender<ThreadEvent>,
    crossbeam::channel::Receiver<ThreadEvent>,
) {
    crossbeam::channel::bounded(256)
}

#[sotf_test::requires_hardware]
#[test]
#[serial]
fn test_playback_thread_creation() {
    super::common::skip_without_device!();
    let (_message_tx, message_rx) = channel();
    let (event_tx, _event_rx) = event_channel();

    // Create playback thread with BlackHole device
    let result = PlaybackThread::new(
        message_rx,
        event_tx,
        48000,
        200,
        2,
        1024,
        super::common::blackhole_device_option(),
        sync_channel::<Vec<f32>>(64).0,
        true,
        OutputAccessMode::Shared,
    );

    assert!(
        result.is_ok(),
        "Failed to create playback thread with BlackHole: {:?}",
        result.err()
    );
}

#[sotf_test::requires_hardware]
#[test]
#[serial]
fn test_playback_send_commands() {
    super::common::skip_without_device!();
    let (_message_tx, message_rx) = channel();
    let (event_tx, _event_rx) = event_channel();

    let playback = PlaybackThread::new(
        message_rx,
        event_tx,
        48000,
        200,
        2,
        1024,
        super::common::blackhole_device_option(),
        sync_channel::<Vec<f32>>(64).0,
        true,
        OutputAccessMode::Shared,
    )
    .expect("Failed to create playback thread with BlackHole");

    // Test sending volume command
    assert!(
        playback
            .send_command(PlaybackCommand::SetVolume(0.5))
            .is_ok()
    );

    // Test sending mute command
    assert!(playback.send_command(PlaybackCommand::Mute(true)).is_ok());
    assert!(playback.send_command(PlaybackCommand::Mute(false)).is_ok());

    // Test sending stop command
    assert!(playback.send_command(PlaybackCommand::Stop).is_ok());
}

#[test]
#[serial]
fn test_playback_volume_commands() {
    super::common::skip_without_device!();
    let (_message_tx, message_rx) = channel();
    let (event_tx, _event_rx) = event_channel();

    let playback = PlaybackThread::new(
        message_rx,
        event_tx,
        48000,
        200,
        2,
        1024,
        super::common::blackhole_device_option(),
        sync_channel::<Vec<f32>>(64).0,
        true,
        OutputAccessMode::Shared,
    )
    .expect("Failed to create playback thread with BlackHole");

    // Test various volume levels
    let volumes = [0.0, 0.25, 0.5, 0.75, 1.0, 1.5, 2.0];

    for &vol in &volumes {
        let result = playback.send_command(PlaybackCommand::SetVolume(vol));
        assert!(result.is_ok(), "Failed to set volume to {}", vol);
    }
}

#[sotf_test::requires_hardware]
#[test]
#[serial]
fn test_playback_shutdown() {
    super::common::skip_without_device!();
    let (_message_tx, message_rx) = channel();
    let (event_tx, _event_rx) = event_channel();

    let mut playback = PlaybackThread::new(
        message_rx,
        event_tx,
        48000,
        200,
        2,
        1024,
        super::common::blackhole_device_option(),
        sync_channel::<Vec<f32>>(64).0,
        true,
        OutputAccessMode::Shared,
    )
    .expect("Failed to create playback thread with BlackHole");

    playback.shutdown();

    // After shutdown, commands should fail
    std::thread::sleep(Duration::from_millis(100));

    let result = playback.send_command(PlaybackCommand::SetVolume(0.5));
    assert!(result.is_err(), "Commands should fail after shutdown");
}

/// Whether the session selected the audit ALSA null backend.
///
/// The audit gates set `AEQ_E2E_DEVICE='SOTF Audit Null'`, resolved to the
/// CPAL-visible device of the same exact name by the shared testkit
/// discovery. That backend consumes as fast as the worker produces instead
/// of realtime pacing, so wall-timing assertions about buffered audio
/// cannot apply to it: underrun counts there report genuine instantaneous
/// ring emptiness, not a transport defect. Any other resolved device
/// (paced virtual BlackHole/HAL devices, real hardware) takes the paced
/// branch. Both branches below execute real transport assertions; neither
/// skips.
fn audit_null_selected() -> bool {
    super::common::blackhole_device_option().as_deref() == Some("SOTF Audit Null")
}

#[test]
#[serial]
fn test_playback_receives_frames() {
    super::common::skip_without_device!();
    if audit_null_selected() {
        eprintln!(
            "test_playback_receives_frames: audit null backend selected; proving \
             buffered-frame transport through the ordered event stream (underrun \
             counts are meaningless on an unpaced backend)."
        );
        receives_frames_transport_proof();
    } else {
        eprintln!(
            "test_playback_receives_frames: paced backend selected; proving no \
             underrun across the 200 ms wall window with 24 buffered frames."
        );
        receives_frames_paced_no_underrun();
    }
}

/// Original paced-device behavior, preserved verbatim: 24 silent frames
/// cover 256 ms of realtime consumption, so no underrun may fire inside
/// the 200 ms observation window.
fn receives_frames_paced_no_underrun() {
    let (message_tx, message_rx) = channel();
    let (event_tx, event_rx) = event_channel();

    let _playback = PlaybackThread::new(
        message_rx,
        event_tx,
        48000,
        200,
        2,
        1024,
        super::common::blackhole_device_option(),
        sync_channel::<Vec<f32>>(64).0,
        true,
        OutputAccessMode::Shared,
    )
    .expect("Failed to create playback thread with BlackHole");

    // Send some audio frames
    let frame = AudioFrame::silent(512, 2, 48000);

    // The playback-ready handshake means callbacks are already active here.
    // Queue more than the 200 ms observation window instead of relying on
    // startup silence still being present in the ring buffer.
    for _ in 0..24 {
        message_tx
            .send(ProcessingMessage::Frame(frame.clone()))
            .ok();
    }

    std::thread::sleep(Duration::from_millis(200));

    // Check for underrun events (should not occur with sufficient frames)
    let events: Vec<_> = event_rx.try_iter().collect();
    let underruns = events
        .iter()
        .filter(|e| matches!(e, ThreadEvent::PlaybackUnderrun(_)))
        .count();

    // With more than 200 ms queued, should not underrun immediately.
    assert_eq!(underruns, 0, "Should not underrun with buffered frames");
}

/// Null-backend transport proof for the same 24-silent-frame stimulus.
///
/// End-of-stream is the only deterministic completion signal (periodic
/// stats publish on the five-second cadence), so the stimulus gains an
/// EOS marker and the observation asserts ordered transport facts —
/// exact received/written/dropped counts, live callbacks, a truthful
/// zero meter on silent content, terminal-before-drained ordering and a
/// clean terminal path — instead of wall-timing underrun facts. The
/// nonzero-signal case stays covered by the dedicated terminal test.
fn receives_frames_transport_proof() {
    const BLOCKS: usize = 24;
    let (message_tx, message_rx) = channel();
    let (event_tx, event_rx) = event_channel();
    let constructed_at = std::time::Instant::now();

    let _playback = PlaybackThread::new(
        message_rx,
        event_tx,
        48000,
        200,
        2,
        1024,
        super::common::blackhole_device_option(),
        sync_channel::<Vec<f32>>(64).0,
        true,
        OutputAccessMode::Shared,
    )
    .expect("Failed to create playback thread with BlackHole");

    let frame = AudioFrame::silent(512, 2, 48000);
    for _ in 0..BLOCKS {
        message_tx
            .send(ProcessingMessage::Frame(frame.clone()))
            .ok();
    }
    message_tx.send(ProcessingMessage::EndOfStream).ok();

    let mut events = Vec::new();
    let drained_at = loop {
        match event_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(event) => {
                let drained = matches!(event, ThreadEvent::PlaybackDrained);
                events.push(event);
                if drained {
                    break std::time::Instant::now();
                }
            }
            Err(_) => panic!(
                "drained receipt never arrived; events so far: {events:?}"
            ),
        }
    };
    assert!(
        drained_at.duration_since(constructed_at) < Duration::from_secs(5),
        "transport premise violated: drain took {:?}",
        drained_at.duration_since(constructed_at)
    );
    let drained_index = events
        .iter()
        .position(|event| matches!(event, ThreadEvent::PlaybackDrained))
        .expect("drained receipt must be present");
    let stats_before: Vec<_> = events[..drained_index]
        .iter()
        .filter_map(|event| match event {
            ThreadEvent::PlaybackStats {
                frames_received,
                frames_written,
                frames_dropped,
                callback_count,
                ..
            } => Some((
                *frames_received,
                *frames_written,
                *frames_dropped,
                *callback_count,
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        stats_before.len(),
        1,
        "exactly the terminal snapshot must precede drained: {events:?}"
    );
    let (received, written, dropped, callbacks) = stats_before[0];
    assert_eq!(received, BLOCKS as u64, "terminal received count");
    assert_eq!(written, BLOCKS as u64, "terminal written count");
    assert_eq!(dropped, 0, "terminal dropped count");
    assert!(callbacks > 0, "terminal callback count");
    // Silent content must read an exact zero meter: the meter path is
    // live (an event fired) and truthful (nothing nonzero flowed).
    let meter_before: Vec<_> = events[..drained_index]
        .iter()
        .filter_map(|event| match event {
            ThreadEvent::PlaybackOutputMeter { peak_linear, .. } => Some(*peak_linear),
            _ => None,
        })
        .collect();
    assert!(
        !meter_before.is_empty(),
        "no meter event before drained: {events:?}"
    );
    assert!(
        meter_before.iter().all(|peak| *peak == 0.0),
        "silent content must meter exactly zero: {meter_before:?}"
    );
    assert!(
        !events[..drained_index]
            .iter()
            .any(|event| matches!(event, ThreadEvent::ProcessingError(_))),
        "unexpected terminal error: {events:?}"
    );
}

/// Note: This test is skipped when using virtual audio devices like BlackHole
/// because virtual devices don't have real-time timing constraints and may not
/// trigger underrun events the same way real hardware does.
#[test]
#[serial]
#[ignore = "Underrun detection requires real audio hardware with timing constraints"]
fn test_playback_detects_underrun() {
    super::common::skip_without_device!();
    let (_message_tx, message_rx) = channel();
    let (event_tx, event_rx) = event_channel();

    let _playback = PlaybackThread::new(
        message_rx,
        event_tx,
        48000,
        200,
        2,
        1024,
        super::common::blackhole_device_option(),
        sync_channel::<Vec<f32>>(64).0,
        true,
        OutputAccessMode::Shared,
    )
    .expect("Failed to create playback thread with BlackHole");

    // Don't send any frames - playback should underrun
    std::thread::sleep(Duration::from_millis(1000));

    // Check for events
    let events: Vec<_> = event_rx.try_iter().collect();

    // Check if we got any errors
    if let Some(ThreadEvent::ProcessingError(e)) = events
        .iter()
        .find(|e| matches!(e, ThreadEvent::ProcessingError(_)))
    {
        panic!("Audio thread error during test: {}", e);
    }

    let underruns = events
        .iter()
        .filter(|e| matches!(e, ThreadEvent::PlaybackUnderrun(_)))
        .count();

    // Should detect at least one underrun when no data is provided
    assert!(
        underruns > 0,
        "Should detect underrun when no frames are provided (got 0 events)"
    );
}

#[test]
#[serial]
fn test_playback_handles_eos() {
    super::common::skip_without_device!();
    let (message_tx, message_rx) = channel();
    let (event_tx, _event_rx) = event_channel();

    let _playback = PlaybackThread::new(
        message_rx,
        event_tx,
        48000,
        200,
        2,
        1024,
        super::common::blackhole_device_option(),
        sync_channel::<Vec<f32>>(64).0,
        true,
        OutputAccessMode::Shared,
    )
    .expect("Failed to create playback thread with BlackHole");

    // Send some frames then EOS
    for _ in 0..5 {
        let frame = AudioFrame::silent(512, 2, 48000);
        message_tx.send(ProcessingMessage::Frame(frame)).ok();
    }

    message_tx.send(ProcessingMessage::EndOfStream).ok();

    // Should handle gracefully
    std::thread::sleep(Duration::from_millis(200));
    // If we get here without panic, test passed
}

#[test]
#[serial]
fn test_playback_handles_flush() {
    super::common::skip_without_device!();
    let (message_tx, message_rx) = channel();
    let (event_tx, _event_rx) = event_channel();

    let _playback = PlaybackThread::new(
        message_rx,
        event_tx,
        48000,
        200,
        2,
        1024,
        super::common::blackhole_device_option(),
        sync_channel::<Vec<f32>>(64).0,
        true,
        OutputAccessMode::Shared,
    )
    .expect("Failed to create playback thread with BlackHole");

    // Send frames, then flush
    for _ in 0..10 {
        let frame = AudioFrame::silent(512, 2, 48000);
        message_tx.send(ProcessingMessage::Frame(frame)).ok();
    }

    message_tx.send(ProcessingMessage::Flush).ok();

    std::thread::sleep(Duration::from_millis(100));

    // Send more frames after flush
    for _ in 0..5 {
        let frame = AudioFrame::silent(512, 2, 48000);
        message_tx.send(ProcessingMessage::Frame(frame)).ok();
    }

    std::thread::sleep(Duration::from_millis(200));
    // Should handle flush without panic
}

#[test]
#[serial]
fn test_playback_channel_update() {
    super::common::skip_without_device!();
    let (_message_tx, message_rx) = channel();
    let (event_tx, _event_rx) = event_channel();

    let playback = PlaybackThread::new(
        message_rx,
        event_tx,
        48000,
        200,
        2,
        1024,
        super::common::blackhole_device_option(),
        sync_channel::<Vec<f32>>(64).0,
        true,
        OutputAccessMode::Shared,
    )
    .expect("Failed to create playback thread with BlackHole");

    // Try updating channel count
    let result = playback.send_command(PlaybackCommand::UpdateChannels(5));

    // Command should be accepted (even if hardware doesn't support it)
    assert!(result.is_ok(), "Should accept channel update command");

    std::thread::sleep(Duration::from_millis(100));
}

#[test]
#[serial]
fn test_playback_rapid_volume_changes() {
    super::common::skip_without_device!();
    let (_message_tx, message_rx) = channel();
    let (event_tx, _event_rx) = event_channel();

    let playback = PlaybackThread::new(
        message_rx,
        event_tx,
        48000,
        200,
        2,
        1024,
        super::common::blackhole_device_option(),
        sync_channel::<Vec<f32>>(64).0,
        true,
        OutputAccessMode::Shared,
    )
    .expect("Failed to create playback thread with BlackHole");

    // Rapidly change volume
    for i in 0..100 {
        let vol = (i as f32 / 100.0).sin().abs();
        playback.send_command(PlaybackCommand::SetVolume(vol)).ok();
    }

    std::thread::sleep(Duration::from_millis(100));
    // Should handle rapid changes without issue
}

#[test]
#[serial]
fn test_playback_rapid_mute_toggle() {
    super::common::skip_without_device!();
    let (_message_tx, message_rx) = channel();
    let (event_tx, _event_rx) = event_channel();

    let playback = PlaybackThread::new(
        message_rx,
        event_tx,
        48000,
        200,
        2,
        1024,
        super::common::blackhole_device_option(),
        sync_channel::<Vec<f32>>(64).0,
        true,
        OutputAccessMode::Shared,
    )
    .expect("Failed to create playback thread with BlackHole");

    // Rapidly toggle mute
    for i in 0..50 {
        playback
            .send_command(PlaybackCommand::Mute(i % 2 == 0))
            .ok();
    }

    std::thread::sleep(Duration::from_millis(100));
    // Should handle rapid mute toggles
}

#[test]
#[serial]
fn test_playback_mixed_commands() {
    super::common::skip_without_device!();
    let (message_tx, message_rx) = channel();
    let (event_tx, _event_rx) = event_channel();

    let playback = PlaybackThread::new(
        message_rx,
        event_tx,
        48000,
        200,
        2,
        1024,
        super::common::blackhole_device_option(),
        sync_channel::<Vec<f32>>(64).0,
        true,
        OutputAccessMode::Shared,
    )
    .expect("Failed to create playback thread with BlackHole");

    // Mix audio frames and commands
    for i in 0..20 {
        if i % 3 == 0 {
            let vol = i as f32 / 20.0;
            playback.send_command(PlaybackCommand::SetVolume(vol)).ok();
        } else if i % 3 == 1 {
            playback
                .send_command(PlaybackCommand::Mute(i % 2 == 0))
                .ok();
        } else {
            let frame = AudioFrame::silent(512, 2, 48000);
            message_tx.send(ProcessingMessage::Frame(frame)).ok();
        }

        std::thread::sleep(Duration::from_millis(10));
    }

    std::thread::sleep(Duration::from_millis(100));
}

#[sotf_test::requires_hardware]
#[test]
#[serial]
fn test_playback_different_sample_rates() {
    super::common::skip_without_device!();
    // Ensure BlackHole is available before running tests
    let _device = super::common::require_blackhole_device();

    let sample_rates = [44100, 48000, 88200, 96000];

    for &sr in &sample_rates {
        let (_message_tx, message_rx) = channel();
        let (event_tx, _event_rx) = event_channel();

        let result = PlaybackThread::new(
            message_rx,
            event_tx,
            sr,
            200,
            2,
            1024,
            super::common::blackhole_device_option(),
            sync_channel::<Vec<f32>>(64).0,
            true,
            OutputAccessMode::Shared,
        );

        match result {
            Ok(_) => {
                // Success - BlackHole supports this rate
            }
            Err(e) => {
                // May fail if BlackHole doesn't support this rate
                assert!(
                    e.contains("audio")
                        || e.contains("device")
                        || e.contains("rate")
                        || e.contains("host"),
                    "Error should be audio-related for {} Hz: {}",
                    sr,
                    e
                );
            }
        }
    }
}

#[test]
#[serial]
fn test_playback_different_channel_counts() {
    super::common::skip_without_device!();
    // Ensure BlackHole is available before running tests
    let _device = super::common::require_blackhole_device();

    let channel_counts = [1, 2, 4, 5, 6, 8];

    for &channels in &channel_counts {
        let (_message_tx, message_rx) = channel();
        let (event_tx, _event_rx) = event_channel();

        let result = PlaybackThread::new(
            message_rx,
            event_tx,
            48000,
            200,
            channels,
            1024,
            super::common::blackhole_device_option(),
            sync_channel::<Vec<f32>>(64).0,
            true,
            OutputAccessMode::Shared,
        );

        match result {
            Ok(_) => {
                // Success - BlackHole supports this channel count
            }
            Err(e) => {
                // May fail if BlackHole doesn't support this channel count
                assert!(
                    e.contains("audio")
                        || e.contains("device")
                        || e.contains("channel")
                        || e.contains("host"),
                    "Error should be audio-related for {} channels: {}",
                    channels,
                    e
                );
            }
        }
    }
}

#[test]
#[serial]
fn test_playback_drop_cleanup() {
    super::common::skip_without_device!();
    let (_message_tx, message_rx) = channel();
    let (event_tx, _event_rx) = event_channel();

    let playback = PlaybackThread::new(
        message_rx,
        event_tx,
        48000,
        200,
        2,
        1024,
        super::common::blackhole_device_option(),
        sync_channel::<Vec<f32>>(64).0,
        true,
        OutputAccessMode::Shared,
    )
    .expect("Failed to create playback thread with BlackHole");

    // Let Drop handle cleanup
    drop(playback);

    std::thread::sleep(Duration::from_millis(100));
    // Should clean up without panic
}

#[test]
#[serial]
fn test_playback_terminal_stats_precede_drained_for_short_stream() {
    const BLOCKS: usize = 10;
    super::common::skip_without_device!();
    // In the audit environment `AEQ_E2E_DEVICE` resolves the named null
    // backend through the shared virtual-device discovery, so this test
    // executes there instead of skipping.
    let device = super::common::blackhole_device_option();
    let constructed_at = std::time::Instant::now();
    let (message_tx, message_rx) = channel();
    let (event_tx, event_rx) = event_channel();

    let _playback = PlaybackThread::new(
        message_rx,
        event_tx,
        48000,
        200,
        2,
        1024,
        device,
        sync_channel::<Vec<f32>>(64).0,
        true,
        OutputAccessMode::Shared,
    )
    .expect("Failed to create playback thread");

    // Ten nonzero blocks then EOS: ~107 ms of audio, far shorter than the
    // five-second diagnostics interval, so no periodic snapshot can fire.
    for block in 0..BLOCKS {
        let mut data = vec![0.0; 512 * 2];
        for frame in 0..512 {
            let time = (block * 512 + frame) as f32 / 48000.0;
            let sample = (time * 440.0 * std::f32::consts::TAU).sin() * 0.5;
            data[frame * 2] = sample;
            data[frame * 2 + 1] = sample;
        }
        let frame = AudioFrame::try_new(data, 512, 2, 48000).expect("frame must build");
        message_tx.send(ProcessingMessage::Frame(frame)).ok();
    }
    message_tx.send(ProcessingMessage::EndOfStream).ok();

    // Collect the ordered event stream until the drained receipt arrives.
    let mut events = Vec::new();
    let drained_at = loop {
        match event_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(event) => {
                let drained = matches!(event, ThreadEvent::PlaybackDrained);
                events.push(event);
                if drained {
                    break std::time::Instant::now();
                }
            }
            Err(_) => panic!(
                "drained receipt never arrived; events so far: {events:?}"
            ),
        }
    };
    // Premise guard: with construction-to-drained under five seconds, every
    // stats event in the stream must be the terminal snapshot, never a
    // periodic one.
    assert!(
        drained_at.duration_since(constructed_at) < Duration::from_secs(5),
        "short-stream premise violated: drain took {:?}",
        drained_at.duration_since(constructed_at)
    );
    let drained_index = events
        .iter()
        .position(|event| matches!(event, ThreadEvent::PlaybackDrained))
        .expect("drained receipt must be present");
    let stats_before: Vec<_> = events[..drained_index]
        .iter()
        .filter_map(|event| match event {
            ThreadEvent::PlaybackStats {
                frames_received,
                frames_written,
                frames_dropped,
                callback_count,
                ..
            } => Some((
                *frames_received,
                *frames_written,
                *frames_dropped,
                *callback_count,
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        stats_before.len(),
        1,
        "exactly the terminal snapshot must precede drained: {events:?}"
    );
    let (received, written, dropped, callbacks) = stats_before[0];
    assert_eq!(received, BLOCKS as u64, "terminal received count");
    assert_eq!(written, BLOCKS as u64, "terminal written count");
    assert_eq!(dropped, 0, "terminal dropped count");
    assert!(callbacks > 0, "terminal callback count");
    // Residual nonzero evidence flows before the drained receipt.
    assert!(
        events[..drained_index].iter().any(|event| matches!(
            event,
            ThreadEvent::PlaybackOutputMeter { peak_linear, .. } if *peak_linear > 0.1
        )),
        "no nonzero meter before drained: {events:?}"
    );
    // The terminal path is clean: no stall or invariant errors.
    assert!(
        !events[..drained_index]
            .iter()
            .any(|event| matches!(event, ThreadEvent::ProcessingError(_))),
        "unexpected terminal error: {events:?}"
    );
}
