// Focused bridge host tests for linear-phase EQ dynamic updates.
//
// Real factory (`plugins_bridge::create_plugin`) plus real plugin/host
// interface (`Plugin::process` trait object, `DawHost` for static parity).
// Worker preparation and retired-bank reclamation run on spawned threads,
// synchronized by `Barrier` plus `mpsc` ack, never sleep. Audio sections use
// a thread-local counting allocator and assert strict `(0, 0)` alloc/free,
// including commit success, transient/stale/drained refusals, full queue and
// reset. No test calls DSP commit APIs directly; commits run inside wrapper
// `process`, so this is not a direct-DSP mirror.
//
// Shared-wrapper adoption: bridge, facade, host, and running-engine routes
// all construct `LinearPhaseEqDynamicPlugin` (bridge re-exports the plugin
// crate owner). Remaining structural paths (preserved, named explicitly):
// `DawHost` generic automation (`set_plugin_parameter_at`,
// `ParameterEventSender`), FFI (`plugins-ffi` mapping/factory), native
// (`plugins-nih` wrapper), and the multi-threaded `AudioEngine` manager path
// (only `EmbeddedAudioEngine` gained narrow control methods).
//
// Numerical accuracy M1 (96 kHz / 1024 taps / 500 Hz multiband, 0.05 dB)
// stays open separately; these tests use 48 kHz passing-case bands only.

use plugins_bridge::create_plugin;
use plugins_bridge::linear_phase_eq_dynamic::LinearPhaseEqDynamicPlugin;
use sotf_host::plugin::{Plugin, ProcessContext, TailLength};
use sotf_host::{DawHost, ParameterId, ParameterValue};
use sotf_plugin_linear_phase_eq::{BandConfig, CommitRefusal, LinearPhaseEqPlugin};
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
// Mirrors the DSP private `XFADE_FRAMES` (verified 512 from current source);
// the blend spans `XFADE_FRAMES + 1` frames with exact 0/1 endpoints.
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

fn config_json(bands: &[BandConfig], mix: f32) -> String {
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
    .to_string()
}

fn pattern(frames: usize) -> Vec<f32> {
    (0..frames * CHANNELS)
        .map(|i| ((i * 7919 % 104729) as f32 / 104729.0 - 0.5) * 0.9)
        .collect()
}

fn create(bands: &[BandConfig], mix: f32) -> Box<dyn Plugin> {
    let mut plugin = create_plugin("LinearPhaseEQ", CHANNELS, f64::from(RATE), &config_json(bands, mix))
        .expect("bridge factory must create LinearPhaseEQ");
    plugin.initialize(f64::from(RATE)).expect("initialize must succeed");
    plugin
}

fn as_dynamic(plugin: &mut dyn Plugin) -> &mut LinearPhaseEqDynamicPlugin {
    plugin
        .as_any_mut()
        .and_then(|any| any.downcast_mut::<LinearPhaseEqDynamicPlugin>())
        .expect("factory must return the dynamic wrapper")
}

fn get_gain(plugin: &dyn Plugin, band: usize) -> f32 {
    match plugin.get_parameter(&ParameterId::from(format!("band_{band}_gain").as_str())) {
        Some(ParameterValue::Float(gain)) => gain,
        other => panic!("band_{band}_gain must read as Float, got {other:?}"),
    }
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
    (sum / samples.len() as f64).sqrt() as f32
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

#[test]
fn factory_static_parity_and_generic_refusal_preserved() {
    let bands = vec![
        band("Peak", 1000.0, 1.0, 0.0),
        band("Peak", 3000.0, 1.0, 0.0),
    ];
    let mut plugin = create(&bands, 1.0);
    // Factory returns the dynamic wrapper behind `Box<dyn Plugin>`.
    assert!(plugin.as_any().is_some());
    assert_eq!(plugin.input_channels(), CHANNELS);
    assert_eq!(plugin.output_channels(), CHANNELS);
    // Static latency matches the long-established 1024-tap linear value:
    // N/2 plus the 32-sample streaming head.
    assert_eq!(plugin.latency_samples(), 512 + 32);
    assert!(matches!(plugin.tail_length(), TailLength::Finite(_)));
    // Parameter count matches DSP schema (5 global plus 6 per band).
    assert_eq!(plugin.parameters().len(), 5 + bands.len() * 6);
    assert_eq!(get_gain(&*plugin, 0), 0.0);
    // Static audio through the wrapper is allocation-free and finite.
    let input = pattern(2048);
    let mut output = vec![0.0; input.len()];
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(&mut *plugin, &input, &mut output, 64);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert!(output.iter().all(|s| s.is_finite()));
    assert!(rms(&output) > 1e-4, "static output must be nonzero");
    // Generic band automation still refuses structurally (preserved).
    let err = plugin
        .set_parameter(
            ParameterId::from("band_0_gain"),
            ParameterValue::Float(6.0),
        )
        .expect_err("generic band automation must stay structural");
    assert!(err.contains("structural"), "{err}");
    assert_eq!(get_gain(&*plugin, 0), 0.0);
    // `DawHost` automatable validation refuses the same control.
    let mut host = DawHost::new(CHANNELS, RATE);
    host.add_plugin(create(&bands, 1.0)).unwrap();
    host.build().unwrap();
    let err = host
        .validate_automatable_plugin_parameter(0, "band_0_gain", &ParameterValue::Float(6.0))
        .expect_err("host automation must stay structural for bands");
    assert!(err.contains("rebuilding"), "{err}");
    // Host static render stays finite and nonzero.
    let mut host_out = vec![0.0; input.len()];
    host.process(&input, &mut host_out).unwrap();
    assert!(host_out.iter().all(|s| s.is_finite()));
    assert!(rms(&host_out) > 1e-4);
}

fn run_automation_once(block: usize) -> Vec<f32> {
    let old_bands = vec![
        band("Peak", 1000.0, 1.0, 0.0),
        band("Peak", 3000.0, 1.0, 0.0),
    ];
    let new_gain = band("Peak", 1000.0, 1.0, 9.0);
    let total = 3000;
    let commit_at = 2048;
    let input = pattern(total);
    let mut plugin = create(&old_bands, 1.0);
    let latency_before = plugin.latency_samples();
    // Populated history beyond full support (1055 frames at 1024 taps).
    let mut output = vec![0.0; input.len()];
    process_into(
        &mut *plugin,
        &input[..commit_at * CHANNELS],
        &mut output[..commit_at * CHANNELS],
        block,
    );
    assert!(rms(&output[..commit_at * CHANNELS]) > 1e-4);
    // Getters still show the accepted old gain before commit.
    assert_eq!(get_gain(&*plugin, 0), 0.0);
    // Worker prepares off audio; barrier plus mpsc ack synchronize.
    let base = as_dynamic(&mut *plugin).snapshot_for_update();
    let barrier = Barrier::new(2);
    let (tx, rx) = mpsc::channel();
    std::thread::scope(|s| {
        let barrier_ref = &barrier;
        s.spawn(move || {
            let prepared =
                LinearPhaseEqPlugin::prepare_band_update(&base, 0, new_gain).expect("prepare");
            tx.send(prepared).expect("ack send");
            barrier_ref.wait();
        });
        let prepared = rx.recv().expect("worker ack");
        as_dynamic(&mut *plugin)
            .submit_prepared_update(prepared)
            .expect("submit");
        barrier.wait();
    });
    assert_eq!(as_dynamic(&mut *plugin).pending_len(), 1);
    // Getters still old until the audio commit runs.
    assert_eq!(get_gain(&*plugin, 0), 0.0);
    // Commit quantum plus remainder, all counted allocation-free.
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(
            &mut *plugin,
            &input[commit_at * CHANNELS..],
            &mut output[commit_at * CHANNELS..],
            block,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(as_dynamic(&mut *plugin).pending_len(), 0);
    assert!(as_dynamic(&mut *plugin).last_refusal().is_none());
    // Accepted getter flips at commit while audio morphs.
    assert_eq!(get_gain(&*plugin, 0), 9.0);
    assert_eq!(plugin.latency_samples(), latency_before);
    assert!(rms(&output[commit_at * CHANNELS..]) > 1e-4);
    output
}

#[test]
fn worker_gain_automation_exact_zero_and_final_reference() {
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
    let output = run_automation_once(64);
    // Twin references through the same factory path (no direct DSP).
    let mut old_ref = create(&old_bands, 1.0);
    let mut new_ref = create(&new_bands, 1.0);
    let mut old_out = vec![0.0; input.len()];
    let mut new_out = vec![0.0; input.len()];
    process_into(&mut *old_ref, &input, &mut old_out, 64);
    process_into(&mut *new_ref, &input, &mut new_out, 64);
    // Exact-zero initial frame: first post-commit frame equals old exactly.
    for ch in 0..CHANNELS {
        let index = commit_at * CHANNELS + ch;
        assert_eq!(
            output[index], old_out[index],
            "blend must start at exactly 0 (ch{ch})"
        );
    }
    // Blend follows exact weights within the established 1e-5 bound.
    for frame in commit_at..total {
        let j = frame - commit_at;
        let w = if j <= XFADE_FRAMES {
            j as f64 / XFADE_FRAMES as f64
        } else {
            1.0
        };
        for ch in 0..CHANNELS {
            let index = frame * CHANNELS + ch;
            let expected =
                f64::from(old_out[index]) * (1.0 - w) + f64::from(new_out[index]) * w;
            assert!(
                (f64::from(output[index]) - expected).abs() < 1e-5,
                "frame{frame} ch{ch}: {} vs {expected} (w={w})",
                output[index]
            );
        }
    }
    // Partitions are bit-exact for the same commit frame.
    let reference = run_automation_once(1);
    assert_eq!(output, reference);
    for block in [8, 512] {
        assert_eq!(run_automation_once(block), reference, "block {block}");
    }
    // Full EOF after an update: finite tail, exact length, idempotent drain.
    let mut plugin = create(&old_bands, 1.0);
    let mut out = vec![0.0; input.len()];
    process_into(&mut *plugin, &input[..commit_at * CHANNELS], &mut out[..commit_at * CHANNELS], 64);
    as_dynamic(&mut *plugin)
        .request_band_update(0, band("Peak", 1000.0, 1.0, 9.0))
        .unwrap();
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(
            &mut *plugin,
            &input[commit_at * CHANNELS..],
            &mut out[commit_at * CHANNELS..],
            64,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    let support = match plugin.tail_length() {
        TailLength::Finite(frames) => frames as usize,
        _ => panic!("expected a finite tail"),
    };
    let mut tailed = Vec::with_capacity(out.len() + (support + 8) * CHANNELS);
    tailed.extend_from_slice(&out);
    let mut scratch = vec![0.0; 257 * CHANNELS];
    let ((allocs, frees), ()) = count_allocs(|| {
        for _ in 0..20000 {
            let drained = plugin
                .drain(&mut scratch, &ProcessContext::new(RATE, 0))
                .unwrap();
            tailed.extend_from_slice(&scratch[..drained.frames * CHANNELS]);
            if drained.complete {
                break;
            }
        }
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(tailed.len(), (total + support) * CHANNELS);
    // Second drain is complete and idempotent.
    let mut empty = vec![0.0; CHANNELS];
    let second = plugin
        .drain(&mut empty, &ProcessContext::new(RATE, 0))
        .unwrap();
    assert!(second.complete);
}

#[test]
fn two_edits_during_blend_eventually_applied_after_retirement() {
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
    let mut plugin = create(&old_bands, 1.0);
    let mut output = vec![0.0; input.len()];
    process_into(
        &mut *plugin,
        &input[..first_at * CHANNELS],
        &mut output[..first_at * CHANNELS],
        64,
    );
    // First edit via worker, committed on the next quantum.
    let base = as_dynamic(&mut *plugin).snapshot_for_update();
    let barrier = Barrier::new(2);
    let (tx, rx) = mpsc::channel();
    std::thread::scope(|s| {
        let barrier_ref = &barrier;
        s.spawn(move || {
            let prepared = LinearPhaseEqPlugin::prepare_band_update(
                &base,
                0,
                band("Peak", 1000.0, 1.0, -6.0),
            )
            .expect("prepare first");
            tx.send(prepared).expect("ack");
            barrier_ref.wait();
        });
        let prepared = rx.recv().expect("worker ack");
        as_dynamic(&mut *plugin)
            .submit_prepared_update(prepared)
            .unwrap();
        barrier.wait();
    });
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(
            &mut *plugin,
            &input[first_at * CHANNELS..second_at * CHANNELS],
            &mut output[first_at * CHANNELS..second_at * CHANNELS],
            64,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(get_gain(&*plugin, 0), -6.0);
    assert!(as_dynamic(&mut *plugin).update_in_progress());
    // Second edit during the first blend, prepared from the fresh base.
    let base = as_dynamic(&mut *plugin).snapshot_for_update();
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
        as_dynamic(&mut *plugin)
            .submit_prepared_update(prepared)
            .unwrap();
        barrier.wait();
    });
    assert_eq!(as_dynamic(&mut *plugin).pending_len(), 1);
    // Next quanta refuse transiently (in-flight) while retaining the slot.
    let mut mid = vec![0.0; 64 * CHANNELS];
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(
            &mut *plugin,
            &input[second_at * CHANNELS..(second_at + 64) * CHANNELS],
            &mut mid,
            64,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(
        as_dynamic(&mut *plugin).last_refusal(),
        Some(CommitRefusal::UpdateInProgress)
    );
    assert_eq!(as_dynamic(&mut *plugin).pending_len(), 1);
    assert_eq!(get_gain(&*plugin, 0), -6.0);
    output[second_at * CHANNELS..(second_at + 64) * CHANNELS].copy_from_slice(&mid);
    // Run past the first blend, reclaim off audio, then the second commits.
    let rest_start = second_at + 64;
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(
            &mut *plugin,
            &input[rest_start * CHANNELS..],
            &mut output[rest_start * CHANNELS..],
            128,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    // Reclaim retired banks on a worker thread (barrier plus ack).
    let banks = as_dynamic(&mut *plugin).take_reclaimable_retired();
    assert!(!banks.is_empty(), "blend must have retired banks");
    let barrier = Barrier::new(2);
    let (ack_tx, ack_rx) = mpsc::channel();
    std::thread::scope(|s| {
        let barrier_ref = &barrier;
        s.spawn(move || {
            drop(banks);
            ack_tx.send(()).expect("reclaim ack");
            barrier_ref.wait();
        });
        ack_rx.recv().expect("reclaim ack");
        barrier.wait();
    });
    // Drive until the second blend completes; it commits after reclaim.
    let tail_in = pattern(1200);
    let mut tail_out = vec![0.0; tail_in.len()];
    // The second commit may need one more quantum after reclaim; run counted.
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(&mut *plugin, &tail_in, &mut tail_out, 128);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(get_gain(&*plugin, 0), -12.0);
    // Final audio matches a from-scratch final twin within 1e-5.
    let mut final_ref = create(&final_bands, 1.0);
    let mut final_out = vec![0.0; input.len()];
    process_into(&mut *final_ref, &input, &mut final_out, 64);
    // Compare only the post-second-blend region (last 500 frames).
    for frame in (total - 500)..total {
        for ch in 0..CHANNELS {
            let index = frame * CHANNELS + ch;
            assert!(
                (f64::from(output[index]) - f64::from(final_out[index])).abs() < 1e-5,
                "frame{frame} ch{ch}: {} vs {}",
                output[index],
                final_out[index]
            );
        }
    }
    assert!(rms(&tail_out) > 1e-4);
}

#[test]
fn reset_cancel_reprepare_stale_and_topology_retain_accepted() {
    let old_bands = vec![
        band("Peak", 1000.0, 1.0, 0.0),
        band("Peak", 3000.0, 1.0, 0.0),
    ];
    let mut plugin = create(&old_bands, 1.0);
    // Populated history. Post-commit span exceeds the 513-frame blend so the
    // first update completes before the stale probe runs.
    let input = pattern(2700);
    let mut output = vec![0.0; input.len()];
    process_into(&mut *plugin, &input[..2048 * CHANNELS], &mut output[..2048 * CHANNELS], 64);
    // Snapshot a base, then advance live with a first edit.
    let stale_base = as_dynamic(&mut *plugin).snapshot_for_update();
    as_dynamic(&mut *plugin)
        .request_band_update(0, band("Peak", 1000.0, 1.0, 6.0))
        .unwrap();
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(
            &mut *plugin,
            &input[2048 * CHANNELS..2100 * CHANNELS],
            &mut output[2048 * CHANNELS..2100 * CHANNELS],
            52,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(get_gain(&*plugin, 0), 6.0);
    // Prepare a second edit from the now-stale base on a worker.
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
            .expect("stale prepare still designs");
            tx.send(prepared).expect("ack");
            barrier_ref.wait();
        });
        let prepared = rx.recv().expect("worker ack");
        as_dynamic(&mut *plugin)
            .submit_prepared_update(prepared)
            .unwrap();
        barrier.wait();
    });
    // Finish the first blend, reclaim, then the stale commit refuses.
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(
            &mut *plugin,
            &input[2100 * CHANNELS..],
            &mut output[2100 * CHANNELS..],
            128,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    as_dynamic(&mut *plugin).reclaim_retired();
    let probe_in = pattern(64);
    let mut probe_out = vec![0.0; probe_in.len()];
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(&mut *plugin, &probe_in, &mut probe_out, 64);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(
        as_dynamic(&mut *plugin).last_refusal(),
        Some(CommitRefusal::StaleBase)
    );
    assert_eq!(as_dynamic(&mut *plugin).pending_len(), 1);
    assert_eq!(get_gain(&*plugin, 0), 6.0);
    assert_eq!(get_gain(&*plugin, 1), 0.0);
    // Cancel the stale slot off audio; live stays accepted.
    assert_eq!(as_dynamic(&mut *plugin).cancel_pending(), 1);
    assert_eq!(as_dynamic(&mut *plugin).pending_len(), 0);
    assert_eq!(get_gain(&*plugin, 1), 0.0);
    // Re-snapshot fresh and re-prepare the same intent on a worker.
    let base = as_dynamic(&mut *plugin).snapshot_for_update();
    let barrier = Barrier::new(2);
    let (tx, rx) = mpsc::channel();
    std::thread::scope(|s| {
        let barrier_ref = &barrier;
        s.spawn(move || {
            let prepared = LinearPhaseEqPlugin::prepare_band_update(
                &base,
                1,
                band("Peak", 3000.0, 1.0, -6.0),
            )
            .expect("fresh prepare");
            tx.send(prepared).expect("ack");
            barrier_ref.wait();
        });
        let prepared = rx.recv().expect("worker ack");
        as_dynamic(&mut *plugin)
            .submit_prepared_update(prepared)
            .unwrap();
        barrier.wait();
    });
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(&mut *plugin, &probe_in, &mut probe_out, 64);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert!(as_dynamic(&mut *plugin).last_refusal().is_none());
    assert_eq!(get_gain(&*plugin, 1), -6.0);
    // Topology refusal: placement changes stay structural at prepare time.
    let mut placed = band("Peak", 1000.0, 1.0, 6.0);
    placed.placement = Some(sotf_plugin_linear_phase_eq::LinearPhaseEqBandPlacement::Left);
    let err = as_dynamic(&mut *plugin)
        .request_band_update(0, placed)
        .expect_err("placement must stay structural");
    assert!(err.contains("structural") || err.contains("placement"), "{err}");
    assert_eq!(as_dynamic(&mut *plugin).pending_len(), 0);
    assert_eq!(get_gain(&*plugin, 0), 6.0);
    // Invalid band refusal at prepare time keeps live intact.
    let err = as_dynamic(&mut *plugin)
        .request_band_update(0, band("Peak", 1000.0, 1.0, 99.0))
        .expect_err("out-of-range gain must refuse");
    assert!(!err.is_empty());
    assert_eq!(get_gain(&*plugin, 0), 6.0);
    // Reset during the second blend completes instantly without dropping.
    assert!(as_dynamic(&mut *plugin).update_in_progress());
    let ((allocs, frees), ()) = count_allocs(|| {
        plugin.reset();
    });
    assert_eq!((allocs, frees), (0, 0));
    assert!(!as_dynamic(&mut *plugin).update_in_progress());
    assert_eq!(get_gain(&*plugin, 1), -6.0);
    assert!(as_dynamic(&mut *plugin).reclaim_retired());
    // Drained commit retains, then succeeds after reset without reprepare.
    let mut twin = create(&old_bands, 1.0);
    let mut twin_out = vec![0.0; 512 * CHANNELS];
    let twin_in = pattern(512);
    process_into(&mut *twin, &twin_in, &mut twin_out, 64);
    as_dynamic(&mut *twin)
        .request_band_update(0, band("Peak", 1000.0, 1.0, 3.0))
        .unwrap();
    let mut scratch = vec![0.0; 257 * CHANNELS];
    for _ in 0..20000 {
        let drained = twin
            .drain(&mut scratch, &ProcessContext::new(RATE, 0))
            .unwrap();
        if drained.complete {
            break;
        }
    }
    let mut after = vec![0.0; 64 * CHANNELS];
    let after_in = pattern(64);
    let err = twin
        .process(&after_in, &mut after, &ProcessContext::new(RATE, 64))
        .expect_err("process after drain must require reset");
    assert!(err.contains("reset"), "{err}");
    assert_eq!(
        as_dynamic(&mut *twin).last_refusal(),
        Some(CommitRefusal::Drained)
    );
    assert_eq!(as_dynamic(&mut *twin).pending_len(), 1);
    twin.reset();
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(&mut *twin, &after_in, &mut after, 64);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert!(as_dynamic(&mut *twin).last_refusal().is_none());
    assert_eq!(get_gain(&*twin, 0), 3.0);
}

#[test]
fn saturated_queue_and_off_thread_reclamation_tracking() {
    let old_bands = vec![
        band("Peak", 1000.0, 1.0, 0.0),
        band("Peak", 3000.0, 1.0, 0.0),
    ];
    let mut plugin = create(&old_bands, 1.0);
    // Post-commit span exceeds the 513-frame blend so the first update
    // completes before the stale probe runs.
    let input = pattern(2700);
    let mut output = vec![0.0; input.len()];
    process_into(&mut *plugin, &input[..2048 * CHANNELS], &mut output[..2048 * CHANNELS], 64);
    // Fill both slots from the same fresh base (same intent twice is fine for
    // saturation; the queue, not the DSP, enforces the bound).
    let base = as_dynamic(&mut *plugin).snapshot_for_update();
    let first = LinearPhaseEqPlugin::prepare_band_update(
        &base,
        0,
        band("Peak", 1000.0, 1.0, 4.0),
    )
    .unwrap();
    let second = LinearPhaseEqPlugin::prepare_band_update(
        &base,
        0,
        band("Peak", 1000.0, 1.0, 5.0),
    )
    .unwrap();
    as_dynamic(&mut *plugin)
        .submit_prepared_update(first)
        .unwrap();
    as_dynamic(&mut *plugin)
        .submit_prepared_update(second)
        .unwrap();
    assert_eq!(as_dynamic(&mut *plugin).pending_len(), 2);
    // Third submit fails full and drops its payload on control (off audio).
    let third = LinearPhaseEqPlugin::prepare_band_update(
        &base,
        0,
        band("Peak", 1000.0, 1.0, 6.0),
    )
    .unwrap();
    let err = as_dynamic(&mut *plugin)
        .submit_prepared_update(third)
        .expect_err("third concurrent update must report full");
    assert!(err.contains("full"), "{err}");
    assert_eq!(as_dynamic(&mut *plugin).pending_len(), 2);
    assert_eq!(get_gain(&*plugin, 0), 0.0);
    // Audio still commits once per quantum with strict (0, 0).
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(
            &mut *plugin,
            &input[2048 * CHANNELS..2112 * CHANNELS],
            &mut output[2048 * CHANNELS..2112 * CHANNELS],
            64,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(get_gain(&*plugin, 0), 4.0);
    // Run past the first blend; the second stays queued (stale base, since
    // both were prepared from the pre-first-commit snapshot).
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(
            &mut *plugin,
            &input[2112 * CHANNELS..],
            &mut output[2112 * CHANNELS..],
            128,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    as_dynamic(&mut *plugin).reclaim_retired();
    // The queued second edit now refuses stale (same base as the first).
    let probe_in = pattern(64);
    let mut probe_out = vec![0.0; probe_in.len()];
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(&mut *plugin, &probe_in, &mut probe_out, 64);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(
        as_dynamic(&mut *plugin).last_refusal(),
        Some(CommitRefusal::StaleBase)
    );
    assert_eq!(get_gain(&*plugin, 0), 4.0);
    // Off-thread reclamation of the first retired route plus the stale slot.
    let banks = as_dynamic(&mut *plugin).take_reclaimable_retired();
    let barrier = Barrier::new(2);
    let (ack_tx, ack_rx) = mpsc::channel();
    std::thread::scope(|s| {
        let barrier_ref = &barrier;
        s.spawn(move || {
            drop(banks);
            ack_tx.send(()).expect("reclaim ack");
            barrier_ref.wait();
        });
        ack_rx.recv().expect("reclaim ack");
        barrier.wait();
    });
    assert_eq!(as_dynamic(&mut *plugin).cancel_pending(), 1);
    assert_eq!(get_gain(&*plugin, 0), 4.0);
    // Dry alignment preserved: mix-0 output equals delayed input exactly.
    let mut dry = create(&old_bands, 0.0);
    let latency = dry.latency_samples();
    let total = 2000;
    let dry_in = pattern(total);
    let mut dry_out = vec![0.0; dry_in.len()];
    process_into(&mut *dry, &dry_in[..500 * CHANNELS], &mut dry_out[..500 * CHANNELS], 63);
    as_dynamic(&mut *dry)
        .request_band_update(0, band("Peak", 1000.0, 1.0, 12.0))
        .unwrap();
    let ((allocs, frees), ()) = count_allocs(|| {
        process_into(
            &mut *dry,
            &dry_in[500 * CHANNELS..],
            &mut dry_out[500 * CHANNELS..],
            1024,
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
