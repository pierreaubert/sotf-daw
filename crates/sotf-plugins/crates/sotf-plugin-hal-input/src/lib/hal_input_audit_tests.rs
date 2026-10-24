//! Reader publication, availability and prepared-epoch regressions.

// Rust guideline compliant 2026-02-21
use super::*;
use std::sync::Mutex;

#[derive(Clone, Copy)]
enum Mutation {
    Disconnect,
    Key,
    Rate,
    Channels,
    Capacity,
    Unavailable,
}

struct State {
    format: HalStreamFormat,
    connected: bool,
    key_fault: bool,
    unavailable: bool,
    frames: usize,
    reads: usize,
    mutation: Option<Mutation>,
}

struct Reader(Arc<Mutex<State>>);

impl HalAudioSource for Reader {
    fn control_mapping_is_current(&self) -> bool {
        true
    }

    fn read_frames(&mut self, output: &mut [f32]) -> usize {
        let mut s = self.0.lock().unwrap();
        s.reads += 1;
        let samples = s.frames.saturating_mul(s.format.channels).min(output.len());
        for (i, v) in output[..samples].iter_mut().enumerate() {
            *v = (i + 1) as f32;
        }
        match s.mutation.take() {
            Some(Mutation::Disconnect) => s.connected = false,
            Some(Mutation::Key) => s.key_fault = true,
            Some(Mutation::Rate) => s.format.sample_rate = 96_000,
            Some(Mutation::Channels) => s.format.channels += 1,
            Some(Mutation::Capacity) => s.format.buffer_capacity_frames += 1,
            Some(Mutation::Unavailable) => s.unavailable = true,
            None => {}
        }
        s.frames
    }
    fn is_connected(&self) -> bool {
        self.0.lock().unwrap().connected
    }
    fn needs_control_recovery(&self) -> bool {
        self.0.lock().unwrap().key_fault
    }
    fn current_format(&self) -> Result<HalStreamFormat, String> {
        let s = self.0.lock().unwrap();
        if s.unavailable {
            Err("unavailable test format".into())
        } else {
            Ok(s.format)
        }
    }
}

fn processor(channels: usize) -> (HalInputPlugin, Arc<Mutex<State>>) {
    let state = Arc::new(Mutex::new(State {
        format: HalStreamFormat {
            sample_rate: 48_000,
            channels,
            buffer_capacity_frames: 16,
        },
        connected: true,
        key_fault: false,
        unavailable: false,
        frames: 3,
        reads: 0,
        mutation: None,
    }));
    let mut p = super::tests::stub_plugin(channels);
    p.activate_reader(Box::new(Reader(Arc::clone(&state))))
        .unwrap();
    p.initialize(48_000.0).unwrap();
    (p, state)
}

#[test]
fn unavailable_format_never_leaves_caller_output_unwritten() {
    let (mut p, state) = processor(2);
    state.lock().unwrap().unavailable = true;
    let mut out = [99.0; 16];
    assert!(
        p.process(&[], &mut out, &ProcessContext::new(48_000, 8))
            .is_err()
    );
    println!(
        "unavailable_format output_first={} recovery={}",
        out[0],
        p.diagnostics().needs_control_recovery
    );
    assert!(out.iter().all(|&v| v == 0.0));
    assert!(p.diagnostics().needs_control_recovery);
    assert_eq!(
        p.diagnostics().last_error,
        HalInputErrorKind::TransportCorruption
    );
    let reads = state.lock().unwrap().reads;
    assert_eq!(reads, 0);
}

#[test]
fn mid_read_fault_discards_the_complete_unvalidated_callback() {
    for mutation in [
        Mutation::Disconnect,
        Mutation::Key,
        Mutation::Rate,
        Mutation::Channels,
        Mutation::Capacity,
        Mutation::Unavailable,
    ] {
        let (mut p, state) = processor(2);
        p.stream_armed = true;
        state.lock().unwrap().mutation = Some(mutation);
        let mut out = [99.0; 16];
        let result = p.process(&[], &mut out, &ProcessContext::new(48_000, 8));
        let d = p.diagnostics();
        println!(
            "mid_read kind={:?} output_first={} missing={} recovery={}",
            d.last_error, out[0], d.missing_frames, d.needs_control_recovery
        );
        assert!(
            out.iter().all(|&v| v == 0.0),
            "unvalidated prefix must not escape"
        );
        assert!(d.needs_control_recovery);
        assert_eq!(d.underrun_count, 0);
        match mutation {
            Mutation::Disconnect | Mutation::Key => {
                assert_eq!(result, Ok(8));
                assert_eq!(d.missing_frames, 8);
            }
            _ => {
                assert!(result.is_err());
                assert_eq!(d.missing_frames, 0);
            }
        }
    }
}

#[test]
fn all_successful_unavailable_silence_is_counted_without_startup_underruns() {
    for unavailable in 0..3 {
        let (mut p, state) = processor(2);
        match unavailable {
            0 => state.lock().unwrap().connected = false,
            1 => state.lock().unwrap().key_fault = true,
            _ => p.reader = None,
        }
        let mut out = [99.0; 16];
        for _ in 0..2 {
            assert_eq!(
                p.process(&[], &mut out, &ProcessContext::new(48_000, 8)),
                Ok(8)
            );
        }
        let d = p.diagnostics();
        println!(
            "unavailable_silence mode={unavailable} missing={} underruns={}",
            d.missing_frames, d.underrun_count
        );
        assert_eq!(d.missing_frames, 16);
        assert_eq!(d.underrun_count, 0);
        assert!(d.needs_control_recovery);
        assert_eq!(d.recovery_generation, 1);
    }
}

#[test]
fn empty_callbacks_do_not_consume_reader_or_change_stable_observed_state() {
    let (mut p, state) = processor(2);
    let before = p.diagnostics();
    assert_eq!(
        p.process(&[], &mut [], &ProcessContext::new(48_000, 0)),
        Ok(0)
    );
    let reads = state.lock().unwrap().reads;
    assert_eq!(reads, 0);
    assert_eq!(p.diagnostics(), before);
}

#[test]
fn rejected_preparation_retains_active_state_and_success_starts_a_fresh_epoch() {
    let (mut p, state) = processor(2);
    let before = p.diagnostics();
    assert!(p.initialize(96_000.0).is_err());
    assert_eq!(
        p.diagnostics(),
        before,
        "failed clock preparation cannot corrupt active-clock status"
    );
    let (_, wrong) = processor(2);
    wrong.lock().unwrap().format.sample_rate = 96_000;
    assert!(p.activate_reader(Box::new(Reader(wrong))).is_err());
    assert_eq!(p.diagnostics(), before);
    p.stream_armed = true;
    p.initialize(48_000.0).unwrap();
    state.lock().unwrap().frames = 0;
    let mut out = [99.0; 16];
    p.process(&[], &mut out, &ProcessContext::new(48_000, 8))
        .unwrap();
    assert_eq!(
        p.diagnostics().underrun_count,
        0,
        "successful preparation starts unarmed"
    );
}

use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;
thread_local! {
    static TRACK_FREE: Cell<bool> = const {Cell::new(false)};
    static FREES: Cell<usize> = const {Cell::new(0)};
}
pub(super) struct CallbackAllocator;
// SAFETY: All storage operations delegate unchanged to the host allocator;
// constant-initialized thread-local counters do not allocate or touch storage.
unsafe impl GlobalAlloc for CallbackAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The caller's allocation layout is forwarded unchanged.
        unsafe { sotf_host::CountingAlloc.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if TRACK_FREE.try_with(Cell::get).unwrap_or(false) {
            FREES.with(|v| v.set(v.get() + 1));
        }
        // SAFETY: The original allocation pointer/layout are forwarded unchanged.
        unsafe { sotf_host::CountingAlloc.dealloc(ptr, layout) }
    }
}
fn no_heap(run: impl FnOnce()) {
    FREES.set(0);
    TRACK_FREE.set(true);
    sotf_host::assert_no_allocs("HAL input validated/unavailable callback", run);
    TRACK_FREE.set(false);
    assert_eq!(FREES.get(), 0);
}

#[test]
fn idle_callbacks_observe_readiness_without_reading_audio_or_counting_missing_frames() {
    let (mut p, state) = processor(2);
    let before = p.diagnostics();
    state.lock().unwrap().connected = false;
    assert_eq!(
        p.process(&[], &mut [], &ProcessContext::new(48_000, 0)),
        Ok(0)
    );
    let d = p.diagnostics();
    assert!(!d.connected && d.needs_control_recovery);
    assert_eq!(d.connection_generation, before.connection_generation + 1);
    assert_eq!(d.missing_frames, 0);
    let reads = state.lock().unwrap().reads;
    assert_eq!(reads, 0);
}

#[test]
fn callback_publication_and_recovery_follow_an_independent_observation_model() {
    let mut events = 0;
    let mut seed = 0x13579bdf_u32;
    let mut random = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as usize
    };
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for channels in 1..=16 {
            let (mut p, state) = processor(channels);
            state.lock().unwrap().format.sample_rate = rate;
            p.initialize(f64::from(rate)).unwrap();
            let mut connected = p.is_connected;
            let mut connections = p.diagnostics().connection_generation;
            let mut recovery = 0;
            let mut formats = 0;
            let mut missing = 0;
            let mut underruns = 0;
            let mut pending = false;
            let mut format_pending = false;
            let mut armed = false;
            for step in 0..120 {
                let case = step % 12;
                let frames = if case == 10 { 0 } else { 1 + random() % 17 };
                let count = if case == 9 {
                    frames + 1
                } else {
                    random() % (frames + 1)
                };
                {
                    let mut s = state.lock().unwrap();
                    s.format = HalStreamFormat {
                        sample_rate: rate,
                        channels,
                        buffer_capacity_frames: 16,
                    };
                    s.connected = case != 1;
                    s.key_fault = case == 2;
                    s.unavailable = case == 8;
                    s.frames = count;
                    s.mutation = match case {
                        3 => Some(Mutation::Disconnect),
                        4 => Some(Mutation::Key),
                        5 => Some(Mutation::Rate),
                        6 => Some(Mutation::Channels),
                        7 => Some(Mutation::Capacity),
                        _ => None,
                    };
                }
                let reads_before = state.lock().unwrap().reads;
                let mut out = vec![99.0; frames * channels];
                if case == 11 {
                    p.activate_reader(Box::new(Reader(Arc::clone(&state))))
                        .unwrap();
                    if !connected {
                        connections += 1;
                    }
                    connected = true;
                    pending = false;
                    format_pending = false;
                    armed = false;
                } else {
                    let result = p.process(&[], &mut out, &ProcessContext::new(rate, frames));
                    let pre_connected = case != 1;
                    if pre_connected != connected {
                        connections += 1;
                        connected = pre_connected;
                    }
                    let rate_fault = case == 5 && rate != 96_000;
                    let unavailable = case == 1 || case == 2 || case == 3 || case == 4;
                    let error = case == 8 || case == 9 || rate_fault || case == 6 || case == 7;
                    if case == 3 {
                        if connected {
                            connections += 1;
                        }
                        connected = false;
                    }
                    if unavailable || error {
                        if !pending {
                            recovery += 1;
                        }
                        pending = true;
                        armed = false;
                        if rate_fault || case == 6 || case == 7 {
                            if !format_pending {
                                formats += 1;
                            }
                            format_pending = true;
                        }
                        if unavailable {
                            missing += frames as u64;
                            assert_eq!(result, Ok(frames));
                        } else {
                            assert!(result.is_err());
                        }
                        assert!(out.iter().all(|&v| v == 0.0));
                    } else if frames != 0 {
                        assert_eq!(result, Ok(frames));
                        pending = false;
                        format_pending = false;
                        missing += (frames - count) as u64;
                        if count > 0 {
                            armed = true;
                        }
                        if count < frames && armed {
                            underruns += 1;
                        }
                        for (i, &sample) in out.iter().enumerate() {
                            assert_eq!(
                                sample,
                                if i < count * channels {
                                    (i + 1) as f32
                                } else {
                                    0.0
                                }
                            );
                        }
                    } else {
                        assert_eq!(result, Ok(0));
                    }
                    let reads = state.lock().unwrap().reads - reads_before;
                    let expected_read =
                        usize::from(frames > 0 && case != 1 && case != 2 && case != 8);
                    assert_eq!(reads, expected_read);
                }
                let d = p.diagnostics();
                assert_eq!(
                    (
                        d.connection_generation,
                        d.recovery_generation,
                        d.format_change_generation,
                        d.missing_frames,
                        d.underrun_count,
                        d.connected,
                        d.needs_control_recovery
                    ),
                    (
                        connections,
                        recovery,
                        formats,
                        missing,
                        underruns,
                        connected,
                        pending
                    ),
                    "rate={rate} channels={channels} step={step}"
                );
                assert_eq!(p.stream_armed, armed);
                assert_eq!(p.latency_samples(), 0);
                events += 1;
            }
        }
    }
    println!(
        "input_observation_model events={events} layouts=16 rates=4 exact_output_and_counters=true bounded_reads=true"
    );
}

#[test]
fn successful_and_unavailable_observations_have_zero_callback_heap_activity() {
    let mut callbacks = 0;
    for channels in 1..=16 {
        let (mut p, state) = processor(channels);
        let mut out = vec![99.0; 8 * channels];
        for case in 0..8 {
            {
                let mut s = state.lock().unwrap();
                s.connected = true;
                s.key_fault = false;
                s.frames = 3;
                s.mutation = None;
                match case {
                    0 => s.frames = 8,
                    1 => s.frames = 0,
                    2 => s.connected = false,
                    3 => s.key_fault = true,
                    4 => s.mutation = Some(Mutation::Disconnect),
                    5 => s.mutation = Some(Mutation::Key),
                    6 => s.frames = 8,
                    _ => s.frames = 3,
                }
            }
            no_heap(|| {
                p.process(&[], &mut out, &ProcessContext::new(48_000, 8))
                    .unwrap();
            });
            callbacks += 1;
        }
    }
    println!("input_observation_heap callbacks={callbacks} allocations=0 frees=0");
}

#[test]
fn failed_control_service_latches_stale_ready_reader_until_valid_replacement() {
    let (mut plugin, state) = processor(2);
    let context = ProcessContext::new(48_000, 3);
    let mut output = [99.0; 6];
    plugin.process(&[], &mut output, &context).unwrap();
    assert_eq!(output, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    let old_reads = state.lock().unwrap().reads;
    assert!(
        plugin
            .service_replacement(Err("broker exited".into()))
            .is_err()
    );
    assert!(
        state.lock().unwrap().connected,
        "retired ready bit remains set"
    );
    let before = plugin.diagnostics();
    assert!(!before.connected && before.needs_control_recovery);
    assert!(plugin.initialize(48_000.0).is_err());
    plugin.reset();
    for _ in 0..3 {
        output.fill(99.0);
        no_heap(|| {
            assert_eq!(plugin.process(&[], &mut output, &context), Ok(3));
        });
        assert_eq!(output, [0.0; 6]);
        assert!(!plugin.diagnostics().connected);
        assert!(plugin.diagnostics().needs_control_recovery);
    }
    assert_eq!(state.lock().unwrap().reads, old_reads);
    assert_eq!(
        plugin.diagnostics().missing_frames,
        before.missing_frames + 9
    );
    assert_eq!(plugin.diagnostics().underrun_count, before.underrun_count);
    // A rejected replacement cannot clear the control-service latch.
    let (_other, wrong) = processor(1);
    assert!(
        plugin
            .service_replacement(Ok(Box::new(Reader(wrong))))
            .is_err()
    );
    assert!(plugin.control_unavailable);
    plugin
        .service_replacement(Ok(Box::new(Reader(Arc::clone(&state)))))
        .unwrap();
    plugin.process(&[], &mut output, &context).unwrap();
    assert_eq!(output, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    assert!(plugin.diagnostics().connected);
    assert!(!plugin.diagnostics().needs_control_recovery);
}

#[test]
fn healthy_control_service_keeps_active_reader_and_its_history() {
    let (mut plugin, active) = processor(2);
    let (_other, probe) = processor(2);
    let mut output = [0.0; 6];
    plugin
        .process(&[], &mut output, &ProcessContext::new(48_000, 3))
        .unwrap();
    let before = plugin.diagnostics();
    plugin
        .service_replacement(Ok(Box::new(Reader(Arc::clone(&probe)))))
        .unwrap();
    assert_eq!(plugin.diagnostics(), before);
    plugin
        .process(&[], &mut output, &ProcessContext::new(48_000, 3))
        .unwrap();
    assert_eq!(active.lock().unwrap().reads, 2);
    assert_eq!(probe.lock().unwrap().reads, 0);
}

#[test]
fn healthy_probe_validates_the_prepared_format_before_retaining_reader() {
    let mut failures = 0;
    for mutation in [Mutation::Rate, Mutation::Channels, Mutation::Capacity] {
        let (mut plugin, active) = processor(2);
        let (_other, probe) = processor(2);
        let mut format = active.lock().unwrap().format;
        match mutation {
            Mutation::Rate => format.sample_rate = 96_000,
            Mutation::Channels => format.channels = 3,
            Mutation::Capacity => format.buffer_capacity_frames = 32,
            _ => unreachable!(),
        }
        active.lock().unwrap().format = format;
        probe.lock().unwrap().format = format;
        // Agreement between active and probed headers does not establish that
        // either still matches this plugin's prepared clock/channel/capacity.
        let result = plugin.service_replacement(Ok(Box::new(Reader(probe))));
        let correct = if matches!(mutation, Mutation::Capacity) {
            result.is_ok() && plugin.diagnostics().buffer_capacity_frames == 32
        } else {
            result.is_err()
                && plugin.diagnostics().needs_control_recovery
                && !plugin.diagnostics().connected
        };
        failures += usize::from(!correct);
    }
    println!("healthy_service_format cases=3 failures={failures}");
    assert_eq!(failures, 0);
}
