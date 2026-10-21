//! FIFO wrap and trusted transport ownership regressions.

// Rust guideline compliant 2026-02-21
use super::*;

fn assert_conserved(plugin: &HalOutputPlugin) {
    let t = plugin.telemetry();
    assert_eq!(
        t.requested_frames,
        t.written_frames
            .saturating_add(t.dropped_frames)
            .saturating_add(t.queued_frames as u64),
        "{t:?}"
    );
}

fn wrapped_queue(
    channels: usize,
    scheduled: Vec<usize>,
) -> (
    HalOutputPlugin,
    Arc<std::sync::Mutex<FakeWriterState>>,
    Vec<f32>,
) {
    let storage = VecDeque::<f32>::with_capacity(64);
    let frames = storage.capacity() / channels;
    let (mut p, state) = make_plugin_with_writer(
        channels,
        (48_000, channels as u32, frames as u32),
        scheduled,
    );
    p.initialize(48_000.0).unwrap();
    p.pending = storage;
    p.pending_capacity_samples = frames * channels;
    let first: Vec<f32> = (0..frames * channels).map(|i| i as f32).collect();
    p.process(&first, &mut [], &ProcessContext::new(48_000, frames))
        .unwrap();
    let next: Vec<f32> = (first.len()..first.len() + channels)
        .map(|i| i as f32)
        .collect();
    p.process(&next, &mut [], &ProcessContext::new(48_000, 1))
        .unwrap();
    assert_conserved(&p);
    assert_eq!(p.pending.len(), channels * 2);
    assert!(!p.pending.as_slices().1.is_empty());
    let mut all = first;
    all.extend(next);
    (p, state, all)
}

#[test]
fn process_flushes_frames_crossing_physical_fifo_wrap() {
    let mut split_layouts = 0;
    for channels in 1..=16 {
        let frames = VecDeque::<f32>::with_capacity(64).capacity() / channels;
        let (mut p, state, mut all) = wrapped_queue(channels, vec![0, frames - 1, 1, 1, 1]);
        if !p.pending.as_slices().0.len().is_multiple_of(channels) {
            split_layouts += 1;
        }
        let next: Vec<f32> = (all.len()..all.len() + channels)
            .map(|i| i as f32)
            .collect();
        p.process(&next, &mut [], &ProcessContext::new(48_000, 1))
            .unwrap();
        all.extend(next);
        let t = p.telemetry();
        println!(
            "process_wrap channels={channels} queued={} written={} requested={}",
            t.queued_frames, t.written_frames, t.requested_frames
        );
        assert_eq!(
            t.queued_frames, 0,
            "writer accepted all complete frames; queue must progress"
        );
        assert_conserved(&p);
        let accepted = state.lock().unwrap().accepted_samples.clone();
        assert_eq!(accepted, all);
    }
    assert!(split_layouts > 0);
}

#[test]
fn failed_second_write_keeps_committed_prefix_and_refuses_new_input_ownership() {
    let channels = 2;
    let frames = VecDeque::<f32>::with_capacity(64).capacity() / channels;
    let (mut p, _, all) = wrapped_queue(channels, vec![0, frames - 1, 1, 2]);
    let requested = p.telemetry().requested_frames;
    let next: Vec<f32> = (all.len()..all.len() + channels)
        .map(|i| i as f32)
        .collect();
    assert!(
        p.process(&next, &mut [], &ProcessContext::new(48_000, 1))
            .is_err()
    );
    let t = p.telemetry();
    println!(
        "partial_write_error requested={} written={} queued={}",
        t.requested_frames, t.written_frames, t.queued_frames
    );
    assert_eq!(
        t.written_frames, frames as u64,
        "trusted prefix was committed before the later error"
    );
    assert_eq!(
        t.requested_frames, requested,
        "Err must not claim ownership of new input"
    );
    assert_eq!(t.queued_frames, 1);
    assert_conserved(&p);
    assert_eq!(t.state, HalOutputTransportState::FormatError);
}

#[test]
fn missing_or_overreporting_writer_does_not_own_a_failed_input_request() {
    for missing in [false, true] {
        let (mut p, _) = make_plugin_with_writer(2, (48_000, 2, 8), vec![2]);
        p.initialize(48_000.0).unwrap();
        if missing {
            p.writer = None;
        }
        assert!(
            p.process(&[0.25, -0.25], &mut [], &ProcessContext::new(48_000, 1))
                .is_err()
        );
        let t = p.telemetry();
        println!(
            "writer_failure missing={missing} requested={} written={} queued={}",
            t.requested_frames, t.written_frames, t.queued_frames
        );
        assert_eq!(t.requested_frames, 0);
        assert_conserved(&p);
    }
}

#[test]
fn wrapped_process_service_and_reset_use_only_prepared_storage() {
    let mut layouts = 0;
    for channels in 1..=16 {
        let frames = VecDeque::<f32>::with_capacity(64).capacity() / channels;
        let (mut p, state, all) = wrapped_queue(channels, vec![0, frames - 1, 1, 1, 1]);
        {
            let mut state = state.lock().unwrap();
            state.capture_audio = false;
            state.capture_accepted_samples = false;
        }
        let next: Vec<f32> = (all.len()..all.len() + channels)
            .map(|i| i as f32)
            .collect();
        let pending_capacity = p.pending.capacity();
        let staging_capacity = p.drain_staging.capacity();
        assert_no_heap_activity("wrapped HAL output process", || {
            p.process(&next, &mut [], &ProcessContext::new(48_000, 1))
                .unwrap();
        });
        assert_eq!(p.queue_state().pending_frames, 0);
        assert_no_heap_activity("empty HAL output service/reset", || {
            p.service_pending(&ProcessContext::new(48_000, 0)).unwrap();
            p.reset();
        });
        assert_eq!(p.pending.capacity(), pending_capacity);
        assert_eq!(p.drain_staging.capacity(), staging_capacity);
        assert_conserved(&p);
        layouts += 1;
    }
    println!("process_wrap_heap layouts={layouts} allocations=0 frees=0 retained_capacity=true");
}

#[test]
fn conservation_and_fifo_order_hold_across_transport_lifecycle_sequences() {
    let mut events = 0;
    let mut process_calls = 0;
    let mut drain_calls = 0;
    let mut seed = 0x12345678_u32;
    let mut random = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as usize
    };
    for rate in [44_100, 96_000] {
        for channels in 1..=16 {
            for ring_frames in [1, 2, 3, 8, 17] {
                let (mut p, state) = make_plugin_with_writer(
                    channels,
                    (rate, channels as u32, ring_frames as u32),
                    vec![],
                );
                p.initialize(f64::from(rate)).unwrap();
                let mut queue = VecDeque::<f32>::new();
                let mut requested = 0_u64;
                let mut written = 0_u64;
                let mut dropped = 0_u64;
                let mut tag = 0;
                for step in 0..256 {
                    let accepted_before = state.lock().unwrap().accepted_samples.len();
                    match step % 19 {
                        3 => {
                            {
                                let mut s = state.lock().unwrap();
                                s.connected = true;
                                s.key_ready = true;
                                s.config_changed = true;
                            }
                            let expected = p.prepared_transport_format().unwrap();
                            assert_eq!(
                                p.recover_transport(expected).unwrap(),
                                SinkTransportRecoveryStatus::Ready
                            );
                        }
                        4 => {
                            dropped += (queue.len() / channels) as u64;
                            queue.clear();
                            p.reset();
                        }
                        5 => {
                            p.initialize(f64::from(rate)).unwrap();
                            dropped += (queue.len() / channels) as u64;
                            queue.clear();
                        }
                        6 => {
                            state.lock().unwrap().transport_format =
                                Some((rate + 1, channels as u32, ring_frames as u32));
                            assert!(p.initialize(f64::from(rate)).is_err());
                            state.lock().unwrap().transport_format = None;
                        }
                        _ => {
                            {
                                let mut s = state.lock().unwrap();
                                match step % 19 {
                                    0 => {
                                        s.connected = false;
                                    }
                                    1 => {
                                        s.connected = true;
                                        s.key_ready = false;
                                    }
                                    2 => {
                                        s.connected = true;
                                        s.key_ready = true;
                                        s.config_changed = true;
                                    }
                                    _ => {}
                                }
                                s.maximum_write_frames = Some(random() % 4);
                            }
                            let draining = step % 7 == 0;
                            let frames = if draining { 0 } else { random() % 13 };
                            let input: Vec<f32> = (0..frames * channels)
                                .map(|_| {
                                    let v = tag as f32;
                                    tag += 1;
                                    v
                                })
                                .collect();
                            let attempts_before = state.lock().unwrap().writer_attempts;
                            if draining {
                                Plugin::drain(&mut p, &mut [], &ProcessContext::new(rate, 0))
                                    .unwrap();
                                drain_calls += 1;
                            } else {
                                assert_eq!(
                                    p.process(&input, &mut [], &ProcessContext::new(rate, frames))
                                        .unwrap(),
                                    frames
                                );
                                requested += frames as u64;
                                process_calls += 1;
                            }
                            let snapshot = state.lock().unwrap();
                            let acknowledged = &snapshot.accepted_samples[accepted_before..];
                            assert_eq!(acknowledged.len() % channels, 0);
                            let attempts = snapshot.writer_attempts - attempts_before;
                            assert!(attempts <= if draining { 2 } else { 3 });
                            // Independent FIFO policy: acknowledgements consume the
                            // oldest samples, then newest overflow is explicitly dropped.
                            queue.extend(input);
                            for &sample in acknowledged {
                                assert_eq!(queue.pop_front(), Some(sample));
                            }
                            written += (acknowledged.len() / channels) as u64;
                            let excess = queue.len().saturating_sub(ring_frames * channels);
                            assert_eq!(excess % channels, 0);
                            dropped += (excess / channels) as u64;
                            queue.truncate(queue.len() - excess);
                        }
                    }
                    let t = p.telemetry();
                    assert_eq!(
                        (t.requested_frames, t.written_frames, t.dropped_frames),
                        (requested, written, dropped),
                        "rate={rate} channels={channels} step={step}"
                    );
                    assert!(p.pending.iter().copied().eq(queue.iter().copied()));
                    assert_eq!(t.queued_frames, queue.len() / channels);
                    assert_eq!(t.queue_capacity_frames, ring_frames);
                    assert_eq!(t.boundary_latency_frames, ring_frames * 2);
                    assert_conserved(&p);
                    events += 1;
                }
            }
        }
    }
    println!(
        "transport_conservation events={events} process_calls={process_calls} drain_calls={drain_calls} layouts=16 rates=2 ring_sizes=5 exact_fifo=true bounded_writer_calls=true"
    );
}

#[test]
fn writer_contract_fault_gates_following_writes_until_control_recovery() {
    let (mut p, state) = make_plugin_with_writer(2, (48_000, 2, 8), vec![2]);
    p.initialize(48_000.0).unwrap();
    assert!(
        p.process(&[1.0, 2.0], &mut [], &ProcessContext::new(48_000, 1))
            .is_err()
    );
    assert_eq!(p.telemetry().state, HalOutputTransportState::FormatError);
    let attempts = state.lock().unwrap().writer_attempts;
    assert_eq!(
        p.process(&[3.0, 4.0], &mut [], &ProcessContext::new(48_000, 1))
            .unwrap(),
        1
    );
    assert!(
        !Plugin::drain(&mut p, &mut [], &ProcessContext::new(48_000, 0))
            .unwrap()
            .complete
    );
    let attempts_after = state.lock().unwrap().writer_attempts;
    assert_eq!(attempts_after, attempts);
    assert_eq!(p.telemetry().requested_frames, 1);
    assert_conserved(&p);
    let expected = p.prepared_transport_format().unwrap();
    assert_eq!(
        p.recover_transport(expected).unwrap(),
        SinkTransportRecoveryStatus::Ready
    );
    assert!(
        Plugin::drain(&mut p, &mut [], &ProcessContext::new(48_000, 0))
            .unwrap()
            .complete
    );
    assert_eq!(p.telemetry().written_frames, 1);
    assert_conserved(&p);
}

#[test]
fn observed_format_change_stays_latched_after_the_shared_flag_is_cleared() {
    let (mut p, state) = make_plugin_with_writer(2, (48_000, 2, 8), vec![]);
    p.initialize(48_000.0).unwrap();
    state.lock().unwrap().config_changed = true;
    p.process(&[1.0, 2.0], &mut [], &ProcessContext::new(48_000, 1))
        .unwrap();
    assert_eq!(
        p.telemetry().state,
        HalOutputTransportState::ConfigurationChanged
    );
    let attempts = state.lock().unwrap().writer_attempts;
    {
        let mut s = state.lock().unwrap();
        // A different transport consumer can clear the shared flag. That is
        // not validation/admission of the output plugin's prepared clock.
        s.config_changed = false;
        s.transport_format = Some((96_000, 2, 8));
    }
    p.process(&[3.0, 4.0], &mut [], &ProcessContext::new(48_000, 1))
        .unwrap();
    let t = p.telemetry();
    println!(
        "format_latch state={:?} writer_attempts={} queued={}",
        t.state,
        state.lock().unwrap().writer_attempts,
        t.queued_frames
    );
    let attempts_after = state.lock().unwrap().writer_attempts;
    assert_eq!(
        attempts_after, attempts,
        "shared flag clearance cannot authorize writes"
    );
    assert_eq!(t.state, HalOutputTransportState::ConfigurationChanged);
    assert_eq!(t.queued_frames, 2);
    assert_conserved(&p);
    let expected = p.prepared_transport_format().unwrap();
    assert!(matches!(
        p.recover_transport(expected),
        Err(SinkTransportRecoveryError::NeedsReprepare { .. })
    ));
    assert_eq!(p.telemetry().queued_frames, 2);
    state.lock().unwrap().transport_format = None;
    assert_eq!(
        p.recover_transport(expected).unwrap(),
        SinkTransportRecoveryStatus::Ready
    );
    assert!(
        Plugin::drain(&mut p, &mut [], &ProcessContext::new(48_000, 0))
            .unwrap()
            .complete
    );
    let accepted = state.lock().unwrap().accepted_samples.clone();
    assert_eq!(accepted, vec![1.0, 2.0, 3.0, 4.0]);
    assert_conserved(&p);
}
