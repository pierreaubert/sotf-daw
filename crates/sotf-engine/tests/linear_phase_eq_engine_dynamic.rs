// Running-engine integration for linear-phase EQ dynamic updates.
//
// Renders through a running `EmbeddedAudioEngine` built from the real facade
// factory (`sotf_plugins::create_plugin` inside engine host build). Control
// snapshots on engine, workers prepare off audio, the shared `Arc` handle
// delivers retained slots to wrapper `process`; retired banks reclaim off
// audio on control or worker threads. Worker sync uses `Barrier` plus `mpsc`
// ack only. Audio sections assert strict `(0, 0)` alloc/free for the f32
// render path; the f64 bridge and compiled-op decline are documented in the
// wrapper and not covered here. No test calls DSP commit APIs; commits run
// inside engine renders.
//
// Native (`plugins-nih`) and FFI (`plugins-ffi`) ordinary setters stay
// structural until their own adoption. M1 accuracy stays open separately.

use sotf_audio::{EmbeddedAudioEngine, EngineConfig, PluginConfig};
use sotf_plugins::ParameterValue;
use sotf_plugins::plugin_linear_phase_eq::{BandConfig, CommitRefusal, LinearPhaseEqPlugin};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::{Barrier, mpsc};

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

fn count_allocs<R>(f: impl FnOnce() -> R) -> ((usize, usize), R) {
    COUNTS.set((0, 0));
    COUNTING.set(true);
    let value = f();
    COUNTING.set(false);
    (COUNTS.get(), value)
}

const RATE: u32 = 48_000;
const CHANNELS: usize = 2;
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

fn eq_parameters(bands: &[BandConfig], mix: f32) -> serde_json::Value {
    let filters: Vec<serde_json::Value> = bands
        .iter()
        .map(|b| {
            serde_json::json!({
                "filter_type": b.filter_type,
                "frequency": b.frequency,
                "q": b.q,
                "gain_db": b.gain_db,
                "active": b.active,
            })
        })
        .collect();
    serde_json::json!({
        "num_filters": bands.len(),
        "fir_length_index": 0,
        "phase_mode_index": 0,
        "auto_gain": false,
        "mix": mix,
        "filters": filters,
    })
}

fn pattern(frames: usize) -> Vec<f32> {
    (0..frames * CHANNELS)
        .map(|i| ((i * 7919 % 104729) as f32 / 104729.0 - 0.5) * 0.9)
        .collect()
}

fn engine_with(bands: &[BandConfig], mix: f32) -> EmbeddedAudioEngine {
    let config = EngineConfig {
        frame_size: 2048,
        output_sample_rate: RATE,
        input_channels: CHANNELS,
        plugins: vec![PluginConfig::new(
            "linear_phase_eq",
            eq_parameters(bands, mix),
        )],
        ..EngineConfig::default()
    };
    let (engine, diagnostics) = EmbeddedAudioEngine::new(&config).expect("engine build");
    assert!(diagnostics.is_empty());
    engine
}

fn render_into(
    engine: &mut EmbeddedAudioEngine,
    input: &[f32],
    output: &mut [f32],
    block: usize,
    start_sample: &mut u64,
) {
    assert_eq!(input.len(), output.len());
    let frames = input.len() / CHANNELS;
    let mut position = 0;
    while position < frames {
        let count = block.min(frames - position);
        let range = position * CHANNELS..(position + count) * CHANNELS;
        let done = engine
            .process_at(*start_sample, &input[range.clone()], &mut output[range])
            .expect("engine render");
        assert_eq!(done, count);
        *start_sample += count as u64;
        position += count;
    }
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

#[test]
fn engine_static_parity_and_generic_refusal() {
    let bands = vec![
        band("Peak", 1000.0, 1.0, 0.0),
        band("Peak", 3000.0, 1.0, 0.0),
    ];
    let mut engine = engine_with(&bands, 1.0);
    assert_eq!(engine.latency_samples(), 512 + 32);
    // Generic band automation through the engine setter stays structural.
    let err = engine
        .set_plugin_parameter_at(0, "band_0_gain", ParameterValue::Float(6.0), 0)
        .expect_err("generic band automation must stay structural");
    assert!(err.contains("rebuilding"), "{err}");
    let input = pattern(2048);
    let mut output = vec![0.0; input.len()];
    let mut clock = 0;
    let ((allocs, frees), ()) = count_allocs(|| {
        render_into(&mut engine, &input, &mut output, 256, &mut clock);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert!(rms(&output) > 1e-4);
    assert_eq!(engine.linear_phase_eq_band_gain(0, 0).unwrap(), 0.0);
}

fn run_engine_automation_once(block: usize) -> Vec<f32> {
    let old_bands = vec![
        band("Peak", 1000.0, 1.0, 0.0),
        band("Peak", 3000.0, 1.0, 0.0),
    ];
    let total = 3000;
    let commit_at = 2048;
    let input = pattern(total);
    let mut engine = engine_with(&old_bands, 1.0);
    let latency_before = engine.latency_samples();
    let mut output = vec![0.0; input.len()];
    let mut clock = 0;
    render_into(
        &mut engine,
        &input[..commit_at * CHANNELS],
        &mut output[..commit_at * CHANNELS],
        block,
        &mut clock,
    );
    assert!(rms(&output[..commit_at * CHANNELS]) > 1e-4);
    assert_eq!(engine.linear_phase_eq_band_gain(0, 0).unwrap(), 0.0);
    // Worker prepares from an engine snapshot; barrier plus ack sync.
    let base = engine.linear_phase_eq_snapshot(0).unwrap();
    let handle = engine.linear_phase_eq_handle(0).unwrap();
    let barrier = Barrier::new(2);
    let (tx, rx) = mpsc::channel();
    std::thread::scope(|s| {
        let barrier_ref = &barrier;
        s.spawn(move || {
            let prepared =
                LinearPhaseEqPlugin::prepare_band_update(&base, 0, band("Peak", 1000.0, 1.0, 9.0))
                    .expect("prepare");
            tx.send(prepared).expect("ack");
            barrier_ref.wait();
        });
        let prepared = rx.recv().expect("worker ack");
        handle.try_submit(prepared).expect("submit via handle");
        barrier.wait();
    });
    assert_eq!(engine.linear_phase_eq_band_gain(0, 0).unwrap(), 0.0);
    let ((allocs, frees), ()) = count_allocs(|| {
        render_into(
            &mut engine,
            &input[commit_at * CHANNELS..],
            &mut output[commit_at * CHANNELS..],
            block,
            &mut clock,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(engine.linear_phase_eq_band_gain(0, 0).unwrap(), 9.0);
    assert_eq!(engine.latency_samples(), latency_before);
    assert!(rms(&output[commit_at * CHANNELS..]) > 1e-4);
    output
}

#[test]
fn engine_gain_automation_exact_zero_final_and_partitions() {
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
    let output = run_engine_automation_once(256);
    let mut old_engine = engine_with(&old_bands, 1.0);
    let mut new_engine = engine_with(&new_bands, 1.0);
    let mut old_out = vec![0.0; input.len()];
    let mut new_out = vec![0.0; input.len()];
    let mut clock = 0;
    render_into(&mut old_engine, &input, &mut old_out, 256, &mut clock);
    clock = 0;
    render_into(&mut new_engine, &input, &mut new_out, 256, &mut clock);
    for ch in 0..CHANNELS {
        let index = commit_at * CHANNELS + ch;
        assert_eq!(output[index], old_out[index], "exact-0 blend start ch{ch}");
    }
    for frame in commit_at..total {
        let j = frame - commit_at;
        let w = if j <= XFADE_FRAMES {
            j as f64 / XFADE_FRAMES as f64
        } else {
            1.0
        };
        for ch in 0..CHANNELS {
            let index = frame * CHANNELS + ch;
            let expected = f64::from(old_out[index]) * (1.0 - w) + f64::from(new_out[index]) * w;
            assert!(
                (f64::from(output[index]) - expected).abs() < 1e-5,
                "frame{frame} ch{ch}"
            );
        }
    }
    let reference = run_engine_automation_once(1);
    assert_eq!(output, reference);
    for block in [63, 512] {
        assert_eq!(
            run_engine_automation_once(block),
            reference,
            "block {block}"
        );
    }
}

#[test]
fn engine_second_edit_during_blend_eventually_applied() {
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
    let mut engine = engine_with(&old_bands, 1.0);
    let mut output = vec![0.0; input.len()];
    let mut clock = 0;
    render_into(
        &mut engine,
        &input[..first_at * CHANNELS],
        &mut output[..first_at * CHANNELS],
        256,
        &mut clock,
    );
    engine
        .linear_phase_eq_request(0, 0, band("Peak", 1000.0, 1.0, -6.0))
        .unwrap();
    let ((allocs, frees), ()) = count_allocs(|| {
        render_into(
            &mut engine,
            &input[first_at * CHANNELS..second_at * CHANNELS],
            &mut output[first_at * CHANNELS..second_at * CHANNELS],
            128,
            &mut clock,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(engine.linear_phase_eq_band_gain(0, 0).unwrap(), -6.0);
    // Second edit during the first blend via worker plus shared handle.
    let base = engine.linear_phase_eq_snapshot(0).unwrap();
    let handle = engine.linear_phase_eq_handle(0).unwrap();
    let barrier = Barrier::new(2);
    let (tx, rx) = mpsc::channel();
    std::thread::scope(|s| {
        let barrier_ref = &barrier;
        s.spawn(move || {
            let prepared = LinearPhaseEqPlugin::prepare_band_update(
                &base,
                0,
                band("Peak", 1000.0, 1.0, -12.0),
            )
            .expect("prepare second");
            tx.send(prepared).expect("ack");
            barrier_ref.wait();
        });
        let prepared = rx.recv().expect("worker ack");
        handle.try_submit(prepared).expect("submit second");
        barrier.wait();
    });
    let ((allocs, frees), ()) = count_allocs(|| {
        render_into(
            &mut engine,
            &input[second_at * CHANNELS..],
            &mut output[second_at * CHANNELS..],
            256,
            &mut clock,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    // Reclaim on a worker thread via the shared handle (barrier plus ack).
    let handle = engine.linear_phase_eq_handle(0).unwrap();
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
        assert!(reclaimed > 0, "blend must have retired banks");
        barrier.wait();
    });
    let tail_in = pattern(1200);
    let mut tail_out = vec![0.0; tail_in.len()];
    let ((allocs, frees), ()) = count_allocs(|| {
        render_into(&mut engine, &tail_in, &mut tail_out, 256, &mut clock);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(engine.linear_phase_eq_band_gain(0, 0).unwrap(), -12.0);
    let mut final_engine = engine_with(&final_bands, 1.0);
    let mut final_out = vec![0.0; input.len()];
    let mut final_clock = 0;
    render_into(
        &mut final_engine,
        &input,
        &mut final_out,
        256,
        &mut final_clock,
    );
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
fn engine_stale_topology_and_reset_retain_accepted() {
    let old_bands = vec![
        band("Peak", 1000.0, 1.0, 0.0),
        band("Peak", 3000.0, 1.0, 0.0),
    ];
    let mut engine = engine_with(&old_bands, 1.0);
    let input = pattern(2700);
    let mut output = vec![0.0; input.len()];
    let mut clock = 0;
    render_into(
        &mut engine,
        &input[..2048 * CHANNELS],
        &mut output[..2048 * CHANNELS],
        256,
        &mut clock,
    );
    let stale_base = engine.linear_phase_eq_snapshot(0).unwrap();
    engine
        .linear_phase_eq_request(0, 0, band("Peak", 1000.0, 1.0, 6.0))
        .unwrap();
    let ((allocs, frees), ()) = count_allocs(|| {
        render_into(
            &mut engine,
            &input[2048 * CHANNELS..2100 * CHANNELS],
            &mut output[2048 * CHANNELS..2100 * CHANNELS],
            52,
            &mut clock,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(engine.linear_phase_eq_band_gain(0, 0).unwrap(), 6.0);
    // Stale second edit from the pre-first-commit base via worker.
    let handle = engine.linear_phase_eq_handle(0).unwrap();
    let barrier = Barrier::new(2);
    let (tx, rx) = mpsc::channel();
    std::thread::scope(|s| {
        let barrier_ref = &barrier;
        s.spawn(move || {
            let prepared = LinearPhaseEqPlugin::prepare_band_update(
                &stale_base,
                1,
                band("Peak", 3000.0, 1.0, -6.0),
            )
            .expect("stale prepare");
            tx.send(prepared).expect("ack");
            barrier_ref.wait();
        });
        let prepared = rx.recv().expect("worker ack");
        handle.try_submit(prepared).expect("submit stale");
        barrier.wait();
    });
    // Run past the 513-frame first blend, reclaim, then the stale edit still
    // refuses with live getters retained.
    let ((allocs, frees), ()) = count_allocs(|| {
        render_into(
            &mut engine,
            &input[2100 * CHANNELS..],
            &mut output[2100 * CHANNELS..],
            256,
            &mut clock,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    engine.linear_phase_eq_reclaim(0);
    let probe_in = pattern(128);
    let mut probe_out = vec![0.0; probe_in.len()];
    let ((allocs, frees), ()) = count_allocs(|| {
        render_into(&mut engine, &probe_in, &mut probe_out, 128, &mut clock);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(engine.linear_phase_eq_band_gain(0, 0).unwrap(), 6.0);
    assert_eq!(engine.linear_phase_eq_band_gain(0, 1).unwrap(), 0.0);
    // Fresh-after-stale recovery through the engine handle cancel path is
    // proven by `engine_stale_head_cancel_recovers_and_applies_fresh` below,
    // so no fresh submit is attempted in this retention-focused leg.
    // Topology refusal at prepare time keeps live intact (engine request).
    let mut placed = band("Peak", 1000.0, 1.0, 6.0);
    placed.placement = Some(sotf_plugins::plugin_linear_phase_eq::LinearPhaseEqBandPlacement::Left);
    let err = engine
        .linear_phase_eq_request(0, 0, placed)
        .expect_err("placement must stay structural");
    assert!(
        err.contains("structural") || err.contains("placement"),
        "{err}"
    );
    assert_eq!(engine.linear_phase_eq_band_gain(0, 0).unwrap(), 6.0);
    // Transport reset preserves accepted gains and stays usable.
    engine.reset_transport(clock);
    assert_eq!(engine.linear_phase_eq_band_gain(0, 0).unwrap(), 6.0);
    let ((allocs, frees), ()) = count_allocs(|| {
        render_into(&mut engine, &probe_in, &mut probe_out, 64, &mut clock);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert!(rms(&probe_out) > 0.0);
}

#[test]
fn engine_queue_saturation_dry_alignment_and_reclaim_tracking() {
    let old_bands = vec![
        band("Peak", 1000.0, 1.0, 0.0),
        band("Peak", 3000.0, 1.0, 0.0),
    ];
    let mut engine = engine_with(&old_bands, 1.0);
    let input = pattern(2700);
    let mut output = vec![0.0; input.len()];
    let mut clock = 0;
    render_into(
        &mut engine,
        &input[..2048 * CHANNELS],
        &mut output[..2048 * CHANNELS],
        256,
        &mut clock,
    );
    // Fill the two-slot handle mailbox without rendering; third fails full.
    let base = engine.linear_phase_eq_snapshot(0).unwrap();
    let handle = engine.linear_phase_eq_handle(0).unwrap();
    for gain in [4.0, 5.0] {
        let prepared =
            LinearPhaseEqPlugin::prepare_band_update(&base, 0, band("Peak", 1000.0, 1.0, gain))
                .unwrap();
        handle.try_submit(prepared).expect("mailbox room");
    }
    let overflow =
        LinearPhaseEqPlugin::prepare_band_update(&base, 0, band("Peak", 1000.0, 1.0, 6.0)).unwrap();
    let err = handle
        .try_submit(overflow)
        .expect_err("third concurrent submit must report full");
    assert!(err.contains("full"), "{err}");
    assert_eq!(engine.linear_phase_eq_band_gain(0, 0).unwrap(), 0.0);
    let ((allocs, frees), ()) = count_allocs(|| {
        render_into(
            &mut engine,
            &input[2048 * CHANNELS..2112 * CHANNELS],
            &mut output[2048 * CHANNELS..2112 * CHANNELS],
            64,
            &mut clock,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(engine.linear_phase_eq_band_gain(0, 0).unwrap(), 4.0);
    let ((allocs, frees), ()) = count_allocs(|| {
        render_into(
            &mut engine,
            &input[2112 * CHANNELS..],
            &mut output[2112 * CHANNELS..],
            256,
            &mut clock,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    // Worker-thread reclaim of completed blends.
    let handle = engine.linear_phase_eq_handle(0).unwrap();
    let barrier = Barrier::new(2);
    let (ack_tx, ack_rx) = mpsc::channel();
    std::thread::scope(|s| {
        let barrier_ref = &barrier;
        s.spawn(move || {
            let reclaimed = handle.try_reclaim();
            ack_tx.send(reclaimed).expect("reclaim ack");
            barrier_ref.wait();
        });
        let _ = ack_rx.recv().expect("reclaim ack");
        barrier.wait();
    });
    assert_eq!(engine.linear_phase_eq_band_gain(0, 0).unwrap(), 4.0);
    // Dry alignment: mix-0 engine output equals delayed input exactly.
    let mut dry = engine_with(&old_bands, 0.0);
    let latency = dry.latency_samples();
    let total = 2000;
    let dry_in = pattern(total);
    let mut dry_out = vec![0.0; dry_in.len()];
    let mut dry_clock = 0;
    render_into(
        &mut dry,
        &dry_in[..500 * CHANNELS],
        &mut dry_out[..500 * CHANNELS],
        63,
        &mut dry_clock,
    );
    dry.linear_phase_eq_request(0, 0, band("Peak", 1000.0, 1.0, 12.0))
        .unwrap();
    let ((allocs, frees), ()) = count_allocs(|| {
        render_into(
            &mut dry,
            &dry_in[500 * CHANNELS..],
            &mut dry_out[500 * CHANNELS..],
            512,
            &mut dry_clock,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(dry.latency_samples(), latency);
    let mut expected = vec![0.0; dry_out.len()];
    for frame in latency..total {
        for ch in 0..CHANNELS {
            expected[frame * CHANNELS + ch] = dry_in[(frame - latency) * CHANNELS + ch];
        }
    }
    assert_eq!(dry_out, expected);
}

#[test]
fn engine_stale_head_cancel_recovers_and_applies_fresh() {
    let old_bands = vec![
        band("Peak", 1000.0, 1.0, 0.0),
        band("Peak", 3000.0, 1.0, 0.0),
    ];
    let final_bands = vec![
        band("Peak", 1000.0, 1.0, 6.0),
        band("Peak", 3000.0, 1.0, -6.0),
    ];
    // Contiguous frame plan keeps twin histories identical: history 0..2048,
    // first edit at 2048 (blend 2048..2561), stale wedge observed at 2700,
    // fresh queued at 2764, cancel plus fresh commit at 2828 (blend to
    // 3341), twin comparison over the settled tail 3341..3600.
    let mut engine = engine_with(&old_bands, 1.0);
    let input = pattern(3600);
    let mut output = vec![0.0; input.len()];
    let mut clock = 0;
    render_into(
        &mut engine,
        &input[..2048 * CHANNELS],
        &mut output[..2048 * CHANNELS],
        256,
        &mut clock,
    );
    assert!(rms(&output[..2048 * CHANNELS]) > 1e-4);
    // Commit a first edit, then wedge the head with a stale second edit.
    let stale_base = engine.linear_phase_eq_snapshot(0).unwrap();
    engine
        .linear_phase_eq_request(0, 0, band("Peak", 1000.0, 1.0, 6.0))
        .unwrap();
    let ((allocs, frees), ()) = count_allocs(|| {
        render_into(
            &mut engine,
            &input[2048 * CHANNELS..2100 * CHANNELS],
            &mut output[2048 * CHANNELS..2100 * CHANNELS],
            52,
            &mut clock,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(engine.linear_phase_eq_band_gain(0, 0).unwrap(), 6.0);
    let handle = engine.linear_phase_eq_handle(0).unwrap();
    let barrier = Barrier::new(2);
    let (tx, rx) = mpsc::channel();
    std::thread::scope(|s| {
        let barrier_ref = &barrier;
        s.spawn(move || {
            let prepared = LinearPhaseEqPlugin::prepare_band_update(
                &stale_base,
                1,
                band("Peak", 3000.0, 1.0, -6.0),
            )
            .expect("stale prepare");
            tx.send(prepared).expect("ack");
            barrier_ref.wait();
        });
        let prepared = rx.recv().expect("worker ack");
        handle.try_submit(prepared).expect("submit stale");
        barrier.wait();
    });
    // Run past the 513-frame first blend; the stale head refuses permanently.
    let ((allocs, frees), ()) = count_allocs(|| {
        render_into(
            &mut engine,
            &input[2100 * CHANNELS..2700 * CHANNELS],
            &mut output[2100 * CHANNELS..2700 * CHANNELS],
            256,
            &mut clock,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    engine.linear_phase_eq_reclaim(0);
    let ((allocs, frees), ()) = count_allocs(|| {
        render_into(
            &mut engine,
            &input[2700 * CHANNELS..2764 * CHANNELS],
            &mut output[2700 * CHANNELS..2764 * CHANNELS],
            64,
            &mut clock,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(
        engine.linear_phase_eq_last_refusal(0),
        Some(CommitRefusal::StaleBase)
    );
    assert_eq!(engine.linear_phase_eq_pending_len(0), 1);
    assert_eq!(engine.linear_phase_eq_band_gain(0, 0).unwrap(), 6.0);
    assert_eq!(engine.linear_phase_eq_band_gain(0, 1).unwrap(), 0.0);
    assert!(rms(&output[2700 * CHANNELS..2764 * CHANNELS]) > 1e-4);
    // Queue a fresh edit behind the wedged stale head.
    let fresh_base = engine.linear_phase_eq_snapshot(0).unwrap();
    let handle = engine.linear_phase_eq_handle(0).unwrap();
    let barrier = Barrier::new(2);
    let (tx, rx) = mpsc::channel();
    std::thread::scope(|s| {
        let barrier_ref = &barrier;
        s.spawn(move || {
            let prepared = LinearPhaseEqPlugin::prepare_band_update(
                &fresh_base,
                1,
                band("Peak", 3000.0, 1.0, -6.0),
            )
            .expect("fresh prepare");
            tx.send(prepared).expect("ack");
            barrier_ref.wait();
        });
        let prepared = rx.recv().expect("worker ack");
        handle.try_submit(prepared).expect("submit fresh");
        barrier.wait();
    });
    let ((allocs, frees), ()) = count_allocs(|| {
        render_into(
            &mut engine,
            &input[2764 * CHANNELS..2828 * CHANNELS],
            &mut output[2764 * CHANNELS..2828 * CHANNELS],
            64,
            &mut clock,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(engine.linear_phase_eq_pending_len(0), 2);
    assert_eq!(
        engine.linear_phase_eq_last_refusal(0),
        Some(CommitRefusal::StaleBase)
    );
    // Explicit head cancel evicts only the stale payload; the fresh queued
    // edit promotes and commits in the same quantum with history preserved.
    assert!(engine.linear_phase_eq_request_cancel(0));
    let ((allocs, frees), ()) = count_allocs(|| {
        render_into(
            &mut engine,
            &input[2828 * CHANNELS..2892 * CHANNELS],
            &mut output[2828 * CHANNELS..2892 * CHANNELS],
            64,
            &mut clock,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(engine.linear_phase_eq_pending_len(0), 0);
    assert_eq!(engine.linear_phase_eq_last_refusal(0), None);
    assert_eq!(engine.linear_phase_eq_band_gain(0, 0).unwrap(), 6.0);
    assert_eq!(engine.linear_phase_eq_band_gain(0, 1).unwrap(), -6.0);
    assert!(rms(&output[2828 * CHANNELS..2892 * CHANNELS]) > 1e-4);
    // The evicted stale payload is destroyed on a worker thread.
    let handle = engine.linear_phase_eq_handle(0).unwrap();
    let cancelled = handle.take_cancelled();
    assert_eq!(cancelled.len(), 1, "exactly the stale head evicts");
    let barrier = Barrier::new(2);
    let (ack_tx, ack_rx) = mpsc::channel();
    std::thread::scope(|s| {
        let barrier_ref = &barrier;
        s.spawn(move || {
            drop(cancelled);
            ack_tx.send(()).expect("cancel reclaim ack");
            barrier_ref.wait();
        });
        ack_rx.recv().expect("cancel reclaim ack");
        barrier.wait();
    });
    assert_eq!(engine.linear_phase_eq_reclaim_cancelled(0), 0);
    // Render past the fresh blend; settled audio matches a from-scratch
    // final twin over identical history.
    let ((allocs, frees), ()) = count_allocs(|| {
        render_into(
            &mut engine,
            &input[2892 * CHANNELS..],
            &mut output[2892 * CHANNELS..],
            256,
            &mut clock,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    engine.linear_phase_eq_reclaim(0);
    let mut final_engine = engine_with(&final_bands, 1.0);
    let mut final_out = vec![0.0; input.len()];
    let mut final_clock = 0;
    render_into(
        &mut final_engine,
        &input,
        &mut final_out,
        256,
        &mut final_clock,
    );
    for frame in 3341..3600 {
        for ch in 0..CHANNELS {
            let index = frame * CHANNELS + ch;
            assert!(
                (f64::from(output[index]) - f64::from(final_out[index])).abs() < 1e-5,
                "frame{frame} ch{ch}"
            );
        }
    }
    assert!(rms(&output[2892 * CHANNELS..]) > 1e-4);
    // Post-recovery wrapper state is indistinguishable from a normal success
    // (empty slots, completed blend, reclaimed retirement), so EOF follows
    // the identical drain path already proven for dynamic updates via the
    // host and bridge suites; the engine exposes no drain API to repeat it.
}
