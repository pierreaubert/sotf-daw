use sotf_host::{
    CountingAlloc, ParametricInPlacePlugin, ParametricInPlacePluginAdapter, ProcessContext,
    assert_no_allocs, run_standard_tests,
};
use sotf_plugin_declick::{DeclickPlugin, DeclickPluginParams};
use std::time::Instant;

#[global_allocator]
static A: CountingAlloc = CountingAlloc;

fn main() {
    let plugin = DeclickPlugin::new(2, 48_000).expect("valid QA configuration");
    let mut plugin = ParametricInPlacePluginAdapter::new(plugin);
    run_standard_tests(&mut plugin, "DeclickPlugin");

    println!("\n[Declick active callback matrix]");
    for channels in [1, 2, 8, 40] {
        for block_size in [16, 257, 1024] {
            run_active_case(
                channels,
                block_size,
                DeclickPluginParams::default(),
                "legacy",
            );
        }
    }

    println!("\n[Declick mode matrix]");
    let modes: Vec<(&str, DeclickPluginParams)> = vec![
        (
            "periodic",
            DeclickPluginParams {
                mode: 1,
                sensitivity: 5.0,
                ..Default::default()
            },
        ),
        (
            "multiband",
            DeclickPluginParams {
                bands: 2,
                sensitivity: 5.0,
                ..Default::default()
            },
        ),
        (
            "widened-residual",
            DeclickPluginParams {
                mode: 1,
                bands: 1,
                repair_width: 4,
                audition_residual: true,
                sensitivity: 5.0,
                ..Default::default()
            },
        ),
    ];
    for channels in [1, 2, 8] {
        for (name, params) in &modes {
            run_active_case(channels, 257, params.clone(), name);
        }
    }

    println!("\n[Declick quality matrix]");
    // R44 errata: `bands` is a BANDS_OPTIONS choice index (0/1/2 =
    // full/2/3-band), not a band count — the R40 "2band" legs were
    // actually 3-band. Both topologies are now covered explicitly.
    let owned = |bands: usize| DeclickPluginParams {
        mode: 1,
        bands,
        crossover_hz: 4000.0,
        sensitivity: 5.0,
        ..Default::default()
    };
    let legs: Vec<(&str, DeclickPluginParams, &str)> = vec![
        ("legacy-default", DeclickPluginParams::default(), "tone"),
        ("owned-periodic-2band", owned(1), "tone"),
        ("owned-periodic-2band", owned(1), "noise"),
        ("owned-periodic-3band", owned(2), "tone"),
        ("owned-periodic-3band", owned(2), "noise"),
    ];
    for (name, params, fixture) in &legs {
        run_quality_case(2, 256, params.clone(), fixture, name);
    }
}

fn run_active_case(channels: usize, block_size: usize, params: DeclickPluginParams, name: &str) {
    let mut plugin = DeclickPlugin::from_params(channels, 48_000, params).unwrap();
    let mut buffer = vec![0.0_f32; channels * block_size];
    for frame in 0..block_size {
        let clean = (frame as f32 * 0.07).sin() * 0.2;
        for ch in 0..channels {
            buffer[frame * channels + ch] = clean * (1.0 - ch as f32 * 0.005);
        }
    }
    if block_size > 8 {
        let click_frame = block_size / 2;
        for ch in 0..channels {
            buffer[click_frame * channels + ch] += 2.0;
        }
    }
    let context = ProcessContext::new(48_000, block_size);
    for _ in 0..4 {
        plugin.process_in_place(&mut buffer, &context).unwrap();
    }
    assert_no_allocs("Declick active matrix", || {
        plugin.process_in_place(&mut buffer, &context).unwrap();
    });

    let mut timings = Vec::with_capacity(64);
    for _ in 0..64 {
        let start = Instant::now();
        plugin.process_in_place(&mut buffer, &context).unwrap();
        timings.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    timings.sort_by(f64::total_cmp);
    let p50 = timings[timings.len() / 2];
    let p99 = timings[timings.len() - 1];
    let max = timings.iter().copied().fold(0.0, f64::max);
    let deadline = block_size as f64 / 48_000.0 * 1000.0;
    println!(
        "  {name} {channels}ch block={block_size}: p50/p99/max {p50:.3}/{p99:.3}/{max:.3} ms (deadline {deadline:.3} ms)"
    );
    assert!(
        max < deadline,
        "{name} {channels}ch block={block_size}: callback max {max:.3} ms exceeded audio deadline {deadline:.3} ms (p50 {p50:.3} ms)"
    );
}

/// Deterministic xorshift64* step (qa fixtures must not need `rand`).
fn xorshift64star(state: &mut u64) -> f32 {
    let mut x = *state;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    *state = x;
    ((x.wrapping_mul(0x2545F4914F6CDD1D) >> 33) as f32) / (u32::MAX as f32) * 2.0 - 1.0
}

/// Plugin-level repair/damage quality through blocks plus declared drain.
///
/// Synthetic-contract fixtures (tone/noise plus interior 3.0 clicks) at
/// the frozen consumer bounds: 5%-of-amplitude repair, 0.05 damage
/// outside a ±8 footprint guard past 64 settled frames, finite output,
/// exact input-plus-latency length. Asserts (qa convention: panic = red).
fn run_quality_case(
    channels: usize,
    block_size: usize,
    params: DeclickPluginParams,
    fixture: &str,
    name: &str,
) {
    let frames = 2048_usize;
    let mut clean = vec![0.0_f32; frames * channels];
    let mut rng = 0x1234_5678_9ABC_DEF0_u64;
    for (frame, slot) in clean.chunks_mut(channels).enumerate() {
        for (ch, sample) in slot.iter_mut().enumerate() {
            let freq = if ch == 0 { 440.0 } else { 660.0 };
            *sample = match fixture {
                "noise" => xorshift64star(&mut rng) * 0.1,
                _ => (frame as f32 * freq / 48_000.0 * std::f32::consts::TAU).sin() * 0.25,
            };
        }
    }
    let clicks = [512_usize, 1024, 1536];
    let mut input = clean.clone();
    for click in &clicks {
        for sample in &mut input[click * channels..(click + 1) * channels] {
            *sample += 3.0;
        }
    }
    let mut plugin = DeclickPlugin::from_params(channels, 48_000, params).unwrap();
    let latency = plugin.latency_samples();
    let mut output = Vec::with_capacity((frames + latency) * channels);
    for chunk in input.chunks(block_size * channels) {
        let mut block = chunk.to_vec();
        let context = ProcessContext::new(48_000, chunk.len() / channels);
        plugin.process_in_place(&mut block, &context).unwrap();
        output.extend_from_slice(&block);
    }
    drain_to_vec(&mut plugin, channels, &mut output);
    assert_eq!(
        output.len(),
        (frames + latency) * channels,
        "{name}/{fixture}: render must be input plus latency exactly"
    );
    assert!(
        output.iter().all(|sample| sample.is_finite()),
        "{name}/{fixture}: render must stay finite"
    );
    let mut worst_repair = 0.0_f32;
    let mut worst_damage = 0.0_f32;
    let aligned = output
        .chunks_exact(channels)
        .skip(latency)
        .zip(clean.chunks_exact(channels))
        .enumerate();
    for (frame, (actual_frame, expected_frame)) in aligned {
        if frame < 64 {
            continue;
        }
        let near = clicks.iter().any(|click| frame.abs_diff(*click) <= 8);
        for (actual, expected) in actual_frame.iter().zip(expected_frame.iter()) {
            let deviation = (actual - expected).abs();
            if clicks.contains(&frame) {
                worst_repair = worst_repair.max(deviation);
                assert!(
                    deviation < 0.15,
                    "{name}/{fixture}: repair frame={frame} deviation={deviation}"
                );
            } else if !near {
                worst_damage = worst_damage.max(deviation);
                assert!(
                    deviation < 0.05,
                    "{name}/{fixture}: damage frame={frame} deviation={deviation}"
                );
            }
        }
    }
    println!("  {name}/{fixture}: worst repair {worst_repair:.6}, worst damage {worst_damage:.6}");
}

/// Declared drain appended to `output` (iteration-bounded; panics loudly
/// instead of hanging when `complete` never arrives).
fn drain_to_vec(plugin: &mut DeclickPlugin, channels: usize, output: &mut Vec<f32>) {
    use sotf_host::plugin::PluginDrainResult;
    let capacity = plugin.drain_output_frames_max();
    assert!(capacity > 0, "drain capacity must be positive");
    let context = ProcessContext::new(48_000, capacity);
    let mut drained = 0_usize;
    for _ in 0..4 {
        let mut tail = vec![0.0_f32; capacity * channels];
        let result: PluginDrainResult = plugin.drain(&mut tail, &context).unwrap();
        output.extend_from_slice(&tail[..result.frames * channels]);
        drained += result.frames;
        if result.complete {
            break;
        }
    }
    assert_eq!(
        drained,
        plugin.latency_samples(),
        "drained frames must equal latency exactly"
    );
}
