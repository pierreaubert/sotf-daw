// Detached control tests for linear-phase EQ dynamic updates.
//
// The shared `LinearPhaseEqControlHandle` behind `get_data` is the only
// control surface used here: workers read the immutable accepted base with
// `try_accepted_snapshot`, submit through the bounded mailbox, and observe
// generations plus queue/refusal status without wrapper `&mut` access. Audio
// commits run inside wrapper `process` only; no test calls DSP commit APIs.
// Every render, drain and reset leg asserts strict `(0, 0)` alloc/free with a
// thread-local counting allocator, and blend legs keep the exact-0 start plus
// final-twin `1e-5` bounds with deterministic stimuli.
//
// M1 accuracy (96 kHz / 1024 taps multiband, 0.05 dB) stays open separately;
// these tests use 48 kHz passing-case bands only.

use sotf_host::plugin::{Plugin, ProcessContext, TailLength};
use sotf_host::{ParameterId, ParameterValue};
use sotf_plugin_linear_phase_eq::dynamic_host::{
    LinearPhaseEqAcceptedSnapshot, LinearPhaseEqControlHandle, LinearPhaseEqDynamicPlugin,
};
use sotf_plugin_linear_phase_eq::{
    BandConfig, CommitRefusal, LinearPhaseEqPlugin, LinearPhaseEqPluginParams,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, mpsc};

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static COUNTS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}

struct CountAlloc;

// SAFETY: Forwards every request unchanged to `System`; counters never touch
// the returned pointers and never allocate.
unsafe impl GlobalAlloc for CountAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.try_with(Cell::get).unwrap_or(false) {
            let _ = COUNTS.try_with(|c| {
                let (a, d) = c.get();
                c.set((a + 1, d));
            });
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if COUNTING.try_with(Cell::get).unwrap_or(false) {
            let _ = COUNTS.try_with(|c| {
                let (a, d) = c.get();
                c.set((a, d + 1));
            });
        }
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountAlloc = CountAlloc;

/// Run `f` with allocation counting; returns `((allocs, frees), value)`.
fn count_allocs<R>(f: impl FnOnce() -> R) -> ((usize, usize), R) {
    COUNTS.set((0, 0));
    COUNTING.set(true);
    let value = f();
    COUNTING.set(false);
    (COUNTS.get(), value)
}

const RATE: u32 = 48_000;
const CHANNELS: usize = 2;
// Mirrors the DSP private `XFADE_FRAMES` (512); the blend spans
// `XFADE_FRAMES + 1` frames with exact 0/1 endpoints.
const XFADE_FRAMES: usize = 512;

fn band(filter_type: &str, frequency: f64, q: f64, gain_db: f64) -> BandConfig {
    BandConfig {
        filter_type: filter_type.to_string(),
        frequency,
        q,
        gain_db,
        active: true,
        placement: None,
    }
}

fn params_for(bands: Vec<BandConfig>) -> LinearPhaseEqPluginParams {
    let num_filters = bands.len();
    LinearPhaseEqPluginParams {
        num_filters,
        fir_length_index: 0,
        phase_mode_index: 0,
        auto_gain: false,
        mix: 1.0,
        filters: bands,
        stereo_pairs: None,
    }
}

fn pattern(frames: usize) -> Vec<f32> {
    (0..frames * CHANNELS)
        .map(|i| ((i * 7919 % 104729) as f32 / 104729.0 - 0.5) * 0.9)
        .collect()
}

fn create(bands: &[BandConfig]) -> LinearPhaseEqDynamicPlugin {
    let mut plugin =
        LinearPhaseEqDynamicPlugin::from_params(CHANNELS, RATE, params_for(bands.to_vec()))
            .expect("wrapper construction must succeed");
    Plugin::initialize(&mut plugin, f64::from(RATE)).expect("same-rate initialize must succeed");
    plugin
}

/// Fetch the detached handle exactly as an engine manager would: only the
/// `get_data` transport, never the same-thread `control_handle` accessor.
fn detached_handle(plugin: &dyn Plugin) -> Arc<LinearPhaseEqControlHandle> {
    let data = plugin
        .get_data()
        .expect("dynamic wrapper must expose a detached handle");
    Arc::downcast::<LinearPhaseEqControlHandle>(data).expect("handle type must downcast")
}

/// Process `input` into `output` in `block`-frame quanta via `Plugin`.
fn process_into(plugin: &mut dyn Plugin, input: &[f32], output: &mut [f32], block: usize) {
    assert_eq!(input.len(), output.len());
    assert_eq!(input.len() % CHANNELS, 0);
    let frames = input.len() / CHANNELS;
    let mut position = 0;
    while position < frames {
        let count = block.min(frames - position);
        let start = position * CHANNELS;
        let end = (position + count) * CHANNELS;
        let done = plugin
            .process(
                &input[start..end],
                &mut output[start..end],
                &ProcessContext::new(RATE, count),
            )
            .expect("process must succeed");
        assert_eq!(done, count);
        position += count;
    }
}

fn get_gain(plugin: &dyn Plugin, band_index: usize) -> f32 {
    match plugin.get_parameter(&ParameterId::from(
        format!("band_{band_index}_gain").as_str(),
    )) {
        Some(ParameterValue::Float(gain)) => gain,
        other => panic!("band_{band_index}_gain must read as Float, got {other:?}"),
    }
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

/// Process whole 512-frame quanta at 44.1 kHz via `Plugin`.
///
/// Index/slice loop mirroring `process_into` (no chunks API): identical
/// 512-frame partitions, same remainder rejection via the assert above.
fn process_into_44100(plugin: &mut dyn Plugin, input: &[f32], output: &mut [f32]) {
    assert_eq!(input.len(), output.len());
    assert_eq!(input.len() % (512 * CHANNELS), 0);
    let frames = input.len() / CHANNELS;
    let mut position = 0;
    while position < frames {
        let count = 512.min(frames - position);
        let start = position * CHANNELS;
        let end = (position + count) * CHANNELS;
        let done = plugin
            .process(
                &input[start..end],
                &mut output[start..end],
                &ProcessContext::new(44_100, count),
            )
            .expect("process must succeed");
        assert_eq!(done, count);
        position += count;
    }
}

/// Prepare one band edit on a worker thread; barrier plus ack sync, no sleep.
fn prepare_on_worker(
    base: &LinearPhaseEqAcceptedSnapshot,
    band_index: usize,
    new_band: BandConfig,
) -> sotf_plugin_linear_phase_eq::PreparedBandUpdate {
    let snapshot = base.snapshot.clone();
    let barrier = Barrier::new(2);
    let (tx, rx) = mpsc::channel();
    std::thread::scope(|s| {
        let barrier_ref = &barrier;
        s.spawn(move || {
            let prepared =
                LinearPhaseEqPlugin::prepare_band_update(&snapshot, band_index, new_band)
                    .expect("worker preparation must succeed");
            tx.send(prepared).expect("worker ack");
            barrier_ref.wait();
        });
        let prepared = rx.recv().expect("worker ack");
        barrier.wait();
        prepared
    })
}

/// Drop reclaimed payloads on a worker thread; barrier plus ack sync.
fn drop_on_worker<T: Send>(payloads: Vec<T>) {
    let barrier = Barrier::new(2);
    let (ack_tx, ack_rx) = mpsc::channel();
    std::thread::scope(|s| {
        let barrier_ref = &barrier;
        s.spawn(move || {
            drop(payloads);
            ack_tx.send(()).expect("drop ack");
            barrier_ref.wait();
        });
        ack_rx.recv().expect("drop ack");
        barrier.wait();
    });
}

#[test]
fn detached_initial_accepted_base_matches_params() {
    let bands = vec![
        band("Peak", 1000.0, 1.0, 0.0),
        band("Peak", 3000.0, 1.0, 0.0),
    ];
    let plugin = create(&bands);
    let handle = detached_handle(&plugin);
    assert_eq!(handle.accepted_generation(), 0);
    let accepted = handle
        .try_accepted_snapshot()
        .expect("initial accepted snapshot must read");
    assert_eq!(accepted.generation, 0);
    let snapshot = &accepted.snapshot;
    assert_eq!(snapshot.channels, CHANNELS);
    assert_eq!(snapshot.sample_rate, f64::from(RATE));
    assert_eq!(snapshot.num_filters, 2);
    assert_eq!(snapshot.fir_length_index, 0);
    assert_eq!(snapshot.phase_mode_index, 0);
    assert!(!snapshot.auto_gain);
    assert_eq!(snapshot.bands.len(), 2);
    assert_eq!(snapshot.bands[0].frequency, 1000.0);
    assert_eq!(snapshot.bands[1].frequency, 3000.0);
    assert_eq!(snapshot.bands[0].gain_db, 0.0);
    assert_eq!(snapshot.stereo_pairs, vec![[0, 1]]);
    // The detached base equals the same-thread control snapshot exactly.
    assert_eq!(*snapshot, plugin.snapshot_for_update());
    let status = handle.control_status();
    assert_eq!(status.accepted_generation, 0);
    assert_eq!(status.retained_queued, 0);
    assert!(!status.blend_in_progress);
    assert!(!status.retired_held);
    assert_eq!(status.last_refusal, None);
    // The detached base is a usable prepare input.
    LinearPhaseEqPlugin::prepare_band_update(snapshot, 0, band("Peak", 1000.0, 1.0, 6.0))
        .expect("detached base must prepare");
}

fn run_detached_automation_once(block: usize) -> Vec<f32> {
    let old_bands = vec![
        band("Peak", 1000.0, 1.0, 0.0),
        band("Peak", 3000.0, 1.0, 0.0),
    ];
    let total = 3000;
    let commit_at = 2048;
    let input = pattern(total);
    let mut plugin = create(&old_bands);
    let latency_before = plugin.latency_samples();
    let mut output = vec![0.0; input.len()];
    process_into(
        &mut plugin,
        &input[..commit_at * CHANNELS],
        &mut output[..commit_at * CHANNELS],
        block,
    );
    assert!(rms(&output[..commit_at * CHANNELS]) > 1e-4);
    // Detached worker flow: accepted base read from the handle only.
    let handle = detached_handle(&plugin);
    let base = handle.try_accepted_snapshot().expect("base must read");
    assert_eq!(base.generation, 0);
    let prepared = prepare_on_worker(&base, 0, band("Peak", 1000.0, 1.0, 9.0));
    handle.try_submit(prepared).expect("submit must queue");
    // Queued submission is not acceptance: generation, snapshot and getters
    // still describe the old configuration before any audio runs.
    assert_eq!(handle.accepted_generation(), 0);
    assert_eq!(
        handle
            .try_accepted_snapshot()
            .expect("snapshot must read")
            .snapshot,
        base.snapshot
    );
    assert_eq!(get_gain(&plugin, 0), 0.0);
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(
            &mut plugin,
            &input[commit_at * CHANNELS..],
            &mut output[commit_at * CHANNELS..],
            block,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    // Only the real audio commit advances generation and snapshot.
    assert_eq!(handle.accepted_generation(), 1);
    let accepted = handle.try_accepted_snapshot().expect("snapshot must read");
    assert_eq!(accepted.generation, 1);
    assert_eq!(accepted.snapshot.bands[0].gain_db, 9.0);
    assert_eq!(get_gain(&plugin, 0), 9.0);
    assert_eq!(plugin.latency_samples(), latency_before);
    assert!(rms(&output[commit_at * CHANNELS..]) > 1e-4);
    output
}

#[test]
fn detached_worker_submit_commits_once_with_exact_blend() {
    let old_bands = vec![
        band("Peak", 1000.0, 1.0, 0.0),
        band("Peak", 3000.0, 1.0, 0.0),
    ];
    let new_bands = vec![
        band("Peak", 1000.0, 1.0, 9.0),
        band("Peak", 3000.0, 1.0, 0.0),
    ];
    let total = 3000;
    let commit_at = 2048;
    let input = pattern(total);
    let output = run_detached_automation_once(256);
    let mut old_plugin = create(&old_bands);
    let mut new_plugin = create(&new_bands);
    let mut old_out = vec![0.0; input.len()];
    let mut new_out = vec![0.0; input.len()];
    process_into(&mut old_plugin, &input, &mut old_out, 256);
    process_into(&mut new_plugin, &input, &mut new_out, 256);
    // Exact-0 blend start on every channel.
    for ch in 0..CHANNELS {
        let index = commit_at * CHANNELS + ch;
        assert_eq!(output[index], old_out[index], "exact-0 blend start ch{ch}");
    }
    // Blend morph follows the fixed 512-frame ramp within 1e-5.
    for frame in commit_at..total {
        let step = frame - commit_at;
        let weight = if step <= XFADE_FRAMES {
            step as f64 / XFADE_FRAMES as f64
        } else {
            1.0
        };
        for ch in 0..CHANNELS {
            let index = frame * CHANNELS + ch;
            let expected =
                f64::from(old_out[index]) * (1.0 - weight) + f64::from(new_out[index]) * weight;
            assert!(
                (f64::from(output[index]) - expected).abs() < 1e-5,
                "frame{frame} ch{ch}"
            );
        }
    }
    // Partitions are bit-exact for the same commit frame.
    let reference = run_detached_automation_once(1);
    assert_eq!(output, reference);
    for block in [63, 512] {
        assert_eq!(run_detached_automation_once(block), reference, "block {block}");
    }
}

#[test]
fn detached_second_edit_during_blend_applies_after_retirement() {
    let old_bands = vec![
        band("Peak", 1000.0, 1.0, 0.0),
        band("Peak", 3000.0, 1.0, 0.0),
    ];
    let final_bands = vec![
        band("Peak", 1000.0, 1.0, -12.0),
        band("Peak", 3000.0, 1.0, 0.0),
    ];
    let total = 4000;
    let first_at = 2048;
    let second_at = 2300;
    let input = pattern(total);
    let mut plugin = create(&old_bands);
    let mut output = vec![0.0; input.len()];
    process_into(
        &mut plugin,
        &input[..first_at * CHANNELS],
        &mut output[..first_at * CHANNELS],
        256,
    );
    // First edit via the detached worker flow.
    let handle = detached_handle(&plugin);
    let base = handle.try_accepted_snapshot().expect("base must read");
    let prepared = prepare_on_worker(&base, 0, band("Peak", 1000.0, 1.0, -6.0));
    handle.try_submit(prepared).expect("first submit must queue");
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(
            &mut plugin,
            &input[first_at * CHANNELS..second_at * CHANNELS],
            &mut output[first_at * CHANNELS..second_at * CHANNELS],
            128,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(handle.accepted_generation(), 1);
    assert_eq!(get_gain(&plugin, 0), -6.0);
    assert!(handle.control_status().blend_in_progress);
    // Second edit during the first blend, from the fresh detached base.
    let fresh = handle.try_accepted_snapshot().expect("fresh base must read");
    assert_eq!(fresh.generation, 1);
    let second = prepare_on_worker(&fresh, 0, band("Peak", 1000.0, 1.0, -12.0));
    handle.try_submit(second).expect("second submit must queue");
    // The pending blend keeps the first acceptance visible.
    assert_eq!(handle.accepted_generation(), 1);
    assert_eq!(get_gain(&plugin, 0), -6.0);
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(
            &mut plugin,
            &input[second_at * CHANNELS..],
            &mut output[second_at * CHANNELS..],
            256,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    // Reclaim retired banks on a worker thread via the shared handle.
    let handle = detached_handle(&plugin);
    let barrier = Barrier::new(2);
    let (ack_tx, ack_rx) = mpsc::channel();
    std::thread::scope(|s| {
        let barrier_ref = &barrier;
        s.spawn(move || {
            let reclaimed = handle.try_reclaim();
            ack_tx.send(reclaimed).expect("reclaim ack");
            barrier_ref.wait();
        });
        let reclaimed = ack_rx.recv().expect("reclaim ack");
        assert!(reclaimed > 0, "completed blends must retire banks");
        barrier.wait();
    });
    // Render past the second blend; the settled tail matches a final twin.
    let tail_in = pattern(1200);
    let mut tail_out = vec![0.0; tail_in.len()];
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(&mut plugin, &tail_in, &mut tail_out, 256);
    });
    assert_eq!((allocs, frees), (0, 0));
    let handle = detached_handle(&plugin);
    assert_eq!(handle.accepted_generation(), 2);
    assert_eq!(
        handle
            .try_accepted_snapshot()
            .expect("snapshot must read")
            .snapshot
            .bands[0]
            .gain_db,
        -12.0
    );
    let mut final_plugin = create(&final_bands);
    let mut final_out = vec![0.0; input.len()];
    process_into(&mut final_plugin, &input, &mut final_out, 256);
    for frame in (total - 400)..total {
        for ch in 0..CHANNELS {
            let index = frame * CHANNELS + ch;
            assert!(
                (f64::from(output[index]) - f64::from(final_out[index])).abs() < 1e-5,
                "frame{frame} ch{ch}"
            );
        }
    }
    assert!(rms(&tail_out) > 1e-4);
}

#[test]
fn detached_stale_base_refuses_and_reprepare_recovers() {
    let old_bands = vec![
        band("Peak", 1000.0, 1.0, 0.0),
        band("Peak", 3000.0, 1.0, 0.0),
    ];
    let mut plugin = create(&old_bands);
    let input = pattern(2700);
    let mut output = vec![0.0; input.len()];
    process_into(
        &mut plugin,
        &input[..2048 * CHANNELS],
        &mut output[..2048 * CHANNELS],
        256,
    );
    let handle = detached_handle(&plugin);
    let stale_base = handle.try_accepted_snapshot().expect("base must read");
    let first = prepare_on_worker(&stale_base, 0, band("Peak", 1000.0, 1.0, 6.0));
    handle.try_submit(first).expect("first submit must queue");
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(
            &mut plugin,
            &input[2048 * CHANNELS..2100 * CHANNELS],
            &mut output[2048 * CHANNELS..2100 * CHANNELS],
            52,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(handle.accepted_generation(), 1);
    // Stale second edit prepared from the pre-first-commit base.
    let stale = prepare_on_worker(&stale_base, 1, band("Peak", 3000.0, 1.0, -6.0));
    handle.try_submit(stale).expect("stale submit must queue");
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(
            &mut plugin,
            &input[2100 * CHANNELS..],
            &mut output[2100 * CHANNELS..],
            256,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    plugin.reclaim_retired();
    let probe_in = pattern(128);
    let mut probe_out = vec![0.0; probe_in.len()];
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(&mut plugin, &probe_in, &mut probe_out, 128);
    });
    assert_eq!((allocs, frees), (0, 0));
    // Refusal is reachable through the detached mirror and matches the
    // wrapper record; accepted state and getters stay retained.
    let status = handle.control_status();
    assert_eq!(status.last_refusal, Some(CommitRefusal::StaleBase));
    assert_eq!(plugin.last_refusal(), Some(CommitRefusal::StaleBase));
    assert_eq!(handle.accepted_generation(), 1);
    assert_eq!(get_gain(&plugin, 0), 6.0);
    assert_eq!(get_gain(&plugin, 1), 0.0);
    // Fresh reprepare from the current detached base recovers.
    handle.request_cancel();
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(&mut plugin, &probe_in, &mut probe_out, 128);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(handle.take_cancelled().len(), 1);
    let fresh = handle.try_accepted_snapshot().expect("fresh base must read");
    assert_eq!(fresh.generation, 1);
    let recovered = prepare_on_worker(&fresh, 1, band("Peak", 3000.0, 1.0, -6.0));
    handle.try_submit(recovered).expect("recovery submit must queue");
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(&mut plugin, &probe_in, &mut probe_out, 64);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(handle.accepted_generation(), 2);
    assert_eq!(handle.control_status().last_refusal, None);
    assert_eq!(get_gain(&plugin, 1), -6.0);
    assert!(rms(&probe_out) > 0.0);
}

#[test]
fn detached_cancel_evicts_stale_head_and_fresh_applies() {
    let old_bands = vec![
        band("Peak", 1000.0, 1.0, 0.0),
        band("Peak", 3000.0, 1.0, 0.0),
    ];
    let final_bands = vec![
        band("Peak", 1000.0, 1.0, 6.0),
        band("Peak", 3000.0, 1.0, -6.0),
    ];
    let mut plugin = create(&old_bands);
    let input = pattern(3600);
    let mut output = vec![0.0; input.len()];
    process_into(
        &mut plugin,
        &input[..2048 * CHANNELS],
        &mut output[..2048 * CHANNELS],
        256,
    );
    let handle = detached_handle(&plugin);
    let stale_base = handle.try_accepted_snapshot().expect("base must read");
    let first = prepare_on_worker(&stale_base, 0, band("Peak", 1000.0, 1.0, 6.0));
    handle.try_submit(first).expect("first submit must queue");
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(
            &mut plugin,
            &input[2048 * CHANNELS..2100 * CHANNELS],
            &mut output[2048 * CHANNELS..2100 * CHANNELS],
            52,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    let stale = prepare_on_worker(&stale_base, 1, band("Peak", 3000.0, 1.0, -6.0));
    handle.try_submit(stale).expect("stale submit must queue");
    // Run past the 513-frame first blend; the stale head refuses.
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(
            &mut plugin,
            &input[2100 * CHANNELS..2700 * CHANNELS],
            &mut output[2100 * CHANNELS..2700 * CHANNELS],
            256,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    plugin.reclaim_retired();
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(
            &mut plugin,
            &input[2700 * CHANNELS..2764 * CHANNELS],
            &mut output[2700 * CHANNELS..2764 * CHANNELS],
            64,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(
        handle.control_status().last_refusal,
        Some(CommitRefusal::StaleBase)
    );
    assert_eq!(handle.control_status().retained_queued, 1);
    // Queue a fresh edit behind the wedged stale head.
    let fresh_base = handle.try_accepted_snapshot().expect("fresh base must read");
    let fresh = prepare_on_worker(&fresh_base, 1, band("Peak", 3000.0, 1.0, -6.0));
    handle.try_submit(fresh).expect("fresh submit must queue");
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(
            &mut plugin,
            &input[2764 * CHANNELS..2828 * CHANNELS],
            &mut output[2764 * CHANNELS..2828 * CHANNELS],
            64,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(handle.control_status().retained_queued, 2);
    // Explicit head cancel evicts only the stale payload; the fresh queued
    // edit promotes and commits in the same quantum.
    handle.request_cancel();
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(
            &mut plugin,
            &input[2828 * CHANNELS..2892 * CHANNELS],
            &mut output[2828 * CHANNELS..2892 * CHANNELS],
            64,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    let status = handle.control_status();
    assert_eq!(status.retained_queued, 0);
    assert_eq!(status.last_refusal, None);
    assert_eq!(status.accepted_generation, 2);
    assert_eq!(get_gain(&plugin, 0), 6.0);
    assert_eq!(get_gain(&plugin, 1), -6.0);
    // Exactly the stale head lands in the outbox; destroy it on a worker.
    let cancelled = handle.take_cancelled();
    assert_eq!(cancelled.len(), 1, "exactly the stale head evicts");
    drop_on_worker(cancelled);
    assert_eq!(handle.try_reclaim_cancelled(), 0);
    // Settled audio matches a from-scratch final twin within 1e-5.
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(
            &mut plugin,
            &input[2892 * CHANNELS..],
            &mut output[2892 * CHANNELS..],
            256,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    plugin.reclaim_retired();
    let mut final_plugin = create(&final_bands);
    let mut final_out = vec![0.0; input.len()];
    process_into(&mut final_plugin, &input, &mut final_out, 256);
    for frame in 3341..3600 {
        for ch in 0..CHANNELS {
            let index = frame * CHANNELS + ch;
            assert!(
                (f64::from(output[index]) - f64::from(final_out[index])).abs() < 1e-5,
                "frame{frame} ch{ch}"
            );
        }
    }
}

#[test]
fn detached_queue_full_retains_live_without_alloc() {
    let old_bands = vec![
        band("Peak", 1000.0, 1.0, 0.0),
        band("Peak", 3000.0, 1.0, 0.0),
    ];
    let mut plugin = create(&old_bands);
    let input = pattern(2700);
    let mut output = vec![0.0; input.len()];
    process_into(
        &mut plugin,
        &input[..2048 * CHANNELS],
        &mut output[..2048 * CHANNELS],
        256,
    );
    // Fill the two-slot handle mailbox without rendering; the third fails.
    let handle = detached_handle(&plugin);
    let base = handle.try_accepted_snapshot().expect("base must read");
    for gain in [4.0, 5.0] {
        let prepared = prepare_on_worker(&base, 0, band("Peak", 1000.0, 1.0, gain));
        handle.try_submit(prepared).expect("mailbox room");
    }
    let overflow = prepare_on_worker(&base, 0, band("Peak", 1000.0, 1.0, 6.0));
    let err = handle
        .try_submit(overflow)
        .expect_err("third concurrent submit must report full");
    assert!(err.contains("full"), "{err}");
    assert_eq!(handle.accepted_generation(), 0);
    assert_eq!(get_gain(&plugin, 0), 0.0);
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(
            &mut plugin,
            &input[2048 * CHANNELS..2112 * CHANNELS],
            &mut output[2048 * CHANNELS..2112 * CHANNELS],
            64,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(handle.accepted_generation(), 1);
    assert_eq!(get_gain(&plugin, 0), 4.0);
    // The same-base sibling can no longer match live: it refuses stale
    // instead of silently overwriting the first acceptance.
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(
            &mut plugin,
            &input[2112 * CHANNELS..],
            &mut output[2112 * CHANNELS..],
            256,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    plugin.reclaim_retired();
    let probe_in = pattern(64);
    let mut probe_out = vec![0.0; probe_in.len()];
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(&mut plugin, &probe_in, &mut probe_out, 64);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(
        handle.control_status().last_refusal,
        Some(CommitRefusal::StaleBase)
    );
    assert_eq!(get_gain(&plugin, 0), 4.0);
}

#[test]
fn detached_orphan_handle_cannot_touch_new_graph() {
    let bands = vec![
        band("Peak", 1000.0, 1.0, 0.0),
        band("Peak", 3000.0, 1.0, 0.0),
    ];
    let mut graph_a = create(&bands);
    let input = pattern(2600);
    let mut output = vec![0.0; input.len()];
    process_into(
        &mut graph_a,
        &input[..2048 * CHANNELS],
        &mut output[..2048 * CHANNELS],
        256,
    );
    let handle_a = detached_handle(&graph_a);
    let base_a = handle_a.try_accepted_snapshot().expect("base must read");
    let evolved = prepare_on_worker(&base_a, 0, band("Peak", 1000.0, 1.0, 6.0));
    handle_a.try_submit(evolved).expect("submit must queue");
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(
            &mut graph_a,
            &input[2048 * CHANNELS..2112 * CHANNELS],
            &mut output[2048 * CHANNELS..2112 * CHANNELS],
            64,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(handle_a.accepted_generation(), 1);
    let frozen_a = handle_a
        .try_accepted_snapshot()
        .expect("snapshot must read")
        .snapshot;
    // Rebuild the graph; the old handle is now orphaned.
    drop(graph_a);
    let mut graph_b = create(&bands);
    let handle_b = detached_handle(&graph_b);
    assert!(!Arc::ptr_eq(&handle_a, &handle_b));
    assert_eq!(handle_b.accepted_generation(), 0);
    // A submission through the orphaned handle cannot reach the new graph,
    // whether the orphaned mailbox reports full or accepts into the void.
    let base_b = handle_b.try_accepted_snapshot().expect("base must read");
    assert_eq!(base_b.generation, 0);
    let orphaned = prepare_on_worker(
        &handle_a.try_accepted_snapshot().expect("orphan reads frozen"),
        0,
        band("Peak", 1000.0, 1.0, 12.0),
    );
    // Both outcomes are safe by design; log which one occurred for future
    // orphan-endpoint diagnostics.
    match handle_a.try_submit(orphaned) {
        Ok(()) => eprintln!("orphan submit outcome: queued into orphaned mailbox"),
        Err(reason) => eprintln!("orphan submit outcome: refused ({reason})"),
    }
    let mut fresh_out = vec![0.0; input.len()];
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(&mut graph_b, &input, &mut fresh_out, 128);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(handle_b.accepted_generation(), 0);
    assert_eq!(get_gain(&graph_b, 0), 0.0);
    // The orphan keeps reporting its own frozen graph state, never the new.
    assert_eq!(handle_a.accepted_generation(), 1);
    assert_eq!(
        handle_a
            .try_accepted_snapshot()
            .expect("orphan must stay readable")
            .snapshot,
        frozen_a
    );
    // The new graph stays healthy through its own detached handle.
    let base_b = handle_b.try_accepted_snapshot().expect("base must read");
    let healthy = prepare_on_worker(&base_b, 1, band("Peak", 3000.0, 1.0, -3.0));
    handle_b.try_submit(healthy).expect("submit must queue");
    let probe_in = pattern(64);
    let mut probe_out = vec![0.0; probe_in.len()];
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(&mut graph_b, &probe_in, &mut probe_out, 64);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(handle_b.accepted_generation(), 1);
    assert_eq!(get_gain(&graph_b, 1), -3.0);
}

#[test]
fn detached_reset_retirement_and_eof_preserve_accepted() {
    let old_bands = vec![
        band("Peak", 1000.0, 1.0, 0.0),
        band("Peak", 3000.0, 1.0, 0.0),
    ];
    let mut plugin = create(&old_bands);
    let input = pattern(2600);
    let mut output = vec![0.0; input.len()];
    process_into(
        &mut plugin,
        &input[..2048 * CHANNELS],
        &mut output[..2048 * CHANNELS],
        256,
    );
    let handle = detached_handle(&plugin);
    let base = handle.try_accepted_snapshot().expect("base must read");
    let first = prepare_on_worker(&base, 0, band("Peak", 1000.0, 1.0, 6.0));
    handle.try_submit(first).expect("submit must queue");
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(
            &mut plugin,
            &input[2048 * CHANNELS..2148 * CHANNELS],
            &mut output[2048 * CHANNELS..2148 * CHANNELS],
            100,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(handle.accepted_generation(), 1);
    assert!(handle.control_status().blend_in_progress);
    // Reset completes the blend instantly and preserves acceptance.
    let ((allocs, frees), ()) = count_allocs(|| {
        Plugin::reset(&mut plugin);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(handle.accepted_generation(), 1);
    assert_eq!(
        handle
            .try_accepted_snapshot()
            .expect("snapshot must read")
            .snapshot
            .bands[0]
            .gain_db,
        6.0
    );
    assert!(!handle.control_status().blend_in_progress);
    assert_eq!(get_gain(&plugin, 0), 6.0);
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(
            &mut plugin,
            &input[2148 * CHANNELS..],
            &mut output[2148 * CHANNELS..],
            256,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    // Retired banks from the completed blend reclaim off audio: at least
    // one retired set returns through the mailbox plus holding slot.
    let mut reclaimed = handle.try_reclaim();
    if plugin.reclaim_retired() {
        reclaimed += 1;
    }
    assert!(reclaimed > 0, "completed blend must retire banks");
    assert!(!handle.control_status().retired_held);
    // Full EOF: finite tail, exact length, idempotent drain, all (0, 0).
    let support = match plugin.tail_length() {
        TailLength::Finite(frames) => frames as usize,
        _ => panic!("expected a finite tail"),
    };
    let mut tailed = Vec::with_capacity(output.len() + (support + 8) * CHANNELS);
    tailed.extend_from_slice(&output);
    let mut scratch = vec![0.0; 257 * CHANNELS];
    let ((allocs, frees), tail_frames) = count_allocs(|| {
        let mut frames = 0;
        for _ in 0..20000 {
            let drained = plugin
                .drain(&mut scratch, &ProcessContext::new(RATE, 0))
                .expect("drain must succeed");
            tailed.extend_from_slice(&scratch[..drained.frames * CHANNELS]);
            frames += drained.frames;
            if drained.complete {
                break;
            }
        }
        frames
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(tailed.len(), (2600 + support) * CHANNELS);
    assert!(tail_frames > 0);
    assert!(tailed.iter().all(|s| s.is_finite()));
    let mut empty = vec![0.0; CHANNELS];
    let second = plugin
        .drain(&mut empty, &ProcessContext::new(RATE, 0))
        .expect("second drain must succeed");
    assert!(second.complete);
    // Acceptance survives EOF. Queue one edit, then prove post-drain
    // processing refuses `Drained` while retaining the slot; reset lets the
    // retained edit commit without reprepare.
    assert_eq!(handle.accepted_generation(), 1);
    let retained_base = handle.try_accepted_snapshot().expect("base must read");
    let retained = prepare_on_worker(&retained_base, 1, band("Peak", 3000.0, 1.0, -6.0));
    handle.try_submit(retained).expect("submit must queue");
    let probe_in = pattern(64);
    let mut probe_out = vec![0.0; probe_in.len()];
    plugin
        .process(&probe_in, &mut probe_out, &ProcessContext::new(RATE, 64))
        .expect_err("process after drain must require reset");
    assert_eq!(
        handle.control_status().last_refusal,
        Some(CommitRefusal::Drained)
    );
    assert_eq!(handle.accepted_generation(), 1);
    let ((allocs, frees), ()) = count_allocs(|| {
        Plugin::reset(&mut plugin);
    });
    assert_eq!((allocs, frees), (0, 0));
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(&mut plugin, &probe_in, &mut probe_out, 64);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(handle.accepted_generation(), 2);
    assert_eq!(handle.control_status().last_refusal, None);
    assert_eq!(get_gain(&plugin, 1), -6.0);
    assert!(rms(&probe_out) > 0.0);
}

#[test]
fn detached_rate_reinit_republishes_without_touching_bands() {
    let bands = vec![
        band("Peak", 1000.0, 1.0, 3.0),
        band("Peak", 3000.0, 1.0, -3.0),
    ];
    let mut plugin = create(&bands);
    let handle = detached_handle(&plugin);
    assert_eq!(handle.accepted_generation(), 0);
    // Same-rate initialize publishes nothing.
    Plugin::initialize(&mut plugin, f64::from(RATE)).expect("same-rate initialize must succeed");
    assert_eq!(handle.accepted_generation(), 0);
    // A real rate change republishes the accepted rate and bumps the
    // generation; band shapes are untouched by the rebuild.
    Plugin::initialize(&mut plugin, 44_100.0).expect("rate change must succeed");
    assert_eq!(handle.accepted_generation(), 1);
    let accepted = handle.try_accepted_snapshot().expect("snapshot must read");
    assert_eq!(accepted.generation, 1);
    assert_eq!(accepted.snapshot.sample_rate, 44_100.0);
    assert_eq!(accepted.snapshot.bands[0].gain_db, 3.0);
    assert_eq!(accepted.snapshot.bands[1].gain_db, -3.0);
    // The new rate renders allocation-free and accepts detached commits.
    // Render past the 544-frame linear latency before asserting nonzero.
    let input: Vec<f32> = (0..2048 * CHANNELS)
        .map(|i| ((i * 7919 % 104729) as f32 / 104729.0 - 0.5) * 0.9)
        .collect();
    let mut output = vec![0.0; input.len()];
    let ((allocs, frees), ()) = count_allocs(|| {
        for quantum in 0..4 {
            let range = quantum * 512 * CHANNELS..(quantum + 1) * 512 * CHANNELS;
            let done = plugin
                .process(
                    &input[range.clone()],
                    &mut output[range],
                    &ProcessContext::new(44_100, 512),
                )
                .expect("process at the new rate must succeed");
            assert_eq!(done, 512);
        }
    });
    assert_eq!((allocs, frees), (0, 0));
    assert!(rms(&output) > 1e-4);
    let fresh = handle.try_accepted_snapshot().expect("fresh base must read");
    let prepared = prepare_on_worker(&fresh, 0, band("Peak", 1000.0, 1.0, 6.0));
    handle.try_submit(prepared).expect("submit must queue");
    let ((allocs, frees), ()) = count_allocs(|| {
        let done = plugin
            .process(
                &input[..512 * CHANNELS],
                &mut output[..512 * CHANNELS],
                &ProcessContext::new(44_100, 512),
            )
            .expect("process must succeed");
        assert_eq!(done, 512);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(handle.accepted_generation(), 2);
    assert_eq!(get_gain(&plugin, 0), 6.0);
}

#[test]
fn detached_old_rate_payload_refuses_then_cancel_reprepare_recovers() {
    let bands = vec![
        band("Peak", 1000.0, 1.0, 0.0),
        band("Peak", 3000.0, 1.0, 0.0),
    ];
    let mut plugin = create(&bands);
    let input = pattern(2048);
    let mut output = vec![0.0; input.len()];
    process_into(&mut plugin, &input, &mut output, 256);
    let handle = detached_handle(&plugin);
    // Queue one edit at the construction rate without rendering.
    let old_rate_base = handle.try_accepted_snapshot().expect("base must read");
    assert_eq!(old_rate_base.snapshot.sample_rate, f64::from(RATE));
    let queued = prepare_on_worker(&old_rate_base, 0, band("Peak", 1000.0, 1.0, 6.0));
    handle.try_submit(queued).expect("submit must queue");
    assert_eq!(handle.accepted_generation(), 0);
    // Rate change republishes and bumps; the queued old-rate payload stays.
    Plugin::initialize(&mut plugin, 44_100.0).expect("rate change must succeed");
    assert_eq!(handle.accepted_generation(), 1);
    let republished = handle.try_accepted_snapshot().expect("snapshot must read");
    assert_eq!(republished.snapshot.sample_rate, 44_100.0);
    assert_eq!(republished.snapshot.bands[0].gain_db, 0.0);
    // The old-rate head refuses stale at the new rate; nothing commits.
    let probe_in = pattern(2048);
    let mut probe_out = vec![0.0; probe_in.len()];
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into_44100(&mut plugin, &probe_in, &mut probe_out);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(handle.accepted_generation(), 1);
    assert_eq!(
        handle.control_status().last_refusal,
        Some(CommitRefusal::StaleBase)
    );
    assert_eq!(plugin.last_refusal(), Some(CommitRefusal::StaleBase));
    assert_eq!(get_gain(&plugin, 0), 0.0);
    // Cancel evicts the stale head; the outbox holds exactly it.
    handle.request_cancel();
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into_44100(&mut plugin, &probe_in, &mut probe_out);
    });
    assert_eq!((allocs, frees), (0, 0));
    let cancelled = handle.take_cancelled();
    assert_eq!(cancelled.len(), 1);
    drop_on_worker(cancelled);
    assert_eq!(handle.control_status().last_refusal, None);
    assert_eq!(handle.control_status().retained_queued, 0);
    // Fresh reprepare at the new rate recovers and commits.
    let fresh = handle.try_accepted_snapshot().expect("fresh base must read");
    assert_eq!(fresh.generation, 1);
    assert_eq!(fresh.snapshot.sample_rate, 44_100.0);
    let recovered = prepare_on_worker(&fresh, 0, band("Peak", 1000.0, 1.0, 6.0));
    handle.try_submit(recovered).expect("recovery submit must queue");
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into_44100(&mut plugin, &probe_in, &mut probe_out);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(handle.accepted_generation(), 2);
    assert_eq!(get_gain(&plugin, 0), 6.0);
    assert!(rms(&probe_out) > 1e-4);
}

#[test]
fn detached_retired_holding_slot_mirror_reports_third_unclaimed() {
    let bands = vec![
        band("Peak", 1000.0, 1.0, 0.0),
        band("Peak", 3000.0, 1.0, 0.0),
    ];
    let mut plugin = create(&bands);
    let history = pattern(2048);
    let mut history_out = vec![0.0; history.len()];
    process_into(&mut plugin, &history, &mut history_out, 256);
    let handle = detached_handle(&plugin);
    // Three sequential commits without any reclaim: the first two retired
    // sets fill the two-slot retired mailbox, the third parks in the
    // holding slot and the mirror reports it.
    for (round, gain) in [2.0, 4.0, 6.0].into_iter().enumerate() {
        let base = handle.try_accepted_snapshot().expect("base must read");
        assert_eq!(base.generation, round as u64);
        let prepared = prepare_on_worker(&base, 0, band("Peak", 1000.0, 1.0, gain));
        handle.try_submit(prepared).expect("submit must queue");
        let input = pattern(600 + 64);
        let mut output = vec![0.0; input.len()];
        let ((allocs, frees), ()) = count_allocs(|| {
            process_into(&mut plugin, &input, &mut output, 256);
        });
        assert_eq!((allocs, frees), (0, 0));
        assert_eq!(handle.accepted_generation(), round as u64 + 1);
    }
    assert_eq!(get_gain(&plugin, 0), 6.0);
    // One trailing quantum moves the third retired set: the mailbox already
    // holds the first two, so it parks in the holding slot.
    let tail_in = pattern(64);
    let mut tail_out = vec![0.0; tail_in.len()];
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(&mut plugin, &tail_in, &mut tail_out, 64);
    });
    assert_eq!((allocs, frees), (0, 0));
    let status = handle.control_status();
    assert!(
        status.retired_held,
        "holding slot must park the third retired set"
    );
    assert!(plugin.retired_held());
    assert_eq!(status.last_refusal, None);
    // Reclaim returns the mailbox pair plus the holding slot; mirrors clear.
    assert_eq!(handle.try_reclaim(), 2);
    assert!(plugin.reclaim_retired());
    assert!(!handle.control_status().retired_held);
    assert!(!plugin.retired_held());
}

#[test]
fn detached_cancel_defers_when_outbox_full_then_recovers() {
    let bands = vec![
        band("Peak", 1000.0, 1.0, 0.0),
        band("Peak", 3000.0, 1.0, 0.0),
    ];
    let mut plugin = create(&bands);
    let history = pattern(2048);
    let mut history_out = vec![0.0; history.len()];
    process_into(&mut plugin, &history, &mut history_out, 256);
    let handle = detached_handle(&plugin);
    let stale_base = handle.try_accepted_snapshot().expect("base must read");
    // Commit A so the generation-0 base goes stale; render past the blend.
    let first = prepare_on_worker(&stale_base, 0, band("Peak", 1000.0, 1.0, 6.0));
    handle.try_submit(first).expect("first submit must queue");
    let blend_in = pattern(600 + 64);
    let mut blend_out = vec![0.0; blend_in.len()];
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(&mut plugin, &blend_in, &mut blend_out, 256);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(handle.accepted_generation(), 1);
    plugin.reclaim_retired();
    // Wedge two stale heads: both retained slots fill from the mailbox.
    let stale1 = prepare_on_worker(&stale_base, 1, band("Peak", 3000.0, 1.0, -6.0));
    let stale2 = prepare_on_worker(&stale_base, 1, band("Peak", 3000.0, 1.0, -12.0));
    handle.try_submit(stale1).expect("stale submit must queue");
    handle.try_submit(stale2).expect("stale submit must queue");
    let probe_in = pattern(64);
    let mut probe_out = vec![0.0; probe_in.len()];
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(&mut plugin, &probe_in, &mut probe_out, 64);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(handle.control_status().retained_queued, 2);
    assert_eq!(
        handle.control_status().last_refusal,
        Some(CommitRefusal::StaleBase)
    );
    // First cancel evicts the head; the queued sibling promotes and refuses.
    handle.request_cancel();
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(&mut plugin, &probe_in, &mut probe_out, 64);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(handle.control_status().retained_queued, 1);
    assert_eq!(
        handle.control_status().last_refusal,
        Some(CommitRefusal::StaleBase)
    );
    // Second cancel evicts the sibling; the two-slot outbox is now full.
    handle.request_cancel();
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(&mut plugin, &probe_in, &mut probe_out, 64);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(handle.control_status().retained_queued, 0);
    assert_eq!(handle.control_status().last_refusal, None);
    // Wedge a third stale head, then cancel into the full outbox: the
    // eviction defers without dropping; slot and refusal stay put.
    let stale3 = prepare_on_worker(&stale_base, 1, band("Peak", 3000.0, 1.0, -3.0));
    handle.try_submit(stale3).expect("stale submit must queue");
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(&mut plugin, &probe_in, &mut probe_out, 64);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(handle.control_status().retained_queued, 1);
    handle.request_cancel();
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(&mut plugin, &probe_in, &mut probe_out, 64);
    });
    assert_eq!((allocs, frees), (0, 0));
    let deferred = handle.control_status();
    assert_eq!(deferred.retained_queued, 1);
    assert_eq!(deferred.last_refusal, Some(CommitRefusal::StaleBase));
    assert_eq!(plugin.last_refusal(), Some(CommitRefusal::StaleBase));
    // Draining the outbox lets the still-pending cancel land on retry.
    let unblocked = handle.take_cancelled();
    assert_eq!(unblocked.len(), 2);
    drop_on_worker(unblocked);
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(&mut plugin, &probe_in, &mut probe_out, 64);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(handle.control_status().retained_queued, 0);
    assert_eq!(handle.control_status().last_refusal, None);
    let evicted = handle.take_cancelled();
    assert_eq!(evicted.len(), 1);
    drop_on_worker(evicted);
    // Fresh reprepare recovers and commits.
    let fresh = handle.try_accepted_snapshot().expect("fresh base must read");
    assert_eq!(fresh.generation, 1);
    let recovered = prepare_on_worker(&fresh, 1, band("Peak", 3000.0, 1.0, -6.0));
    handle.try_submit(recovered).expect("recovery submit must queue");
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(&mut plugin, &probe_in, &mut probe_out, 64);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(handle.accepted_generation(), 2);
    assert_eq!(get_gain(&plugin, 0), 6.0);
    assert_eq!(get_gain(&plugin, 1), -6.0);
}

#[test]
fn detached_concurrent_submitters_bound_loudly() {
    let bands = vec![
        band("Peak", 1000.0, 1.0, 0.0),
        band("Peak", 3000.0, 1.0, 0.0),
    ];
    let mut plugin = create(&bands);
    let history = pattern(2048);
    let mut history_out = vec![0.0; history.len()];
    process_into(&mut plugin, &history, &mut history_out, 256);
    let handle = detached_handle(&plugin);
    let base = handle.try_accepted_snapshot().expect("base must read");
    // Prepare four same-base payloads on the main thread; submit them from
    // four threads behind one barrier start. Exactly the mailbox capacity
    // can queue; every other outcome is a loud full/busy refusal.
    let payloads: Vec<_> = [1.0, 2.0, 3.0, 4.0]
        .into_iter()
        .map(|gain| prepare_on_worker(&base, 0, band("Peak", 1000.0, 1.0, gain)))
        .collect();
    let barrier = Barrier::new(5);
    // Share the barrier by reference: each `move` closure copies the `Copy`
    // reference, so the main thread keeps ownership for its own wait below.
    let shared = &barrier;
    let (outcome_tx, outcome_rx) = mpsc::channel();
    let (ok_count, err_count) = std::thread::scope(|s| {
        for payload in payloads {
            let worker_tx = outcome_tx.clone();
            let worker_handle = Arc::clone(&handle);
            s.spawn(move || {
                shared.wait();
                let outcome = worker_handle.try_submit(payload);
                worker_tx.send(outcome).expect("outcome ack");
            });
        }
        barrier.wait();
        drop(outcome_tx);
        let mut ok_count = 0;
        let mut err_count = 0;
        for outcome in outcome_rx {
            match outcome {
                Ok(()) => ok_count += 1,
                Err(reason) => {
                    err_count += 1;
                    assert!(!reason.is_empty());
                    assert!(
                        reason.contains("full") || reason.contains("busy"),
                        "refusal must name full or busy: {reason}"
                    );
                }
            }
        }
        (ok_count, err_count)
    });
    eprintln!("concurrent submit outcomes: {ok_count} queued, {err_count} refused");
    // The first lock winner always queues into the empty mailbox; capacity
    // bounds the rest. The winning order is intentionally racy.
    assert!((1..=2).contains(&ok_count), "queued: {ok_count}");
    assert_eq!(ok_count + err_count, 4);
    let input = pattern(600 + 64);
    let mut output = vec![0.0; input.len()];
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(&mut plugin, &input, &mut output, 256);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(handle.accepted_generation(), 1);
    assert!([1.0, 2.0, 3.0, 4.0].contains(&get_gain(&plugin, 0)));
    // A same-base sibling can no longer match live once the first commits.
    plugin.reclaim_retired();
    let probe_in = pattern(64);
    let mut probe_out = vec![0.0; probe_in.len()];
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(&mut plugin, &probe_in, &mut probe_out, 64);
    });
    assert_eq!((allocs, frees), (0, 0));
    if ok_count == 2 {
        assert_eq!(
            handle.control_status().last_refusal,
            Some(CommitRefusal::StaleBase)
        );
    } else {
        assert_eq!(handle.control_status().last_refusal, None);
    }
}

#[test]
fn detached_seqlock_soak_never_tears() {
    let bands = vec![
        band("Peak", 1000.0, 1.0, 3.0),
        band("Peak", 3000.0, 1.0, -3.0),
    ];
    let mut plugin = create(&bands);
    let handle = detached_handle(&plugin);
    // One writer thread republishes the rate while the main thread reads
    // the accepted snapshot in a tight loop. Every observed read must be a
    // consistent published state; contention only ever surfaces as `Busy`.
    const PUBLISHES: u64 = 200;
    let done = AtomicBool::new(false);
    let mut read_count = 0u64;
    let mut busy_count = 0u64;
    let mut last_generation = 0u64;
    std::thread::scope(|s| {
        s.spawn(|| {
            for round in 0..PUBLISHES {
                let rate = if round % 2 == 0 { 44_100 } else { RATE };
                Plugin::initialize(&mut plugin, f64::from(rate)).expect("republish must succeed");
            }
            done.store(true, Ordering::Release);
        });
        while !done.load(Ordering::Acquire) {
            match handle.try_accepted_snapshot() {
                Ok(accepted) => {
                    read_count += 1;
                    assert!(accepted.generation <= PUBLISHES);
                    assert!(accepted.generation >= last_generation);
                    last_generation = accepted.generation;
                    assert!(
                        accepted.snapshot.sample_rate == 44_100.0
                            || accepted.snapshot.sample_rate == f64::from(RATE)
                    );
                    assert_eq!(accepted.snapshot.bands[0].gain_db, 3.0);
                    assert_eq!(accepted.snapshot.bands[1].gain_db, -3.0);
                }
                Err(_) => busy_count += 1,
            }
        }
    });
    // Settled reads after the writer finished are exact and deterministic.
    for _ in 0..4 {
        let accepted = handle
            .try_accepted_snapshot()
            .expect("settled read must succeed");
        assert_eq!(accepted.generation, PUBLISHES);
        assert_eq!(accepted.snapshot.sample_rate, f64::from(RATE));
    }
    assert_eq!(handle.accepted_generation(), PUBLISHES);
    eprintln!("seqlock soak: {read_count} consistent reads, {busy_count} busy retries");
    assert!(read_count > 0);
}
