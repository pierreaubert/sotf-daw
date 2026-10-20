//! Complete default-host callback timing across FIR layouts and update phases.
//!
//! Run with --release. JSON lines retain every row even when deadlines miss;
//! a nonzero exit records missed physical/reported deadlines or invariants.
//! Local wall-clock observations are not a portable realtime certification.

use serde::Serialize;
use sotf_host::plugin::{Plugin, ProcessContext, TailLength};
use sotf_plugin_linear_phase_eq::{
    BandConfig, LinearPhaseEqBandPlacement as Placement, LinearPhaseEqDynamicPlugin,
    LinearPhaseEqPluginParams,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::{Duration, Instant};

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

// Cover a complete 8192/128-frame partition cycle on both steady legs.
const STEADY_CALLBACKS: usize = 64;
const TAPS: usize = 8192;

#[derive(Default, Serialize)]
struct Timings {
    #[serde(skip)]
    samples: Vec<Duration>,
    callbacks: usize,
    physical_misses: usize,
    reported_misses: usize,
    p50_us: f64,
    p95_us: f64,
    p99_us: f64,
    max_us: f64,
}

impl Timings {
    fn push(&mut self, elapsed: Duration, physical: Duration, reported: Duration) {
        self.samples.push(elapsed);
        self.physical_misses += usize::from(elapsed >= physical);
        self.reported_misses += usize::from(elapsed >= reported);
    }

    fn finish(&mut self) {
        self.samples.sort_unstable();
        self.callbacks = self.samples.len();
        if self.callbacks == 0 {
            return;
        }
        let rank =
            |p: usize| self.samples[(self.callbacks * p).div_ceil(100) - 1].as_secs_f64() * 1e6;
        self.p50_us = rank(50);
        self.p95_us = rank(95);
        self.p99_us = rank(99);
        self.max_us = self.samples[self.callbacks - 1].as_secs_f64() * 1e6;
    }
}

#[derive(Serialize)]
struct Row {
    rate_hz: u32,
    channels: usize,
    stages: usize,
    phase: &'static str,
    taps_per_stage: usize,
    callback_frames: usize,
    reported_quantum_frames: usize,
    physical_budget_us: f64,
    reported_budget_us: f64,
    steady_before: Timings,
    priming_or_acceptance: Timings,
    blend: Timings,
    steady_after: Timings,
    allocations: usize,
    frees: usize,
    finite_output: bool,
    replay_bound_ok: bool,
    accepted_generation: u64,
    settled: bool,
}

fn measure(
    plugin: &mut LinearPhaseEqDynamicPlugin,
    input: &[f32],
    output: &mut [f32],
    context: &ProcessContext,
) -> (Duration, usize, usize, bool) {
    let started = Instant::now();
    let ((allocations, frees), result) = count_allocs(|| plugin.process(input, output, context));
    let elapsed = started.elapsed();
    assert_eq!(
        result.expect("configured default-host callback must render"),
        context.num_frames
    );
    (
        elapsed,
        allocations,
        frees,
        output.iter().all(|x| x.is_finite()),
    )
}

fn scenario(rate: u32, channels: usize, stages: usize, phase: usize, frames: usize) -> Row {
    // Mostly Stereo stages exercise full-channel convolution load; one Mid
    // stage selects the ordered route while retaining an explicit pair matrix.
    let bands: Vec<_> = (0..stages)
        .map(|i| BandConfig {
            filter_type: "Peak".into(),
            frequency: 6000.0,
            q: 0.8,
            gain_db: if i % 2 == 0 { -3.0 } else { 3.0 },
            active: true,
            placement: Some(if i + 1 == stages {
                Placement::Mid
            } else {
                Placement::Stereo
            }),
        })
        .collect();
    let mut update = bands[stages - 1].clone();
    update.gain_db += 3.0;
    let params = LinearPhaseEqPluginParams {
        num_filters: stages,
        fir_length_index: 3,
        phase_mode_index: phase,
        auto_gain: false,
        mix: 1.0,
        filters: bands,
        stereo_pairs: Some(
            (0..channels / 2)
                .map(|pair| [2 * pair, 2 * pair + 1])
                .collect(),
        ),
    };
    let mut plugin = LinearPhaseEqDynamicPlugin::from_params(channels, rate, params).unwrap();
    let handle = plugin.control_handle();
    let support = match plugin.tail_length() {
        TailLength::Finite(x) => x as usize,
        _ => panic!("finite support"),
    };
    let input: Vec<f32> = (0..frames * channels)
        .map(|i| ((i * 7919 % 104729) as f32 / 104729.0 - 0.5) * 0.1)
        .collect();
    let mut output = vec![0.0; input.len()];
    let context = ProcessContext::new(rate, frames);
    let physical = Duration::from_secs_f64(frames as f64 / f64::from(rate));
    let quantum = plugin.realtime_quantum_frames();
    let reported = Duration::from_secs_f64(quantum.max(frames) as f64 / f64::from(rate));
    let mut row = Row {
        rate_hz: rate,
        channels,
        stages,
        phase: if phase == 0 { "linear" } else { "minimum" },
        taps_per_stage: TAPS,
        callback_frames: frames,
        reported_quantum_frames: quantum,
        physical_budget_us: physical.as_secs_f64() * 1e6,
        reported_budget_us: reported.as_secs_f64() * 1e6,
        steady_before: Timings::default(),
        priming_or_acceptance: Timings::default(),
        blend: Timings::default(),
        steady_after: Timings::default(),
        allocations: 0,
        frees: 0,
        finite_output: true,
        replay_bound_ok: true,
        accepted_generation: 0,
        settled: false,
    };
    // Populate beyond full cascade support before either steady or update leg.
    for _ in 0..(support + 37).div_ceil(frames) {
        let (_, a, d, finite) = measure(&mut plugin, &input, &mut output, &context);
        row.allocations += a;
        row.frees += d;
        row.finite_output &= finite;
    }
    for _ in 0..STEADY_CALLBACKS {
        let (elapsed, a, d, finite) = measure(&mut plugin, &input, &mut output, &context);
        row.allocations += a;
        row.frees += d;
        row.finite_output &= finite;
        row.steady_before.push(elapsed, physical, reported);
    }
    // This call performs design/preparation on control, outside the measured
    // and heap-guarded audio path. Real callbacks alone prime and accept it.
    plugin.request_band_update(stages - 1, update).unwrap();
    let max_callbacks =
        support.div_ceil(frames) + 513_usize.div_ceil(frames) + STEADY_CALLBACKS + 4;
    for _ in 0..max_callbacks {
        let before = handle.control_status();
        let (elapsed, a, d, finite) = measure(&mut plugin, &input, &mut output, &context);
        row.allocations += a;
        row.frees += d;
        row.finite_output &= finite;
        let after = handle.control_status();
        row.replay_bound_ok &= after.priming_frames_replayed <= 2 * frames;
        let timings = if before.accepted_generation == 0 {
            &mut row.priming_or_acceptance
        } else if before.blend_in_progress {
            &mut row.blend
        } else {
            &mut row.steady_after
        };
        timings.push(elapsed, physical, reported);
        if row.steady_after.samples.len() == STEADY_CALLBACKS {
            break;
        }
    }
    let status = handle.control_status();
    row.accepted_generation = status.accepted_generation;
    row.settled = status.accepted_generation == 1
        && !status.blend_in_progress
        && !status.priming_in_progress
        && row.steady_after.samples.len() == STEADY_CALLBACKS;
    row.steady_before.finish();
    row.priming_or_acceptance.finish();
    row.blend.finish();
    row.steady_after.finish();
    row
}

fn main() -> std::process::ExitCode {
    let mut failed_rows = 0;
    let mut total_rows = 0;
    let mut physical_misses = 0;
    let mut reported_misses = 0;
    for rate in [48_000, 96_000, 192_000] {
        for channels in [2, 8, 12] {
            for stages in [1, 2, 10] {
                for phase in [0, 1] {
                    for frames in [128, 512] {
                        let row = scenario(rate, channels, stages, phase, frames);
                        let timings = [
                            &row.steady_before,
                            &row.priming_or_acceptance,
                            &row.blend,
                            &row.steady_after,
                        ];
                        let physical: usize = timings.iter().map(|t| t.physical_misses).sum();
                        let reported: usize = timings.iter().map(|t| t.reported_misses).sum();
                        physical_misses += physical;
                        reported_misses += reported;
                        total_rows += 1;
                        failed_rows += usize::from(
                            physical > 0
                                || reported > 0
                                || row.allocations > 0
                                || row.frees > 0
                                || !row.finite_output
                                || !row.replay_bound_ok
                                || !row.settled,
                        );
                        println!("{}", serde_json::json!({"row":row}));
                    }
                }
            }
        }
    }
    println!(
        "{}",
        serde_json::json!({"summary":{"rows":total_rows,"failed_rows":failed_rows,
        "physical_misses":physical_misses,"reported_misses":reported_misses,
        "deadline_margin":1.0,"timing_scope":"complete default-host callbacks, local wall-clock observation"}})
    );
    if failed_rows == 0 {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
