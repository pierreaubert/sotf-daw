//! Release callback measurements for the performance assessment in audit-20261018.md.
//!
//! JSON Lines on stdout; setup, graph reference validation and contention start
//! are outside measured callbacks. No audio device or GPU workload is implied.
use clap::{Parser, ValueEnum};
use serde_json::json;
use sotf_plugins::{
    AnalogCompressorPlugin, DawHost, DenoiserPlugin, DynamicEqPlugin, GainPlugin, LimiterPlugin,
    LoudnessMonitorPlugin, ParameterValue, ParametricInPlacePluginAdapter, ParametricPluginAdapter,
    Plugin, UpmixerPlugin,
};
use std::hint::black_box;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

#[path = "audit_perf/allocator.rs"]
mod allocator;
#[global_allocator]
static ALLOCATOR: allocator::Meter = allocator::Meter;

// Host isolation may substitute passthrough after an individual plugin failure.
// Count diagnostics without formatting on callbacks so such a run cannot pass.
struct Diagnostics;
static DIAGNOSTICS: Diagnostics = Diagnostics;
static PROCESS_WARNINGS: AtomicUsize = AtomicUsize::new(0);
impl log::Log for Diagnostics {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Warn
    }
    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            PROCESS_WARNINGS.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn flush(&self) {}
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Chain {
    Dynamics,
    Analog,
    Spectral,
    Spatial,
}

#[derive(Parser, Debug)]
#[command(about = "Audit release callback timing and heap measurements (JSONL)")]
struct Args {
    #[arg(long, value_enum, default_value_t = Chain::Dynamics)]
    chain: Chain,
    #[arg(long, value_delimiter = ',', default_value = "2,8,32,64")]
    channels: Vec<usize>,
    #[arg(
        long,
        value_delimiter = ',',
        default_value = "44100,48000,96000,192000"
    )]
    rates: Vec<u32>,
    /// Zero selects the irregular 1/7/31/257/512 pattern.
    #[arg(long, value_delimiter = ',', default_value = "1,32,64,128,512,0")]
    blocks: Vec<usize>,
    /// At least 10000 callbacks are recommended for p99.9 observations.
    #[arg(long, default_value_t = 10000)]
    iterations: usize,
    /// Number of dynamics/analog processors or spectral pairs in the chain.
    #[arg(long, default_value_t = 1)]
    instances: usize,
    /// Keep historical 8192-frame host and Dynamic EQ preparation for comparison.
    #[arg(long)]
    legacy_capacity: bool,
    /// Actual competing CPU workers; zero measures isolated processing.
    #[arg(long, default_value_t = 0)]
    cpu_workers: usize,
    /// Label externally supplied contention (for example GPU workload and version).
    #[arg(long, default_value = "none")]
    external_load: String,
    /// Sleep between callbacks to approximate device cadence.
    #[arg(long)]
    paced: bool,
    /// Fail if a measured callback allocates or frees heap storage.
    #[arg(long)]
    require_zero_heap: bool,
    /// Fail when any measured callback exceeds this fraction of its deadline.
    #[arg(long)]
    max_budget_fraction: Option<f64>,
}

struct Load {
    stop: Arc<AtomicBool>,
    threads: Vec<std::thread::JoinHandle<()>>,
}
impl Load {
    fn start(workers: usize) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let ready = Arc::new(std::sync::Barrier::new(workers + 1));
        let threads = (0..workers)
            .map(|_| {
                let stop = Arc::clone(&stop);
                let ready = Arc::clone(&ready);
                std::thread::spawn(move || {
                    let mut state = vec![0.125_f64; 32768];
                    ready.wait();
                    while !stop.load(Ordering::Relaxed) {
                        for value in &mut state {
                            *value = black_box((*value * 1.000001 + 0.01).sin());
                        }
                        black_box(&state);
                    }
                })
            })
            .collect();
        ready.wait();
        Self { stop, threads }
    }
}
impl Drop for Load {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for thread in self.threads.drain(..) {
            thread.join().expect("CPU load worker panicked");
        }
    }
}

fn frames(block: usize, iteration: usize) -> usize {
    if block == 0 {
        [1, 7, 31, 257, 512][iteration % 5]
    } else {
        block
    }
}

fn host(
    chain: Chain,
    channels: usize,
    rate: u32,
    max_frames: usize,
    instances: usize,
    negotiated: bool,
    compiled: bool,
) -> Result<DawHost, String> {
    let mut host = DawHost::new(channels, rate);
    host.set_compiled_linear_enabled(compiled)?;
    if negotiated {
        host.set_max_block_frames(max_frames)?;
    }
    host.add_plugin(Box::new(ParametricPluginAdapter::new(GainPlugin::new(
        channels, -1.0,
    ))))?;
    let mut output_channels = channels;
    for _ in 0..instances {
        output_channels = match chain {
            Chain::Dynamics => {
                let eq = if negotiated {
                    DynamicEqPlugin::new_with_max_block_frames(channels, max_frames)?
                } else {
                    DynamicEqPlugin::new(channels)
                };
                host.add_plugin(Box::new(ParametricInPlacePluginAdapter::new(eq)))?;
                channels
            }
            Chain::Analog => {
                host.add_plugin(Box::new(ParametricInPlacePluginAdapter::new(
                    AnalogCompressorPlugin::new(channels),
                )))?;
                channels
            }
            Chain::Spectral => {
                for _ in 0..2 {
                    host.add_plugin(Box::new(ParametricInPlacePluginAdapter::new(
                        DenoiserPlugin::new(channels, false),
                    )))?;
                }
                channels
            }
            Chain::Spatial => {
                if channels != 2 {
                    return Err("spatial input requires two channels".into());
                }
                let upmixer = UpmixerPlugin::new(
                    4096, "9.1.6", 1.0, 0.5, 0.3, 80.0, 0.5, 250.0, 1.0, 1.0, false, 1.0,
                );
                let output_channels = upmixer.output_channels();
                host.add_plugin(Box::new(upmixer))?;
                output_channels
            }
        };
    }
    host.add_plugin(Box::new(ParametricInPlacePluginAdapter::new(
        LimiterPlugin::new(output_channels, -1.0, 50.0, 5.0, false),
    )))?;
    host.add_plugin(Box::new(LoudnessMonitorPlugin::new(output_channels)?))?;
    host.build()?;
    Ok(host)
}

fn signal(input: &mut [f32], channels: usize, rate: u32, position: usize, phase: &str) {
    for (index, sample) in input.iter_mut().enumerate() {
        *sample = match phase {
            "silence" => 0.0,
            "denormal" => 1.0e-39,
            _ => {
                let frame = position + index / channels;
                let angle = frame as f64 * std::f64::consts::TAU / f64::from(rate);
                (0.2 * (angle * 997.0).sin() + 0.1 * (angle * 313.0).sin()) as f32
            }
        };
    }
}

// Compare the optimized host path with graph execution before timing. Both get
// identical signal history, callback partitions, automation and resets.
fn validate(args: &Args, channels: usize, rate: u32, block: usize) -> Result<(), String> {
    let max_frames = frames(block, 4);
    let mut candidate = host(
        args.chain,
        channels,
        rate,
        max_frames,
        args.instances,
        !args.legacy_capacity,
        true,
    )?;
    let mut reference = host(
        args.chain,
        channels,
        rate,
        max_frames,
        args.instances,
        false,
        false,
    )?;
    let mut input = vec![0.0; max_frames * channels];
    let mut actual = vec![0.0; candidate.output_channels() * max_frames];
    let mut expected = actual.clone();
    let mut position = 0;
    let validation_blocks = 256.max(16384_usize.div_ceil(if block == 0 { 161 } else { block }));
    for iteration in 0..validation_blocks {
        let frames = frames(block, iteration);
        let input = &mut input[..frames * channels];
        signal(input, channels, rate, position, "tone");
        if iteration == 64 {
            for host in [&mut candidate, &mut reference] {
                host.set_plugin_parameter_at(
                    0,
                    "gain_db",
                    ParameterValue::Float(-3.0),
                    frames / 2,
                )?;
            }
        }
        if iteration == 128 {
            candidate.reset();
            reference.reset();
        }
        let output_samples = frames * candidate.output_channels();
        let actual = &mut actual[..output_samples];
        let expected = &mut expected[..output_samples];
        let produced = candidate.process(input, actual)?;
        let reference_produced = reference.process(input, expected)?;
        if produced != reference_produced {
            return Err("reference frame count differs".into());
        }
        for (a, b) in actual.iter().zip(expected.iter()) {
            if !a.is_finite() || !b.is_finite() || (a - b).abs() > 2e-5 * (1.0 + b.abs()) {
                return Err(format!(
                    "graph reference mismatch at block {iteration}: {a} vs {b}"
                ));
            }
        }
        position += frames;
    }
    actual.resize(
        candidate.drain_output_frames_max() * candidate.output_channels(),
        0.0,
    );
    expected.resize(
        reference.drain_output_frames_max() * reference.output_channels(),
        0.0,
    );
    for _ in 0..65536 {
        let a = candidate.drain(&mut actual)?;
        let b = reference.drain(&mut expected)?;
        if a.frames != b.frames || a.complete != b.complete {
            return Err("graph reference drain geometry differs".into());
        }
        for (a, b) in actual
            .iter()
            .zip(&expected)
            .take(a.frames * candidate.output_channels())
        {
            if !a.is_finite() || !b.is_finite() || (a - b).abs() > 2e-5 * (1.0 + b.abs()) {
                return Err("graph reference drain waveform differs".into());
            }
        }
        if a.complete {
            return Ok(());
        }
    }
    Err("graph reference drain exceeded bounded validation calls".into())
}

fn percentile(sorted: &[u64], permille: usize) -> u64 {
    sorted[((sorted.len() - 1) * permille).div_ceil(1000)]
}

fn run(args: &Args, channels: usize, rate: u32, block: usize) -> Result<bool, String> {
    let warnings = PROCESS_WARNINGS.load(Ordering::Relaxed);
    validate(args, channels, rate, block)?;
    if PROCESS_WARNINGS.load(Ordering::Relaxed) != warnings {
        return Err("reference validation emitted a host/plugin warning or error".into());
    }
    let baseline = allocator::LIVE.load(Ordering::Relaxed);
    allocator::PEAK.store(baseline, Ordering::Relaxed);
    let max_frames = frames(block, 4);
    let mut host = host(
        args.chain,
        channels,
        rate,
        max_frames,
        args.instances,
        !args.legacy_capacity,
        true,
    )?;
    let mut input = vec![0.0; max_frames * channels];
    let drain_frames = host.drain_output_frames_max().max(1);
    let mut output = vec![0.0; max_frames.max(drain_frames) * host.output_channels()];
    let mut times = Vec::with_capacity(args.iterations);
    let _load = Load::start(args.cpu_workers);
    let mut position = 0;
    let mut failed = false;
    let mut scenario_peak = allocator::PEAK.load(Ordering::Relaxed);
    for phase in [
        "cold",
        "steady",
        "automation",
        "silence",
        "denormal",
        "reset",
        "eof",
    ] {
        times.clear();
        allocator::PEAK.store(allocator::LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
        let iterations = if phase == "cold" { 1 } else { args.iterations };
        let mut allocs = 0;
        let mut frees = 0;
        let mut misses = 0;
        let mut worst_fraction = 0.0_f64;
        let mut queue_high_water = 0;
        let mut eof_complete = false;
        for iteration in 0..iterations {
            let frames = frames(block, iteration);
            signal(
                &mut input[..frames * channels],
                channels,
                rate,
                position,
                phase,
            );
            if phase == "automation" {
                host.set_plugin_parameter_at(
                    0,
                    "gain_db",
                    ParameterValue::Float(if iteration % 2 == 0 { -3.0 } else { -1.0 }),
                    frames / 2,
                )?;
            }
            // Start another finite stream after completion, so EOF timing never
            // degenerates into thousands of completed-drain no-ops.
            if phase == "eof" && eof_complete {
                host.reset();
                host.process(
                    &input[..frames * channels],
                    &mut output[..frames * host.output_channels()],
                )?;
            }
            queue_high_water = queue_high_water.max(host.pending_parameter_events());
            let start = Instant::now();
            allocator::begin();
            if phase == "reset" {
                host.reset();
            }
            let result = if phase == "eof" {
                host.drain(&mut output[..drain_frames * host.output_channels()])
                    .map(|result| {
                        eof_complete = result.complete;
                    })
            } else {
                host.process(
                    black_box(&input[..frames * channels]),
                    black_box(&mut output[..frames * host.output_channels()]),
                )
                .map(|_| ())
            };
            let (a, f) = allocator::end();
            let elapsed = start.elapsed();
            result?;
            if PROCESS_WARNINGS.load(Ordering::Relaxed) != warnings {
                return Err("measured callback emitted a host/plugin warning or error".into());
            }
            allocs += a;
            frees += f;
            if output.iter().any(|sample| !sample.is_finite()) {
                return Err("non-finite rendered output".into());
            }
            let budget_frames = if phase == "eof" { drain_frames } else { frames };
            let budget = Duration::from_secs_f64(budget_frames as f64 / f64::from(rate));
            let fraction = elapsed.as_secs_f64() / budget.as_secs_f64();
            worst_fraction = worst_fraction.max(fraction);
            misses += usize::from(elapsed > budget);
            times.push(elapsed.as_nanos().min(u128::from(u64::MAX)) as u64);
            position += frames;
            if args.paced {
                std::thread::sleep(budget.saturating_sub(start.elapsed()));
            }
        }
        times.sort_unstable();
        let peak = allocator::PEAK.load(Ordering::Relaxed);
        scenario_peak = scenario_peak.max(peak);
        let live = allocator::LIVE.load(Ordering::Relaxed);
        println!(
            "{}",
            json!({
                "chain": format!("{:?}", args.chain), "phase": phase,
                "negotiated_capacity": !args.legacy_capacity, "instances": args.instances, "channels_in": channels, "channels_out": host.output_channels(), "rate": rate,
                "block": if block == 0 { json!([1,7,31,257,512]) } else { json!(block) },
                "callbacks": iterations, "drain_capacity_frames": drain_frames, "p50_ns": percentile(&times,500), "p95_ns": percentile(&times,950),
                "p99_ns": percentile(&times,990), "p999_ns": percentile(&times,999), "max_ns": times.last(),
                "deadline_misses": misses, "max_budget_fraction": worst_fraction,
                "callback_allocations": allocs, "callback_frees": frees,
                "requested_heap_live_bytes": live, "requested_heap_peak_bytes": peak,
                "scenario_heap_growth_bytes": scenario_peak.saturating_sub(baseline),
                "dropped_parameter_events": host.dropped_parameter_events(),
                "parameter_queue_high_water_events": queue_high_water,
                "cpu_workers": args.cpu_workers, "external_load": args.external_load,
                "paced": args.paced, "release": !cfg!(debug_assertions), "reference": "graph-equivalent"
            })
        );
        failed |= args.require_zero_heap && (allocs != 0 || frees != 0);
        failed |= args
            .max_budget_fraction
            .is_some_and(|limit| worst_fraction > limit);
    }
    Ok(failed)
}

fn main() -> Result<(), String> {
    let args = Args::parse();
    log::set_logger(&DIAGNOSTICS).map_err(|error| error.to_string())?;
    log::set_max_level(log::LevelFilter::Warn);
    if args.instances == 0
        || args.instances > 128
        || (matches!(args.chain, Chain::Spatial) && args.instances != 1)
        || args.iterations == 0
        || args.channels.iter().any(|c| *c == 0 || *c > 64)
        || args
            .rates
            .iter()
            .any(|r| ![44100, 48000, 96000, 192000].contains(r))
        || args.blocks.iter().any(|b| *b > 8192)
        || args
            .max_budget_fraction
            .is_some_and(|f| !f.is_finite() || f <= 0.0)
    {
        return Err("invalid geometry, iteration count or budget fraction".into());
    }
    println!(
        "{}",
        json!({"kind": "metadata", "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH, "logical_cpus": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
        "package_version": env!("CARGO_PKG_VERSION"), "release": !cfg!(debug_assertions)})
    );
    let mut failed = false;
    for &channels in &args.channels {
        if matches!(args.chain, Chain::Spatial) && channels != 2 {
            continue;
        }
        for &rate in &args.rates {
            for &block in &args.blocks {
                failed |= run(&args, channels, rate, block)?;
            }
        }
    }
    if failed {
        Err("performance gate failed (see JSONL measurements)".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tail_percentiles_and_irregular_geometry() {
        let times: Vec<u64> = (1..=10000).collect();
        assert_eq!(percentile(&times, 999), 9991);
        assert_eq!(percentile(&[42], 999), 42);
        assert_eq!((0..5).map(|i| frames(0, i)).sum::<usize>(), 808);
    }
}
