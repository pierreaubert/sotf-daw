//! Matched optimized callback-cost reference for AUD139.
//!
//! This is a manual benchmark, not a pass/fail performance test. Run it before
//! changing Dynamic EQ production code, then rerun the same source and command
//! after implementation. It reports the measured distribution and never
//! claims a worst-case execution time.

use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_plugin_dynamic_eq::{DynEqBandParams, DynamicEqPlugin, DynamicEqPluginParams};
use std::hint::black_box;
use std::time::{Duration, Instant};

const SAMPLE_RATE: u32 = 48_000;
const TRIAL_FRAMES: usize = 65_536;
const WARMUP_TRIALS: usize = 2;
const MEASURED_TRIALS: usize = 7;

fn params_for(channels: usize, num_bands: usize) -> DynamicEqPluginParams {
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
            .expect("pre-edit benchmark callback is valid");
        assert_eq!(processed, callback_frames);
        black_box(block);
    }

    started.map_or(Duration::ZERO, |instant| instant.elapsed())
}

fn median(values: &mut [u128]) -> u128 {
    values.sort_unstable();
    values[values.len() / 2]
}

#[test]
#[ignore = "manual AUD139 optimized pre-edit CPU baseline"]
fn capture_aud139_optimized_callback_cpu_baseline() {
    for channels in [2, 8] {
        for num_bands in [4, 8] {
            for callback_frames in [64, 256, 1_024] {
                let params = params_for(channels, num_bands);
                let mut plugin = DynamicEqPlugin::try_from_params_at_sample_rate(
                    channels,
                    params,
                    SAMPLE_RATE,
                )
                .expect("valid benchmark parameters");

                for _ in 0..WARMUP_TRIALS {
                    plugin.reset();
                    let _ = run_trial(&mut plugin, channels, callback_frames, false);
                }

                let mut elapsed_ns = Vec::with_capacity(MEASURED_TRIALS);
                for _ in 0..MEASURED_TRIALS {
                    plugin.reset();
                    let elapsed = run_trial(&mut plugin, channels, callback_frames, true);
                    elapsed_ns.push(elapsed.as_nanos());
                }

                let median_ns = median(&mut elapsed_ns.clone());
                let ns_per_frame = median_ns as f64 / TRIAL_FRAMES as f64;
                let ns_per_callback = median_ns as f64
                    / (TRIAL_FRAMES / callback_frames) as f64;
                println!(
                    "AUD139_CPU case=pre_edit profile=release sample_rate={SAMPLE_RATE} channels={channels} bands={num_bands} callback_frames={callback_frames} frames_per_trial={TRIAL_FRAMES} warmups={WARMUP_TRIALS} measured_trials={MEASURED_TRIALS} median_ns={median_ns} median_ns_per_frame={ns_per_frame:.3} median_ns_per_callback={ns_per_callback:.3} trial_ns={elapsed_ns:?}"
                );
            }
        }
    }
}
