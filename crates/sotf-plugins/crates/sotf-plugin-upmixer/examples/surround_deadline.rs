//! Measures individual surround callbacks against their available audio time.

// Rust guideline compliant 2026-10-09

use sotf_host::speaker_config::{ChannelLayout, get_speaker_config};
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_upmixer::{SurroundBassMode, UpmixerPlugin, UpmixerPluginParams};
use std::time::Instant;

const BLOCKS: usize = 2048;
const INPUT_FRAMES: usize = 32_768;

fn main() -> Result<(), String> {
    println!(
        "input,output,bass,sample_rate,frames,mean_us,p99_us,p999_us,max_us,deadline_us,misses,max_cpu_us,cpu_misses,wall_misses_within_cpu_deadline,checksum"
    );
    for (input_id, output_id, mode) in [
        ("5.0", "5.1.4", SurroundBassMode::Off),
        ("5.1", "7.1.4", SurroundBassMode::Derived),
        ("7.1", "9.1.6", SurroundBassMode::Managed),
    ] {
        for sample_rate in [48_000_u32, 96_000] {
            for frames in [64_usize, 128, 512] {
                measure(input_id, output_id, mode, sample_rate, frames)?;
            }
        }
    }
    Ok(())
}

fn measure(
    input_id: &str,
    output_id: &str,
    mode: SurroundBassMode,
    sample_rate: u32,
    frames: usize,
) -> Result<(), String> {
    let layout = ChannelLayout::from_speaker_config(
        get_speaker_config(input_id).ok_or("missing input layout")?,
    )?;
    let channels = layout.channels.len();
    let mut params = UpmixerPluginParams::default();
    params.core.speaker_config = output_id.into();
    params.multichannel.input_layout = Some(layout);
    params.multichannel.bass_mode = mode;
    params.multichannel.combine_input_lfe = true;
    let mut plugin = UpmixerPlugin::try_from_params(params)?;
    plugin.initialize(f64::from(sample_rate))?;

    let input: Vec<f32> = (0..INPUT_FRAMES * channels)
        .map(|index| {
            let frame = index / channels;
            let channel = index % channels;
            let t = frame as f32 / sample_rate as f32;
            let phase = channel as f32 * 0.37;
            0.18 * (std::f32::consts::TAU * (137.0 + channel as f32 * 53.0) * t + phase).sin()
                + 0.09 * (std::f32::consts::TAU * 3179.0 * t + phase * 1.7).sin()
        })
        .collect();
    let mut output = vec![0.0; frames * plugin.output_channels()];
    let context = ProcessContext::new(sample_rate, frames);
    for block in 0..128 {
        let start = (block * frames % INPUT_FRAMES) * channels;
        plugin.process(
            &input[start..start + frames * channels],
            &mut output,
            &context,
        )?;
    }
    let mut durations = Vec::with_capacity(BLOCKS);
    let mut cpu_durations = Vec::with_capacity(BLOCKS);
    let mut checksum = 0.0_f64;
    for block in 0..BLOCKS {
        let input_start = ((block + 128) * frames % INPUT_FRAMES) * channels;
        let start = Instant::now();
        let cpu_start = thread_cpu_nanos();
        plugin.process(
            &input[input_start..input_start + frames * channels],
            &mut output,
            &context,
        )?;
        let cpu_end = thread_cpu_nanos();
        durations.push(start.elapsed().as_nanos() as f64 / 1000.0);
        cpu_durations.push(
            cpu_start
                .zip(cpu_end)
                .map(|(start, end)| end.saturating_sub(start) as f64 / 1000.0),
        );
        checksum += f64::from(output[1]);
    }
    if !checksum.is_finite() || checksum == 0.0 {
        return Err("benchmark produced no finite audio".into());
    }
    let mean = durations.iter().sum::<f64>() / BLOCKS as f64;
    let deadline = frames as f64 / f64::from(sample_rate) * 1_000_000.0;
    let misses = durations.iter().filter(|time| **time > deadline).count();
    let cpu_misses = cpu_durations
        .iter()
        .flatten()
        .filter(|time| **time > deadline)
        .count();
    let max_cpu = cpu_durations
        .iter()
        .flatten()
        .copied()
        .fold(0.0_f64, f64::max);
    let wall_misses_within_cpu_deadline = durations
        .iter()
        .zip(&cpu_durations)
        .filter(|(wall, cpu)| **wall > deadline && cpu.is_some_and(|time| time <= deadline))
        .count();
    let cpu_available = cpu_durations.iter().all(Option::is_some);
    let (max_cpu_field, cpu_misses_field, low_cpu_misses_field) = if cpu_available {
        (
            format!("{max_cpu:.2}"),
            cpu_misses.to_string(),
            wall_misses_within_cpu_deadline.to_string(),
        )
    } else {
        (String::new(), String::new(), String::new())
    };
    durations.sort_unstable_by(f64::total_cmp);
    println!(
        "{input_id},{output_id},{mode:?},{sample_rate},{frames},{mean:.2},{:.2},{:.2},{:.2},{deadline:.2},{misses},{max_cpu_field},{cpu_misses_field},{low_cpu_misses_field},{checksum:.3}",
        durations[BLOCKS * 99 / 100],
        durations[BLOCKS * 999 / 1000],
        durations[BLOCKS - 1]
    );
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn thread_cpu_nanos() -> Option<u64> {
    let mut timestamp = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `timestamp` is valid writable storage and the clock ID requests the current thread.
    let result = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut timestamp) };
    if result != 0 {
        return None;
    }
    let seconds = u64::try_from(timestamp.tv_sec).ok()?;
    let nanos = u64::try_from(timestamp.tv_nsec).ok()?;
    seconds.checked_mul(1_000_000_000)?.checked_add(nanos)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn thread_cpu_nanos() -> Option<u64> {
    None
}
