// Facade and host integration for linear-phase EQ dynamic updates.
//
// Creates via actual `sotf_plugins::create_plugin` (facade, now dynamic) and
// renders through a real `DawHost` route. Running-engine legs live in the
// engine crate (`sotf-engine/tests/linear_phase_eq_engine_dynamic.rs`)
// because the facade must not take a normal dependency on the engine (that
// would cycle; the allowed dev-dependency exists only for tests). Control snapshots via
// `get_plugin` plus `as_any`, workers prepare off audio, the shared `Arc`
// handle from `get_plugin_data` delivers retained slots; retired banks
// reclaim off audio. Worker sync uses `Barrier` plus `mpsc` ack only. Audio
// sections assert strict `(0, 0)` alloc/free for f32 process plus drain. No
// test calls DSP commit APIs; commits run inside host renders.
//
// Native (`plugins-nih`) and FFI (`plugins-ffi`) ordinary setters stay
// structural until their own adoption. M1 accuracy stays open separately.

use sotf_plugins::plugin_linear_phase_eq::dynamic_host::{
    LinearPhaseEqControlHandle, LinearPhaseEqDynamicPlugin,
};
use sotf_plugins::plugin_linear_phase_eq::{BandConfig, LinearPhaseEqPlugin};
use sotf_plugins::plugin::TailLength;
use sotf_plugins::{DawHost, Host, ParameterId, ParameterValue};
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

fn host_with(bands: &[BandConfig], mix: f32) -> DawHost {
    let mut host = DawHost::new(CHANNELS, RATE);
    host.add_plugin(
        sotf_plugins::create_plugin("linear_phase_eq", &eq_parameters(bands, mix), CHANNELS, RATE)
            .expect("facade must create linear_phase_eq"),
    )
    .unwrap();
    host.build().unwrap();
    host
}

fn host_process_into(host: &mut DawHost, input: &[f32], output: &mut [f32], block: usize) {
    assert_eq!(input.len(), output.len());
    let frames = input.len() / CHANNELS;
    let mut position = 0;
    while position < frames {
        let count = block.min(frames - position);
        let range = position * CHANNELS..(position + count) * CHANNELS;
        let done = host
            .process(&input[range.clone()], &mut output[range])
            .expect("host render");
        assert_eq!(done, count);
        position += count;
    }
}

fn host_gain(host: &DawHost, band: usize) -> f32 {
    match host
        .get_plugin(0)
        .and_then(|p| p.get_parameter(&ParameterId::from(format!("band_{band}_gain").as_str())))
    {
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

#[test]
fn facade_factory_returns_dynamic_wrapper_with_static_parity() {
    let bands = vec![
        band("Peak", 1000.0, 1.0, 0.0),
        band("Peak", 3000.0, 1.0, 0.0),
    ];
    let mut plugin =
        sotf_plugins::create_plugin("linear_phase_eq", &eq_parameters(&bands, 1.0), CHANNELS, RATE)
            .expect("facade must create linear_phase_eq");
    assert!(plugin.as_any().is_some());
    let dynamic = plugin
        .as_any_mut()
        .and_then(|any| any.downcast_mut::<LinearPhaseEqDynamicPlugin>())
        .expect("facade must return the dynamic wrapper");
    assert_eq!(dynamic.pending_len(), 0);
    assert_eq!(plugin.latency_samples(), 512 + 32);
    assert_eq!(plugin.parameters().len(), 5 + bands.len() * 6);
    // Generic band setter stays structural.
    let err = plugin
        .set_parameter(
            ParameterId::from("band_0_gain"),
            ParameterValue::Float(6.0),
        )
        .expect_err("generic band setter must stay structural");
    assert!(err.contains("structural"), "{err}");
    // Host route renders static nonzero audio allocation-free.
    let mut host = host_with(&bands, 1.0);
    let input = pattern(2048);
    let mut output = vec![0.0; input.len()];
    let ((allocs, frees), ()) = count_allocs(|| {
        host_process_into(&mut host, &input, &mut output, 256);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert!(rms(&output) > 1e-4);
    assert_eq!(host_gain(&host, 0), 0.0);
}

fn run_host_automation_once(block: usize) -> Vec<f32> {
    let old_bands = vec![
        band("Peak", 1000.0, 1.0, 0.0),
        band("Peak", 3000.0, 1.0, 0.0),
    ];
    let total = 3000;
    let commit_at = 2048;
    let input = pattern(total);
    let mut host = host_with(&old_bands, 1.0);
    let mut output = vec![0.0; input.len()];
    host_process_into(
        &mut host,
        &input[..commit_at * CHANNELS],
        &mut output[..commit_at * CHANNELS],
        block,
    );
    assert!(rms(&output[..commit_at * CHANNELS]) > 1e-4);
    assert_eq!(host_gain(&host, 0), 0.0);
    // Worker prepares from a host snapshot; barrier plus ack sync.
    let base = host
        .get_plugin(0)
        .and_then(|p| p.as_any())
        .and_then(|any| any.downcast_ref::<LinearPhaseEqDynamicPlugin>())
        .expect("host must own the dynamic wrapper")
        .snapshot_for_update();
    let handle = Host::get_plugin_data(&host, 0)
        .and_then(|data| std::sync::Arc::downcast::<LinearPhaseEqControlHandle>(data).ok())
        .expect("host must forward the control handle");
    let barrier = Barrier::new(2);
    let (tx, rx) = mpsc::channel();
    std::thread::scope(|s| {
        let barrier_ref = &barrier;
        s.spawn(move || {
            let prepared = LinearPhaseEqPlugin::prepare_band_update(
                &base,
                0,
                band("Peak", 1000.0, 1.0, 9.0),
            )
            .expect("prepare");
            tx.send(prepared).expect("ack");
            barrier_ref.wait();
        });
        let prepared = rx.recv().expect("worker ack");
        handle.try_submit(prepared).expect("submit via handle");
        barrier.wait();
    });
    assert_eq!(host_gain(&host, 0), 0.0);
    let ((allocs, frees), ()) = count_allocs(|| {
        host_process_into(
            &mut host,
            &input[commit_at * CHANNELS..],
            &mut output[commit_at * CHANNELS..],
            block,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(host_gain(&host, 0), 9.0);
    assert!(rms(&output[commit_at * CHANNELS..]) > 1e-4);
    output
}

#[test]
fn host_dynamic_update_exact_zero_partitions_eof_and_final() {
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
    let output = run_host_automation_once(256);
    let mut old_host = host_with(&old_bands, 1.0);
    let mut new_host = host_with(&new_bands, 1.0);
    let mut old_out = vec![0.0; input.len()];
    let mut new_out = vec![0.0; input.len()];
    host_process_into(&mut old_host, &input, &mut old_out, 256);
    host_process_into(&mut new_host, &input, &mut new_out, 256);
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
            let expected =
                f64::from(old_out[index]) * (1.0 - w) + f64::from(new_out[index]) * w;
            assert!(
                (f64::from(output[index]) - expected).abs() < 1e-5,
                "frame{frame} ch{ch}"
            );
        }
    }
    let reference = run_host_automation_once(1);
    assert_eq!(output, reference);
    for block in [63, 512] {
        assert_eq!(run_host_automation_once(block), reference, "block {block}");
    }
    // Full dynamic EOF through the host drain (one-arg signature).
    let mut host = host_with(&old_bands, 1.0);
    let mut host_out = vec![0.0; input.len()];
    host_process_into(
        &mut host,
        &input[..commit_at * CHANNELS],
        &mut host_out[..commit_at * CHANNELS],
        256,
    );
    let base = host
        .get_plugin(0)
        .and_then(|p| p.as_any())
        .and_then(|any| any.downcast_ref::<LinearPhaseEqDynamicPlugin>())
        .expect("host must own the dynamic wrapper")
        .snapshot_for_update();
    let handle = Host::get_plugin_data(&host, 0)
        .and_then(|data| std::sync::Arc::downcast::<LinearPhaseEqControlHandle>(data).ok())
        .expect("host must forward the control handle");
    let barrier = Barrier::new(2);
    let (tx, rx) = mpsc::channel();
    std::thread::scope(|s| {
        let barrier_ref = &barrier;
        s.spawn(move || {
            let prepared = LinearPhaseEqPlugin::prepare_band_update(
                &base,
                0,
                band("Peak", 1000.0, 1.0, 9.0),
            )
            .expect("prepare");
            tx.send(prepared).expect("ack");
            barrier_ref.wait();
        });
        let prepared = rx.recv().expect("worker ack");
        handle.try_submit(prepared).expect("submit");
        barrier.wait();
    });
    let ((allocs, frees), ()) = count_allocs(|| {
        host_process_into(
            &mut host,
            &input[commit_at * CHANNELS..],
            &mut host_out[commit_at * CHANNELS..],
            256,
        );
    });
    assert_eq!((allocs, frees), (0, 0));
    let support = match host.get_plugin(0).expect("plugin").tail_length() {
        TailLength::Finite(frames) => frames as usize,
        _ => panic!("expected a finite tail"),
    };
    let mut tailed = Vec::with_capacity(host_out.len() + (support + 8) * CHANNELS);
    tailed.extend_from_slice(&host_out);
    let mut scratch = vec![0.0; 257 * CHANNELS];
    let ((allocs, frees), ()) = count_allocs(|| {
        for _ in 0..20000 {
            let drained = host.drain(&mut scratch).unwrap();
            tailed.extend_from_slice(&scratch[..drained.frames * CHANNELS]);
            if drained.complete {
                break;
            }
        }
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(tailed.len(), (total + support) * CHANNELS);
    for frame in (total - 200)..total {
        for ch in 0..CHANNELS {
            let index = frame * CHANNELS + ch;
            assert!(
                (f64::from(host_out[index]) - f64::from(new_out[index])).abs() < 1e-5,
                "frame{frame} ch{ch}"
            );
        }
    }
}
