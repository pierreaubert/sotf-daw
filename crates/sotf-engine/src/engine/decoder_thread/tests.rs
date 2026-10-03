use super::super::{DecoderCommand, DecoderMessage, DecoderResponse, ThreadEvent};
use super::consts::send_or_interrupt;
use super::decoder_state::DecoderState;
use super::hal_input_guard_trip::guard_hal_input_block;
#[cfg(any(test, all(target_os = "macos", feature = "hal")))]
use super::misc::frames_to_sample_count;
use super::sample_queue::SampleQueue;
use super::types::run_decoder_thread;
use crate::DsdOutputMode;
use crate::decoder::{AudioSpec, DecodedAudio};

use std::path::PathBuf;
use std::time::Duration;

#[test]
fn decoder_scratch_and_frame_pool_prepare_sixty_four_channels() {
    let (_recycle_tx, recycle_rx) = std::sync::mpsc::sync_channel(1);
    let mut state = DecoderState::new(recycle_rx, DsdOutputMode::Disabled);
    let samples = crate::EngineConfig::MAX_FRAME_SIZE * crate::EngineConfig::MAX_INPUT_CHANNELS;

    assert!(state.resampler_buffer.data.capacity() >= samples * 2);
    assert!(state.resample_output_buffer.capacity() >= samples * 2);
    assert!(state.resample_staging.data.capacity() >= samples * 4);
    assert!(state.chunk_buffer.capacity() >= samples);
    assert!(state.frame_send_buffer.capacity() >= samples);
    assert_eq!(state.frame_buffer_pool.len(), 8);
    assert!(
        state
            .frame_buffer_pool
            .iter()
            .all(|buffer| buffer.capacity() >= samples)
    );

    let mut scratch = [
        &mut state.resampler_buffer.data,
        &mut state.resample_output_buffer,
        &mut state.resample_staging.data,
        &mut state.chunk_buffer,
        &mut state.frame_send_buffer,
    ];
    let mut original_ptrs = Vec::with_capacity(scratch.len());
    for (index, buffer) in scratch.iter_mut().enumerate() {
        let required = match index {
            0 | 1 => samples * 2,
            2 => samples * 4,
            _ => samples,
        };
        original_ptrs.push(buffer.as_ptr());
        DecoderState::ensure_buffer_len(buffer, required).unwrap();
    }
    for (buffer, original_ptr) in scratch.iter().zip(original_ptrs) {
        assert_eq!(buffer.as_ptr(), original_ptr);
    }

    for buffer in &mut state.frame_buffer_pool {
        let original_ptr = buffer.as_ptr();
        DecoderState::ensure_buffer_len(buffer, samples).unwrap();
        assert_eq!(buffer.as_ptr(), original_ptr);
    }
}

#[test]
fn decoder_shutdown_does_not_block_on_a_stuck_worker() {
    let (command_tx, _command_rx) = std::sync::mpsc::channel();
    let (_response_tx, response_rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(|| std::thread::sleep(Duration::from_secs(1)));
    let (_async_error_tx, async_error_rx) = std::sync::mpsc::channel();
    let mut decoder = super::DecoderThread {
        command_tx,
        response_inbox: std::sync::Mutex::new(super::DecoderResponseInbox {
            rx: response_rx,
            pending: std::collections::VecDeque::new(),
            buffered: std::collections::HashMap::new(),
            abandoned: std::collections::HashSet::new(),
        }),
        next_request_id: std::sync::atomic::AtomicU64::new(1),
        thread_handle: Some(handle),
        async_error_rx,
        exit_status: crate::engine::worker_death::WorkerExitStatus::new(),
    };

    let started = std::time::Instant::now();
    decoder.shutdown_with_timeout(Duration::from_millis(20));

    assert!(started.elapsed() < Duration::from_millis(250));
    assert!(decoder.thread_handle.is_none());
}

#[test]
fn decoder_late_reply_cannot_acknowledge_a_newer_request() {
    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let (response_tx, response_rx) = std::sync::mpsc::channel();
    let (_async_error_tx, async_error_rx) = std::sync::mpsc::channel();
    let mut decoder = super::DecoderThread {
        command_tx,
        response_inbox: std::sync::Mutex::new(super::DecoderResponseInbox {
            rx: response_rx,
            pending: std::collections::VecDeque::new(),
            buffered: std::collections::HashMap::new(),
            abandoned: std::collections::HashSet::new(),
        }),
        next_request_id: std::sync::atomic::AtomicU64::new(1),
        thread_handle: None,
        async_error_rx,
        exit_status: crate::engine::worker_death::WorkerExitStatus::new(),
    };

    let stale_id = decoder.send_command(DecoderCommand::Pause).unwrap();
    let live_id = decoder.send_command(DecoderCommand::Resume).unwrap();
    assert!(matches!(command_rx.recv().unwrap(), DecoderCommand::Pause));
    assert!(matches!(command_rx.recv().unwrap(), DecoderCommand::Resume));
    decoder.abandon_request(stale_id);
    response_tx
        .send(super::super::DecoderResponse::Error("late".into()))
        .unwrap();
    response_tx.send(super::super::DecoderResponse::Ok).unwrap();

    assert!(matches!(
        decoder.try_recv_response_for(live_id),
        Some(super::super::DecoderResponse::Ok)
    ));
    decoder.thread_handle = None;
}

#[test]
fn send_or_interrupt_returns_unsent_message_with_interrupting_command() {
    let (message_tx, message_rx) = std::sync::mpsc::sync_channel(1);
    message_tx.send(DecoderMessage::EndOfStream).unwrap();

    let (command_tx, command_rx) = std::sync::mpsc::channel();
    command_tx
        .send(DecoderCommand::QueueNext(PathBuf::from("next.wav").into()))
        .unwrap();

    let result = send_or_interrupt(&message_tx, &command_rx, DecoderMessage::Flush).unwrap();
    let Some((DecoderCommand::QueueNext(_), pending)) = result else {
        panic!("expected QueueNext command with the unsent message");
    };

    assert!(matches!(pending, DecoderMessage::Flush));
    assert!(matches!(
        message_rx.try_recv().unwrap(),
        DecoderMessage::EndOfStream
    ));
}

#[test]
fn send_or_interrupt_errors_when_queue_stays_full() {
    let (message_tx, _message_rx) = std::sync::mpsc::sync_channel(1);
    message_tx.send(DecoderMessage::EndOfStream).unwrap();

    let (_command_tx, command_rx) = std::sync::mpsc::channel();

    let result = send_or_interrupt(&message_tx, &command_rx, DecoderMessage::Flush);

    assert!(result.unwrap_err().contains("queue stuck"));
}

#[test]
fn silent_source_fallback_send_is_interruptible_when_queue_full() {
    let (message_tx, message_rx) = std::sync::mpsc::sync_channel(1);
    message_tx.send(DecoderMessage::Flush).unwrap();
    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let (response_tx, _response_rx) = std::sync::mpsc::channel();
    command_tx.send(DecoderCommand::Stop).unwrap();

    let (_recycle_tx, recycle_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut state = DecoderState::new(recycle_rx, DsdOutputMode::Disabled);
        state.start_silent_source(2);
        #[cfg(all(target_os = "macos", feature = "hal"))]
        {
            state.hal_reader = None;
        }
        let result = state.process_hal_input(&message_tx, &command_rx, &response_tx, 16, 48_000);
        done_tx.send(result).ok();
    });

    let result = done_rx.recv_timeout(std::time::Duration::from_millis(250));
    drop(message_rx);
    let result = result.expect("silent-source send blocked instead of honoring Stop command");
    let (_sent, cmd) = result.expect("silent-source processing failed");
    assert!(matches!(cmd, Some(DecoderCommand::Stop)));
}

#[test]
fn decoder_frame_buffer_handoff_has_no_allocation_fallback() {
    let source = include_str!("../decoder_thread.rs");
    assert!(
        !source.contains(concat!("Err(_) => Vec::", "with_capacity(len)")),
        "decoder frame handoff must use preallocated/recycled buffers instead of allocating on recycle miss"
    );
}

#[test]
fn exact_decoder_block_transfers_ownership_without_sample_copies() {
    let (_recycle_tx, recycle_rx) = std::sync::mpsc::channel();
    let mut state = DecoderState::new(recycle_rx, DsdOutputMode::Disabled);
    let mut decoded = DecodedAudio::new(AudioSpec {
        sample_rate: 48_000,
        channels: 2,
        bits_per_sample: 32,
        total_frames: None,
    });
    decoded.samples = vec![0.25; 2_048];
    let decoded_allocation = decoded.samples.as_ptr();
    state.decode_buffer = Some(decoded);

    assert!(state.queue_decoded_samples().unwrap());
    assert_eq!(state.resampler_buffer.data.as_ptr(), decoded_allocation);
    let frame_data = state
        .take_exact_decoded_block(2_048)
        .expect("exact decoded block should transfer directly");
    assert_eq!(frame_data.as_ptr(), decoded_allocation);
    assert!(state.resampler_buffer.is_empty());
}

#[test]
fn ensure_buffer_len_errors_instead_of_growing_capacity() {
    let mut buffer = Vec::with_capacity(4);

    DecoderState::ensure_buffer_len(&mut buffer, 4).unwrap();
    assert_eq!(buffer.len(), 4);
    assert_eq!(buffer.capacity(), 4);

    let err = DecoderState::ensure_buffer_len(&mut buffer, 5).unwrap_err();
    assert!(err.contains("too small"));
    assert_eq!(buffer.capacity(), 4);
}

#[test]
fn hal_input_reloads_stale_cipher_before_reading() {
    let source = include_str!("decoder_state.rs");
    let reload_call = source
        .find("!self.reload_hal_cipher_if_needed()")
        .expect("HAL input path should refresh stale encryption ciphers");
    let read_call = source
        .find("let frames_read = reader.read(&mut self.hal_input_buffer)")
        .expect("HAL input path should read from the shared-memory reader");

    assert!(
        source.contains("reader.needs_cipher_reload()")
            && source.contains("reader.reload_cipher()"),
        "decoder should detect and reload stale HAL input ciphers"
    );
    assert!(
        reload_call < read_call,
        "decoder must reload a stale cipher before reading, otherwise key rotation yields silence"
    );
}

#[test]
fn sample_queue_consume_advances_cursor_without_front_memmove() {
    let mut queue = SampleQueue::new();
    queue.extend_from_slice(&[0.0, 1.0, 2.0, 3.0]);

    let original_ptr = queue.as_slice().as_ptr();
    queue.consume(2);

    assert_eq!(queue.as_slice(), &[2.0, 3.0]);
    assert_eq!(queue.as_slice().as_ptr(), unsafe { original_ptr.add(2) });
}

#[test]
fn hal_read_frame_count_converts_to_sample_count() {
    let frame_size = 1024;
    let channels = 2;
    let buffer_len = frame_size * channels;

    assert_eq!(
        frames_to_sample_count(frame_size, channels, buffer_len),
        buffer_len
    );
    assert_eq!(frames_to_sample_count(192, channels, buffer_len), 384);
    assert_eq!(
        frames_to_sample_count(frame_size + 1, channels, buffer_len),
        buffer_len
    );
}

#[test]
fn hal_input_guard_allows_normal_float_pcm() {
    let mut samples = vec![-1.25, -0.5, 0.0, 0.5, 1.25, 2.0];

    let trip = guard_hal_input_block(&mut samples);

    assert_eq!(trip, None);
    assert_eq!(samples, vec![-1.25, -0.5, 0.0, 0.5, 1.25, 2.0]);
}

#[test]
fn hal_input_guard_silences_impossible_peak() {
    let mut samples = vec![0.1, -0.2, 36.4, 0.3];

    let trip = guard_hal_input_block(&mut samples).expect("guard should trip");

    assert_eq!(trip.invalid_samples, 0);
    assert_eq!(trip.over_limit_samples, 1);
    assert_eq!(trip.peak, 36.4);
    assert!(samples.iter().all(|&sample| sample == 0.0));
}

#[test]
fn hal_input_guard_silences_non_finite_samples() {
    let mut samples = vec![0.1, f32::NAN, f32::INFINITY, -0.2];

    let trip = guard_hal_input_block(&mut samples).expect("guard should trip");

    assert_eq!(trip.invalid_samples, 2);
    assert_eq!(trip.over_limit_samples, 0);
    assert_eq!(trip.peak, 0.2);
    assert!(samples.iter().all(|&sample| sample == 0.0));
}

#[test]
fn silent_source_fallback_uses_configured_channel_count() {
    let (_recycle_tx, recycle_rx) = std::sync::mpsc::channel();
    let mut state = DecoderState::new(recycle_rx, DsdOutputMode::Disabled);
    state.start_silent_source(6);
    #[cfg(all(target_os = "macos", feature = "hal"))]
    {
        state.hal_reader = None;
    }

    let (message_tx, message_rx) = std::sync::mpsc::sync_channel(1);
    let (_command_tx, command_rx) = std::sync::mpsc::channel();
    let (response_tx, _response_rx) = std::sync::mpsc::channel();

    let (processed, pending) = state
        .process_hal_input(&message_tx, &command_rx, &response_tx, 128, 48_000)
        .unwrap();

    assert!(processed);
    assert!(pending.is_none());
    let DecoderMessage::Frame(frame) = message_rx.try_recv().unwrap() else {
        panic!("expected silent audio frame");
    };
    assert_eq!(frame.num_frames, 128);
    assert_eq!(frame.num_channels, 6);
    assert_eq!(frame.data.len(), 128 * 6);
    assert!(frame.data.iter().all(|&sample| sample == 0.0));
}

/// Regression test for the decoder thread blocking on `recv()` when inactive.
///
/// When the thread is paused and the command sender is dropped, it must exit
/// promptly instead of waiting forever for the next command.
#[test]
fn paused_decoder_thread_exits_on_disconnected_sender() {
    let (message_tx, _message_rx) = std::sync::mpsc::sync_channel(4);
    let (event_tx, _event_rx) = crossbeam::channel::bounded(32);
    let (response_tx, response_rx) = std::sync::mpsc::channel();
    let (recycle_tx, recycle_rx) = std::sync::mpsc::channel();

    // Keep the recycle sender alive so the decoder thread does not exit for
    // an unrelated reason.
    let _recycle_guard = recycle_tx;

    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let (async_error_tx, _async_error_rx) = std::sync::mpsc::channel();

    let handle = std::thread::Builder::new()
        .name("decoder-disconnect-test".into())
        .spawn(move || {
            run_decoder_thread(
                message_tx,
                command_rx,
                response_tx,
                event_tx,
                48_000,
                1024,
                recycle_rx,
                DsdOutputMode::Disabled,
                async_error_tx,
            )
        })
        .expect("spawn decoder thread");

    // Pause the thread so its next command wait is on the inactive path.
    command_tx.send(DecoderCommand::Pause).unwrap();
    let pause_response = response_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("pause command should be acknowledged");
    assert!(matches!(pause_response, super::super::DecoderResponse::Ok));

    // Drop the only command sender.  The inactive decoder thread must notice
    // the disconnect and exit instead of blocking on recv() indefinitely.
    drop(command_tx);

    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = handle.join();
        done_tx.send(()).ok();
    });

    done_rx
        .recv_timeout(Duration::from_millis(250))
        .expect("paused decoder thread should exit after command sender is dropped");
}

#[test]
fn play_sends_flush_before_ack() {
    let (temp, _mono) = sotf_testkit::audio::temp_sine_wav(0.5, 48_000, 2, 440.0).unwrap();
    let (message_tx, message_rx) = std::sync::mpsc::sync_channel(4);
    let (event_tx, _event_rx) = crossbeam::channel::bounded(32);
    let (response_tx, response_rx) = std::sync::mpsc::channel();
    let (recycle_tx, recycle_rx) = std::sync::mpsc::channel();
    let _recycle_guard = recycle_tx;
    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let (async_error_tx, _async_error_rx) = std::sync::mpsc::channel();

    let handle = std::thread::Builder::new()
        .name("decoder-flush-order-test".into())
        .spawn(move || {
            run_decoder_thread(
                message_tx,
                command_rx,
                response_tx,
                event_tx,
                48_000,
                1024,
                recycle_rx,
                DsdOutputMode::Disabled,
                async_error_tx,
            )
        })
        .expect("spawn decoder thread");

    command_tx
        .send(DecoderCommand::Play(
            crate::AudioSource::File(temp.path().to_path_buf()),
            1,
        ))
        .unwrap();
    // Order pin: the FIRST message is Flush; the ack follows it.
    assert!(matches!(
        message_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        DecoderMessage::Flush
    ));
    assert!(matches!(
        response_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        DecoderResponse::Ok
    ));

    // Settle: stop the now-active decoder, draining async frames until the
    // barrier Flush (FIFO ⇒ everything before it predates the Stop).
    command_tx.send(DecoderCommand::Stop).unwrap();
    assert!(matches!(
        response_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        DecoderResponse::Ok
    ));
    loop {
        match message_rx.recv_timeout(Duration::from_secs(1)).unwrap() {
            DecoderMessage::Flush => break,
            DecoderMessage::Frame(_) => continue,
            other => panic!("unexpected message before barrier Flush: {other:?}"),
        }
    }

    command_tx.send(DecoderCommand::Shutdown).unwrap();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = handle.join();
        done_tx.send(()).ok();
    });
    done_rx
        .recv_timeout(Duration::from_millis(250))
        .expect("decoder thread should exit after Shutdown");
}

#[test]
#[serial_test::serial]
fn playat_seek_failure_reverts_to_silence() {
    use crate::decoder::service_resolver::{
        ResolvedServiceStream, clear_service_stream_resolver, set_service_stream_resolver,
    };
    use std::io::Cursor;
    use std::sync::Arc;

    // Process-global resolver: the guard restores None even on panic.
    // #[serial] (not folding: this thread-harness test lives cross-module
    // from the folding test, where run_decoder_thread is invisible)
    // excludes the other resolver test in decoder/core.rs.
    struct ResolverGuard;
    impl Drop for ResolverGuard {
        fn drop(&mut self) {
            clear_service_stream_resolver();
        }
    }
    let _resolver_guard = ResolverGuard;

    // Service PCM seeks always fail (production behavior): load succeeds,
    // seek fails deterministically with no files, network, or timing.
    let pcm_samples: Vec<f32> = vec![0.5, -0.5, 0.25, -0.25];
    let pcm_bytes: Vec<u8> = pcm_samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    set_service_stream_resolver(Arc::new(move |service, track_id| {
        assert_eq!(service, crate::ServiceId::Spotify);
        assert_eq!(track_id, "b5-track");
        Ok(ResolvedServiceStream::Pcm {
            sample_rate: 48_000,
            channels: 2,
            bits_per_sample: 32,
            total_frames: Some(2),
            reader: Box::new(Cursor::new(pcm_bytes.clone())),
        })
    }));

    let (message_tx, message_rx) = std::sync::mpsc::sync_channel(4);
    let (event_tx, event_rx) = crossbeam::channel::bounded(32);
    let (response_tx, response_rx) = std::sync::mpsc::channel();
    let (recycle_tx, recycle_rx) = std::sync::mpsc::channel();
    let _recycle_guard = recycle_tx;
    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let (async_error_tx, _async_error_rx) = std::sync::mpsc::channel();

    let handle = std::thread::Builder::new()
        .name("decoder-b5-revert-test".into())
        .spawn(move || {
            run_decoder_thread(
                message_tx,
                command_rx,
                response_tx,
                event_tx,
                48_000,
                1024,
                recycle_rx,
                DsdOutputMode::Disabled,
                async_error_tx,
            )
        })
        .expect("spawn decoder thread");

    let source = crate::AudioSource::ServiceStream {
        service: crate::ServiceId::Spotify,
        track_id: "b5-track".to_string(),
    };
    command_tx
        .send(DecoderCommand::PlayAt(source, 1.0, 1))
        .unwrap();

    // Explicit failure, ordered: Flush precedes the Error ack (same arm),
    // and the DecoderError event carries the seek cause.
    assert!(matches!(
        message_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        DecoderMessage::Flush
    ));
    assert!(matches!(
        response_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        DecoderResponse::Error(message) if message == "Failed to seek during play_at"
    ));
    assert!(matches!(
        event_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        ThreadEvent::DecoderError(message) if message.contains("Seeking not supported")
    ));

    // Silence proof, FIFO-causal (no timing): nothing may appear between
    // this arm's Flush and the barrier Stop's Flush.
    assert!(message_rx.try_recv().is_err());
    command_tx.send(DecoderCommand::Stop).unwrap();
    assert!(matches!(
        response_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        DecoderResponse::Ok
    ));
    assert!(matches!(
        message_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        DecoderMessage::Flush
    ));
    assert!(message_rx.try_recv().is_err());

    // Recovery: the thread is not wedged; a real file plays afterwards.
    let (temp, _mono) = sotf_testkit::audio::temp_sine_wav(0.5, 48_000, 2, 440.0).unwrap();
    command_tx
        .send(DecoderCommand::Play(
            crate::AudioSource::File(temp.path().to_path_buf()),
            2,
        ))
        .unwrap();
    assert!(matches!(
        message_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        DecoderMessage::Flush
    ));
    assert!(matches!(
        response_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        DecoderResponse::Ok
    ));
    assert!(matches!(
        message_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        DecoderMessage::Frame(_)
    ));

    command_tx.send(DecoderCommand::Shutdown).unwrap();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = handle.join();
        done_tx.send(()).ok();
    });
    done_rx
        .recv_timeout(Duration::from_millis(250))
        .expect("decoder thread should exit after Shutdown");
}

/// Regression test for the upmixer silence bug with cross-rate resampling.
///
/// Root cause: the rubato resampler produces a variable number of output frames
/// per input chunk (e.g. ~940 frames for 48 kHz → 44.1 kHz with a 1024-frame
/// input block).  If those sub-`frame_size` blocks were forwarded directly to
/// the processing thread, plugins whose hop size equals `frame_size` (like the
/// upmixer at hop = 1024) would never accumulate enough input to fire an FFT
/// block, producing complete silence.
///
/// The fix is a `resample_staging` buffer that accumulates resampled output and
/// only emits complete `frame_size`-frame blocks.  This test verifies the
/// staging logic in isolation: when fed chunks smaller than `frame_size` the
/// staging buffer must hold them, and when the accumulated total reaches
/// `frame_size` it must emit exactly one full block.
#[test]
fn test_resample_staging_emits_full_frame_size_blocks() {
    let frame_size: usize = 1024;
    let channels: usize = 2;
    let send_chunk_len = frame_size * channels;

    // Simulate the resampler producing ~940 frames per 1024-frame input
    // (48 kHz → 44.1 kHz ratio ≈ 0.91875).
    let resampled_chunk_frames: usize = 940;
    let resampled_chunk_len = resampled_chunk_frames * channels;

    let mut staging = SampleQueue::new();
    let mut emitted_blocks: usize = 0;

    // Feed several resampled chunks and count how many full blocks are emitted.
    // Two chunks of 940 = 1880 samples → one complete 1024-frame block with
    // 856 frames left in staging.
    for chunk_idx in 0..4 {
        // Each sample is tagged with chunk index for easy debugging.
        let chunk: Vec<f32> = (0..resampled_chunk_len)
            .map(|i| (chunk_idx * 1000 + i) as f32)
            .collect();
        staging.extend_from_slice(&chunk);

        while staging.len() >= send_chunk_len {
            staging.consume(send_chunk_len);
            emitted_blocks += 1;
        }
    }

    // 4 chunks × 1880 samples = 7520 samples.
    // 7520 / 2048 = 3 full blocks (6144 samples), remainder 1376 samples = 688 frames.
    assert_eq!(
        emitted_blocks, 3,
        "Expected 3 full blocks after 4 × 940-frame chunks"
    );
    let expected_remainder = (4 * resampled_chunk_len) - (3 * send_chunk_len);
    assert_eq!(
        staging.len(),
        expected_remainder,
        "Staging buffer should hold the partial remainder (688 frames)"
    );
    assert!(
        staging.len() < send_chunk_len,
        "Remainder must be less than one full block"
    );

    // Feed one more chunk: 1376 + 1880 = 3256 → 1 more full block.
    let chunk: Vec<f32> = vec![0.0; resampled_chunk_len];
    staging.extend_from_slice(&chunk);
    while staging.len() >= send_chunk_len {
        staging.consume(send_chunk_len);
        emitted_blocks += 1;
    }
    assert_eq!(
        emitted_blocks, 4,
        "Expected a fourth full block after the fifth chunk"
    );
}

#[test]
fn mixed_stream_commands_emit_exactly_one_flush_each() {
    let (temp, _mono) = sotf_testkit::audio::temp_sine_wav(0.5, 48_000, 2, 440.0).unwrap();
    let (message_tx, message_rx) = std::sync::mpsc::sync_channel(4);
    let (event_tx, _event_rx) = crossbeam::channel::bounded(32);
    let (response_tx, response_rx) = std::sync::mpsc::channel();
    let (recycle_tx, recycle_rx) = std::sync::mpsc::channel();
    let _recycle_guard = recycle_tx;
    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let (async_error_tx, _async_error_rx) = std::sync::mpsc::channel();

    let handle = std::thread::Builder::new()
        .name("decoder-flush-count-test".into())
        .spawn(move || {
            run_decoder_thread(
                message_tx,
                command_rx,
                response_tx,
                event_tx,
                48_000,
                1024,
                recycle_rx,
                DsdOutputMode::Disabled,
                async_error_tx,
            )
        })
        .expect("spawn decoder thread");

    // Drain concurrently so no arm ever blocks; the count is exact
    // because Shutdown ends the stream (disconnect totality).
    let flush_count = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let drainer_count = std::sync::Arc::clone(&flush_count);
    let drainer = std::thread::spawn(move || {
        loop {
            match message_rx.recv() {
                Ok(DecoderMessage::Flush) => {
                    drainer_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    });

    let file = || crate::AudioSource::File(temp.path().to_path_buf());
    let expect_ok = |what: &str| {
        assert!(
            matches!(
                response_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
                DecoderResponse::Ok
            ),
            "{what} must ack Ok"
        );
    };

    command_tx.send(DecoderCommand::Play(file(), 1)).unwrap();
    expect_ok("Play");
    command_tx.send(DecoderCommand::Pause).unwrap();
    expect_ok("Pause");
    command_tx.send(DecoderCommand::QueueNext(file())).unwrap();
    expect_ok("QueueNext");
    command_tx.send(DecoderCommand::CancelNext).unwrap();
    expect_ok("CancelNext");
    command_tx.send(DecoderCommand::Seek(0.1)).unwrap();
    // Either outcome Flushes first (the arm sends before attempting),
    // so the count holds whichever path executes: Ok when the Pause
    // won the race against EOF, "No decoder" when EOF auto-stopped.
    match response_rx.recv_timeout(Duration::from_secs(1)).unwrap() {
        DecoderResponse::Ok => {}
        DecoderResponse::Error(e) if e == "No decoder" => {}
        other => panic!("Seek must ack Ok or No-decoder, got {other:?}"),
    }
    command_tx.send(DecoderCommand::Stop).unwrap();
    expect_ok("Stop");
    // No acknowledgment by design; FIFO order still applies.
    command_tx
        .send(DecoderCommand::StartSilentSource(2))
        .unwrap();
    command_tx.send(DecoderCommand::Play(file(), 2)).unwrap();
    expect_ok("Play");
    command_tx.send(DecoderCommand::Stop).unwrap();
    expect_ok("Stop");

    command_tx.send(DecoderCommand::Shutdown).unwrap();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = handle.join();
        done_tx.send(()).ok();
    });
    done_rx
        .recv_timeout(Duration::from_millis(250))
        .expect("decoder thread should exit after Shutdown");
    drainer.join().unwrap();

    // Five stream commands, five Flushes; Pause, QueueNext,
    // CancelNext, and StartSilentSource contribute zero.
    assert_eq!(flush_count.load(std::sync::atomic::Ordering::SeqCst), 5);
}

#[test]
fn async_decode_error_reports_adopted_tag_and_replay_recovers() {
    // A5: backpressure the decoder's output queue totally (never
    // drain it): the frame send trips queue-stuck, the loop reports
    // ONE tagged async error — the ADOPTED tag, proving the arm
    // adopted the carried identity — and stops the phase. Draining
    // + re-Play then recovers on the live thread under a new tag:
    // error-stop is not death. Barriers, not sleeps: channel
    // receipts order every step (the stuck trip itself takes
    // production time; the deadline fails loud, it never sleeps).
    let (temp, _mono) = sotf_testkit::audio::temp_sine_wav(2.0, 48_000, 2, 440.0).unwrap();
    let (message_tx, message_rx) = std::sync::mpsc::sync_channel(4);
    let (event_tx, _event_rx) = crossbeam::channel::bounded(32);
    let (response_tx, response_rx) = std::sync::mpsc::channel();
    let (recycle_tx, recycle_rx) = std::sync::mpsc::channel();
    let _recycle_guard = recycle_tx;
    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let (async_error_tx, async_error_rx) = std::sync::mpsc::channel();

    let handle = std::thread::Builder::new()
        .name("decoder-async-error-test".into())
        .spawn(move || {
            run_decoder_thread(
                message_tx,
                command_rx,
                response_tx,
                event_tx,
                48_000,
                1024,
                recycle_rx,
                DsdOutputMode::Disabled,
                async_error_tx,
            )
        })
        .expect("spawn decoder thread");

    let file = || crate::AudioSource::File(temp.path().to_path_buf());
    command_tx.send(DecoderCommand::Play(file(), 7)).unwrap();
    assert!(matches!(
        response_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        DecoderResponse::Ok
    ));
    // Barrier 1: the tagged async error (queue-stuck under total
    // backpressure — nobody drains message_rx).
    let error = async_error_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("queue-stuck must report exactly once");
    assert_eq!(error.attempt, 7);
    assert!(
        error.message.contains("queue stuck"),
        "unexpected error text: {}",
        error.message
    );
    // Exactly one report: the phase stopped with its error.
    assert!(async_error_rx.try_recv().is_err());
    // Recovery on the live thread: drain, re-Play under a new tag,
    // frames flow again — then Stop (acked barrier: no frame send
    // can follow it, so the no-error assert below is deterministic,
    // not a race with a second stuck trip).
    while message_rx.try_recv().is_ok() {}
    command_tx.send(DecoderCommand::Play(file(), 8)).unwrap();
    assert!(matches!(
        response_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        DecoderResponse::Ok
    ));
    assert!(matches!(
        message_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        DecoderMessage::Flush
    ));
    assert!(matches!(
        message_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        DecoderMessage::Frame(_)
    ));
    command_tx.send(DecoderCommand::Stop).unwrap();
    assert!(matches!(
        response_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        DecoderResponse::Ok
    ));
    assert!(async_error_rx.try_recv().is_err());

    command_tx.send(DecoderCommand::Shutdown).unwrap();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = handle.join();
        done_tx.send(()).ok();
    });
    done_rx
        .recv_timeout(Duration::from_millis(250))
        .expect("decoder thread should exit after Shutdown");
}
