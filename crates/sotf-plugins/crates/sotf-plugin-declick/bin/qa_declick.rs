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
