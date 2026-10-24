//! Explicit admission-boundary callback and requested-memory diagnostics.

// Rust guideline compliant 2026-02-21
use serde_json::{Value, json};
use sotf_host::{ParametricInPlacePluginAdapter, Plugin, ProcessContext};
use sotf_plugin_convolution::{ConvolutionPlugin, ConvolutionPluginParams};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

// Match the persistent backend and owned decoded-sample admission limits.
// File preflight below independently confirms each computed boundary.
const BUDGET: usize = 512 * 1024 * 1024;
const HEAD: usize = 257;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
thread_local! {
    static GUARDED: Cell<bool> = const { Cell::new(false) };
    static COUNTS: Cell<(usize,usize)> = const { Cell::new((0,0)) };
}
struct Allocator;
// SAFETY: Forward all original pointers/layouts unchanged to System. Counters
// record requested allocation bytes, not allocator usable size or process RSS.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: Forward the valid allocation request unchanged.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        let _ = GUARDED.try_with(|active| {
            if active.get() {
                let _ = COUNTS.try_with(|counts| {
                    let (a, d) = counts.get();
                    counts.set((a + 1, d));
                });
            }
        });
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        let _ = GUARDED.try_with(|active| {
            if active.get() {
                let _ = COUNTS.try_with(|counts| {
                    let (a, d) = counts.get();
                    counts.set((a, d + 1));
                });
            }
        });
        // SAFETY: Forward the original pointer and its valid allocation layout.
        unsafe { System.dealloc(ptr, layout) };
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

#[derive(Clone, Copy)]
struct Case {
    rate: u32,
    outputs: usize,
    ir_channels: usize,
    nupc: bool,
    head: bool,
    matrix: bool,
}
impl Case {
    fn geometry(self) -> Value {
        json!({"rate_hz":self.rate,"output_channels":self.outputs,
        "ir_channels":self.ir_channels,"nupc":self.nupc,"head":self.head,
        "head_taps":HEAD,"true_stereo":self.matrix})
    }
    fn estimate(self, frames: usize) -> usize {
        ConvolutionPlugin::qa_backend_estimate(
            &vec![frames; self.ir_channels],
            self.outputs,
            self.nupc,
            self.matrix,
            self.head,
            HEAD,
        )
    }
    fn boundary(self) -> usize {
        let mut low = 1_usize;
        let mut high = (self.rate as usize * 30).min(BUDGET / (self.ir_channels * 4));
        while low < high {
            let middle = low + (high - low).div_ceil(2);
            if self.estimate(middle) <= BUDGET {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        low
    }
}
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn fixture(case: Case, frames: usize, id: usize) -> Result<Fixture, String> {
    let is_aiff = case.ir_channels == 32;
    let path = std::env::temp_dir().join(format!(
        "sotf-maxload-{}-{id}.{}",
        std::process::id(),
        if is_aiff { "aiff" } else { "wav" }
    ));
    let file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    let fixture = Fixture(path);
    let mut file = BufWriter::new(file);
    let size = (frames * case.ir_channels * 2) as u32;
    if is_aiff {
        file.write_all(b"FORM").map_err(|e| e.to_string())?;
        file.write_all(&(46 + size).to_be_bytes())
            .map_err(|e| e.to_string())?;
        file.write_all(b"AIFFCOMM").map_err(|e| e.to_string())?;
        file.write_all(&18_u32.to_be_bytes())
            .map_err(|e| e.to_string())?;
        file.write_all(&(case.ir_channels as u16).to_be_bytes())
            .map_err(|e| e.to_string())?;
        file.write_all(&(frames as u32).to_be_bytes())
            .map_err(|e| e.to_string())?;
        file.write_all(&16_u16.to_be_bytes())
            .map_err(|e| e.to_string())?;
        let exponent = 0x400e_u16 + (case.rate / 48_000).ilog2() as u16;
        file.write_all(&exponent.to_be_bytes())
            .map_err(|e| e.to_string())?;
        file.write_all(&[0xbb, 0x80, 0, 0, 0, 0, 0, 0])
            .map_err(|e| e.to_string())?;
        file.write_all(b"SSND").map_err(|e| e.to_string())?;
        file.write_all(&(8 + size).to_be_bytes())
            .map_err(|e| e.to_string())?;
        file.write_all(&[0; 8]).map_err(|e| e.to_string())?;
    } else {
        file.write_all(b"RIFF").map_err(|e| e.to_string())?;
        file.write_all(&(36 + size).to_le_bytes())
            .map_err(|e| e.to_string())?;
        file.write_all(b"WAVEfmt ").map_err(|e| e.to_string())?;
        file.write_all(&16_u32.to_le_bytes())
            .map_err(|e| e.to_string())?;
        file.write_all(&1_u16.to_le_bytes())
            .map_err(|e| e.to_string())?;
        file.write_all(&(case.ir_channels as u16).to_le_bytes())
            .map_err(|e| e.to_string())?;
        file.write_all(&case.rate.to_le_bytes())
            .map_err(|e| e.to_string())?;
        file.write_all(&(case.rate * case.ir_channels as u32 * 2).to_le_bytes())
            .map_err(|e| e.to_string())?;
        file.write_all(&(case.ir_channels as u16 * 2).to_le_bytes())
            .map_err(|e| e.to_string())?;
        file.write_all(&16_u16.to_le_bytes())
            .map_err(|e| e.to_string())?;
        file.write_all(b"data").map_err(|e| e.to_string())?;
        file.write_all(&size.to_le_bytes())
            .map_err(|e| e.to_string())?;
    }
    for frame in 0..frames {
        for ch in 0..case.ir_channels {
            let sample = (((frame * 17 + ch * 13) % 61) as i16 - 30) * 16;
            file.write_all(&if is_aiff {
                sample.to_be_bytes()
            } else {
                sample.to_le_bytes()
            })
            .map_err(|e| e.to_string())?;
        }
    }
    file.flush().map_err(|e| e.to_string())?;
    Ok(fixture)
}
fn preflight(case: Case, fixture: &Fixture) -> Result<(), String> {
    ConvolutionPlugin::validate_ir_resource_for_routing(
        fixture.0.to_str().ok_or("fixture path is not UTF-8")?,
        Some(f64::from(case.rate)),
        case.outputs,
        case.nupc,
        case.matrix,
        case.head,
        HEAD,
    )
}

fn measure(
    plugin: &mut dyn Plugin,
    case: Case,
    frames: usize,
    calls: usize,
) -> Result<Value, String> {
    let input: Vec<_> = (0..frames * case.outputs)
        .map(|i| ((i * 7919 % 104729) as f32 / 104729.0 - 0.5) * 0.1)
        .collect();
    let mut output = vec![0.0; input.len()];
    let context = ProcessContext::new(case.rate, frames);
    let quantum = plugin.realtime_quantum_frames();
    let physical = frames as f64 / f64::from(case.rate);
    let reported = frames.max(quantum) as f64 / f64::from(case.rate);
    let mut times = Vec::with_capacity(calls);
    let (mut allocations, mut frees, mut misses, mut reported_misses) = (0, 0, 0, 0);
    let mut finite = true;
    for _ in 0..calls {
        COUNTS.with(|c| c.set((0, 0)));
        let start = Instant::now();
        GUARDED.with(|c| c.set(true));
        let result = plugin.process(&input, &mut output, &context);
        GUARDED.with(|c| c.set(false));
        let elapsed = start.elapsed();
        let (a, d) = COUNTS.with(Cell::get);
        allocations += a;
        frees += d;
        if result? != frames {
            return Err("callback returned wrong frame count".into());
        }
        finite &= output.iter().all(|sample| sample.is_finite());
        misses += usize::from(elapsed.as_secs_f64() >= physical);
        reported_misses += usize::from(elapsed.as_secs_f64() >= reported);
        times.push(elapsed.as_secs_f64() * 1e6);
    }
    times.sort_by(f64::total_cmp);
    let percentile = |rank: usize| times[(calls * rank).div_ceil(100).saturating_sub(1)];
    Ok(
        json!({"callback_frames":frames,"callbacks":calls,"physical_budget_us":physical*1e6,
        "reported_quantum_frames":quantum,"reported_budget_us":reported*1e6,"physical_misses":misses,
        "reported_misses":reported_misses,"p50_us":percentile(50),"p95_us":percentile(95),
        "p99_us":percentile(99),"max_us":times.last(),"allocations":allocations,"frees":frees,"finite":finite}),
    )
}
fn run(case: Case, id: usize) -> Result<Value, String> {
    let length = case.boundary();
    let over = fixture(case, length + 1, id * 2)?;
    let next_error = preflight(case, &over)
        .err()
        .ok_or("next frame above boundary was admitted")?;
    drop(over);
    let file = fixture(case, length, id * 2 + 1)?;
    preflight(case, &file)?;
    let before = LIVE.load(Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);
    let start = Instant::now();
    let plugin = ConvolutionPlugin::from_params_with_routing(
        case.outputs,
        case.rate,
        ConvolutionPluginParams {
            ir_file: file.0.to_string_lossy().into_owned(),
            use_nupc: case.nupc,
            zero_latency_head: case.head,
            head_taps: HEAD,
            ..Default::default()
        },
        case.matrix,
    )?;
    let construction_ms = start.elapsed().as_secs_f64() * 1e3;
    let retained = LIVE.load(Ordering::Relaxed).saturating_sub(before);
    let peak = PEAK.load(Ordering::Relaxed).saturating_sub(before);
    let actual_largest_period = plugin.qa_largest_partition_frames();
    let mut plugin = ParametricInPlacePluginAdapter::new(plugin);
    Plugin::initialize(&mut plugin, f64::from(case.rate))?;
    let latency = plugin.latency_samples();
    let warm_calls = (length + latency + 37).div_ceil(128);
    let warm = measure(&mut plugin, case, 128, warm_calls)?;
    let legacy_largest_period = if case.nupc {
        let head = if case.head { HEAD.min(length) } else { 0 };
        let min_block = if head == 0 { 1024 } else { 1024.min(head) };
        plugins_spatial::nupc::plan_partitions(length - head, min_block)
            .iter()
            .map(|spec| spec.block_size)
            .max()
            .unwrap_or(1)
    } else {
        1024
    };
    // Retain the preceding untrimmed power-of-two head grid as another floor.
    // Smaller prepared final levels must not shorten this established workload.
    let baseline_largest_period = if case.nupc && case.head {
        let head = HEAD.min(length);
        let base = 1_usize << 1024.min(head).ilog2();
        plugins_spatial::nupc::plan_partitions(length - head, base)
            .iter()
            .map(|spec| spec.block_size)
            .max()
            .unwrap_or(1)
    } else {
        legacy_largest_period
    };
    let largest_period = legacy_largest_period
        .max(baseline_largest_period)
        .max(actual_largest_period);
    let mut steady = Vec::new();
    for frames in [128, 512] {
        let calls = 256.max((largest_period * 4).div_ceil(frames));
        steady.push(measure(&mut plugin, case, frames, calls)?);
    }
    Ok(
        json!({"event":"case_result","geometry":case.geometry(),"ir_frames":length,
        "ir_seconds":length as f64/f64::from(case.rate),"estimate_bytes":case.estimate(length),
        "next_frame_rejection":next_error,"construction_ms":construction_ms,
        "construction_retained_requested_delta_bytes":retained,"construction_peak_requested_delta_bytes":peak,
        "retained_delta_within_estimate":retained<=case.estimate(length),"latency_frames":latency,
        "legacy_largest_period_frames":legacy_largest_period,
        "baseline_largest_period_frames":baseline_largest_period,
        "actual_largest_period_frames":actual_largest_period,
        "measured_largest_period_frames":largest_period,
        "warm":warm,"steady":steady}),
    )
}
fn emit(value: Value) {
    println!("{value}");
    std::io::stdout().flush().unwrap();
}
fn main() {
    drop(ConvolutionPlugin::new(2, 48_000));
    let (mut cases, mut failed_cases, mut misses, mut errors) = (0, 0, 0, 0);
    for rate in [48_000, 96_000, 192_000] {
        for (outputs, ir_channels, matrix) in [
            (2, 1, false),
            (2, 2, false),
            (12, 1, false),
            (12, 12, false),
            (32, 1, false),
            (32, 32, false),
            (2, 4, true),
        ] {
            for (nupc, head) in [(false, false), (true, false), (true, true)] {
                cases += 1;
                let case = Case {
                    rate,
                    outputs,
                    ir_channels,
                    matrix,
                    nupc,
                    head,
                };
                emit(json!({"event":"case_start","id":cases,"geometry":case.geometry()}));
                match run(case, cases) {
                    Ok(row) => {
                        let phases =
                            std::iter::once(&row["warm"]).chain(row["steady"].as_array().unwrap());
                        let mut failed = !row["retained_delta_within_estimate"].as_bool().unwrap();
                        for phase in phases {
                            let count = phase["physical_misses"].as_u64().unwrap();
                            misses += count;
                            failed |= count != 0
                                || phase["reported_misses"].as_u64().unwrap() != 0
                                || phase["allocations"].as_u64().unwrap() != 0
                                || phase["frees"].as_u64().unwrap() != 0
                                || !phase["finite"].as_bool().unwrap();
                        }
                        failed_cases += usize::from(failed);
                        emit(row);
                    }
                    Err(error) => {
                        errors += 1;
                        failed_cases += 1;
                        emit(
                            json!({"event":"case_error","id":cases,"geometry":case.geometry(),"error":error}),
                        );
                    }
                }
            }
        }
    }
    emit(
        json!({"event":"summary","cases":cases,"failed_cases":failed_cases,"errors":errors,
        "physical_misses":misses,"timing_margin":1.0,"pass":failed_cases==0,
        "scope":"local complete Plugin::process; full-history warm and steady; requested allocation deltas are not RSS or native-device compliance"}),
    );
    if failed_cases != 0 {
        std::process::exit(1);
    }
}
