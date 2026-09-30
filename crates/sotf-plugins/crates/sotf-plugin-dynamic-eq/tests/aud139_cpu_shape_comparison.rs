//! Manual matched Peak-versus-shelf callback-cost comparison for AUD139.
//!
//! This benchmark reports raw trial durations and does not claim a worst-case
//! execution time. It reuses the pre-edit Peak harness's callback sizes,
//! channel counts, band counts, signal, warmups, and measured-trial count.
//! Run this ignored test with `cargo test --release --test
//! aud139_cpu_shape_comparison -- --ignored --nocapture`.

// Rust guideline compliant 2026-02-21

use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_plugin_dynamic_eq::{DynEqBandParams, DynEqShape, DynamicEqPlugin, DynamicEqPluginParams};
use std::hint::black_box;
use std::time::{Duration, Instant};

const SAMPLE_RATE: u32 = 48_000;
const TRIAL_FRAMES: usize = 65_536;
const WARMUP_TRIALS: usize = 2;
const MEASURED_TRIALS: usize = 7;
const SHELF_SLOPE: f32 = 0.75;
const BUILD_PROFILE: &str = if cfg!(debug_assertions) {
    "debug_assertions_enabled"
} else {
    "release"
};

#[derive(Clone, Copy, Debug)]
enum ShapeCase {
    Peak,
    LowShelf,
    HighShelf,
}

impl ShapeCase {
    const ALL: [Self; 3] = [Self::Peak, Self::LowShelf, Self::HighShelf];

    const fn name(self) -> &'static str {
        match self {
            Self::Peak => "peak",
            Self::LowShelf => "low_shelf",
            Self::HighShelf => "high_shelf",
        }
    }

    const fn shape(self) -> DynEqShape {
        match self {
            Self::Peak => DynEqShape::Peak,
            Self::LowShelf => DynEqShape::LowShelf,
            Self::HighShelf => DynEqShape::HighShelf,
        }
    }
}

fn params_for(channels: usize, num_bands: usize, shape: ShapeCase) -> DynamicEqPluginParams {
    let mut params = DynamicEqPluginParams {
        num_bands,
        threshold: -28.0,
        ratio: 3.0,
        attack_ms: 3.0,
        release_ms: 80.0,
        knee: 4.0,
        link_channels: channels > 1,
        mix: 1.0,
        bands: vec![DynEqBandParams::default(); num_bands],
    };

    for (index, band) in params.bands.iter_mut().enumerate() {
        let fraction = if num_bands == 1 {
            0.0
        } else {
            index as f32 / (num_bands - 1) as f32
        };
        band.frequency = 70.0 * (6_000.0_f32 / 70.0).powf(fraction);
        band.q = 0.85;
        band.gain = if index % 2 == 0 { 8.0 } else { -6.0 };
        band.band_threshold = -28.0;
        band.band_ratio = 3.0;
        band.active = true;
        band.solo = false;
        band.shape = shape.shape();
        band.shelf_slope = SHELF_SLOPE;
    }
    params
}

fn prepared_blocks(channels: usize, callback_frames: usize) -> Vec<Vec<f32>> {
    let block_count = TRIAL_FRAMES / callback_frames;
    let mut blocks = Vec::with_capacity(block_count);
    let mut first_frame = 0usize;

    for _ in 0..block_count {
        let mut block = Vec::with_capacity(callback_frames * channels);
        for frame in first_frame..first_frame + callback_frames {
            let time = frame as f32 / SAMPLE_RATE as f32;
            for channel in 0..channels {
                let phase = channel as f32 * 0.173;
                let sample = 0.19 * (std::f32::consts::TAU * 73.0 * time + phase).sin()
                    + 0.13 * (std::f32::consts::TAU * 1_307.0 * time + phase * 0.7).sin()
                    + 0.08 * (std::f32::consts::TAU * 6_103.0 * time - phase * 0.4).sin();
                block.push(sample);
            }
        }
        blocks.push(block);
        first_frame += callback_frames;
    }
    blocks
}

fn run_trial(
    plugin: &mut DynamicEqPlugin,
    channels: usize,
    callback_frames: usize,
    measure: bool,
) -> Duration {
    let mut blocks = prepared_blocks(channels, callback_frames);
    let context = ProcessContext::new(SAMPLE_RATE, callback_frames);
    let started = measure.then(Instant::now);

    for block in &mut blocks {
        let processed = plugin
            .process_in_place(block, &context)
            .expect("matched CPU callback settings remain valid");
        assert_eq!(processed, callback_frames);
        black_box(block);
    }

    started.map_or(Duration::ZERO, |instant| instant.elapsed())
}

fn median(values: &[u128]) -> u128 {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted[sorted.len() / 2]
}

#[test]
#[ignore = "manual AUD139 matched Peak/LowShelf/HighShelf CPU comparison"]
fn capture_aud139_matched_peak_and_shelf_callback_cpu() {
    for channels in [2, 8] {
        for num_bands in [4, 8] {
            for callback_frames in [64, 256, 1_024] {
                let mut plugins = ShapeCase::ALL
                    .into_iter()
                    .map(|shape| {
                        DynamicEqPlugin::try_from_params_at_sample_rate(
                            channels,
                            params_for(channels, num_bands, shape),
                            SAMPLE_RATE,
                        )
                        .expect("valid matched CPU parameters")
                    })
                    .collect::<Vec<_>>();

                for warmup in 0..WARMUP_TRIALS {
                    for offset in 0..ShapeCase::ALL.len() {
                        let shape_index = (warmup + offset) % ShapeCase::ALL.len();
                        let plugin = &mut plugins[shape_index];
                        plugin.reset();
                        let _ = run_trial(plugin, channels, callback_frames, false);
                    }
                }

                let mut trial_ns = [
                    Vec::with_capacity(MEASURED_TRIALS),
                    Vec::with_capacity(MEASURED_TRIALS),
                    Vec::with_capacity(MEASURED_TRIALS),
                ];
                for trial in 0..MEASURED_TRIALS {
                    // Rotate the first shape in each round to reduce fixed-order bias.
                    for offset in 0..ShapeCase::ALL.len() {
                        let shape_index = (trial + offset) % ShapeCase::ALL.len();
                        let plugin = &mut plugins[shape_index];
                        plugin.reset();
                        let elapsed = run_trial(plugin, channels, callback_frames, true);
                        trial_ns[shape_index].push(elapsed.as_nanos());
                    }
                }

                let medians = trial_ns.each_ref().map(|samples| median(samples));
                let peak_ns = medians[0];
                assert!(peak_ns > 0, "Peak timing must be nonzero");
                for (index, shape) in ShapeCase::ALL.into_iter().enumerate() {
                    let ratio_to_peak = medians[index] as f64 / peak_ns as f64;
                    let ns_per_frame = medians[index] as f64 / TRIAL_FRAMES as f64;
                    let ns_per_callback =
                        medians[index] as f64 / (TRIAL_FRAMES / callback_frames) as f64;
                    println!(
                        "AUD139_CPU_COMPARE profile={BUILD_PROFILE} sample_rate={SAMPLE_RATE} channels={channels} bands={num_bands} callback_frames={callback_frames} frames_per_trial={TRIAL_FRAMES} warmups={WARMUP_TRIALS} measured_trials={MEASURED_TRIALS} shelf_slope={SHELF_SLOPE} shape={} median_ns={} median_ns_per_frame={ns_per_frame:.3} median_ns_per_callback={ns_per_callback:.3} ratio_to_peak={ratio_to_peak:.6} accepted_shelf_ratio_max=1.25 trial_ns={:?}",
                        shape.name(),
                        medians[index],
                        trial_ns[index],
                    );
                }
            }
        }
    }
}
