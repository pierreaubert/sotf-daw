//! Offline paired wall/current-thread CPU diagnostic; legacy QA gates stay separate.

use sotf_host::test_utils::thread_cpu_time;
use sotf_host::{ParameterId, ParameterValue, Plugin, ProcessContext};
use sotf_plugin_aae::{AaePlugin, params::AaePluginParams};
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
struct Sample {
    wall: Duration,
    cpu: Option<Duration>,
}

fn percentile(values: &mut [u128], numerator: usize) -> Option<u128> {
    values.sort_unstable();
    (!values.is_empty()).then(|| values[(values.len() * numerator / 100).min(values.len() - 1)])
}

// Match qa-aae's five-second observation, spanning meter and room-fade epochs.
const OBSERVATION_SECONDS: usize = 5;

fn report_empty_brackets() {
    let mut walls = Vec::with_capacity(128);
    let mut cpus = Vec::with_capacity(128);
    for _ in 0..128 {
        let cpu_begin = thread_cpu_time();
        let wall_begin = Instant::now();
        let wall = wall_begin.elapsed();
        let cpu = cpu_begin
            .zip(thread_cpu_time())
            .and_then(|(a, b)| b.checked_sub(a));
        walls.push(wall.as_nanos());
        if let Some(cpu) = cpu {
            cpus.push(cpu.as_nanos());
        }
    }
    let wall_p50 = percentile(&mut walls, 50);
    let cpu_p50 = percentile(&mut cpus, 50);
    println!(
        "{}",
        serde_json::json!({"kind":"empty_clock_brackets","samples":128,"cpu_samples":cpus.len(),"wall_p50_ns":wall_p50,"cpu_p50_ns":cpu_p50,"wall_max_ns":walls.last(),"cpu_max_ns":cpus.last(),"scope":"CPU bracket contains sampling work; do not subtract this estimate to change deadline decisions"})
    );
}

fn main() {
    report_empty_brackets();
    let mut cases = 0;
    let mut wall_misses = 0;
    let mut cpu_misses = 0;
    let mut missing_cpu = 0;
    // Max output layout with the same cathedral/autogain/content configuration
    // as qa-aae. Every measured block is full-sized, avoiding a partial offline
    // block being mistaken for a shorter device callback period.
    for rate in [44_100_u32, 48_000, 96_000, 192_000] {
        for block in [64, 512] {
            for updates in [false, true] {
                let mut plugin = AaePlugin::try_from_params(AaePluginParams {
                    speaker_config: "9.1.6".into(),
                    room_preset: "cathedral".into(),
                    auto_gain_enabled: true,
                    content_aware: true,
                    ..Default::default()
                })
                .unwrap();
                plugin.initialize(f64::from(rate)).unwrap();
                let input_channels = plugin.input_channels();
                let output_channels = plugin.output_channels();
                let zeros = vec![0.0_f32; block * input_channels];
                let mut warm_output = vec![0.0_f32; block * output_channels];
                let context = ProcessContext::new(rate, block);
                // qa-aae warms ten silent blocks, measures one further silent
                // block for allocations, then prepares these spatial controls.
                for _ in 0..11 {
                    plugin.process(&zeros, &mut warm_output, &context).unwrap();
                }
                let envelopment = ParameterId::from("envelopment");
                let height = ParameterId::from("height_amount");
                for step in 0..100 {
                    let value = step as f32 / 99.0;
                    plugin
                        .set_parameter(envelopment.clone(), ParameterValue::Float(value))
                        .unwrap();
                    plugin
                        .set_parameter(height.clone(), ParameterValue::Float(1.0 - value))
                        .unwrap();
                }
                let blocks = (rate as usize * OBSERVATION_SECONDS).div_ceil(block);
                // Match the legacy benchmark's full-stream memory pass rather
                // than repeatedly reusing a hot block. Final blocks stay full.
                let input = vec![0.1_f32; blocks * block * input_channels];
                let mut output = vec![0.0_f32; blocks * block * output_channels];
                let mut samples = Vec::with_capacity(blocks);
                let rt60 = ParameterId::from("rt60");
                let room = ParameterId::from("room_size");
                for index in 0..blocks {
                    // CPU brackets the wall interval and includes timer overhead.
                    // wall-cpu may therefore be slightly negative; retain it.
                    let cpu_begin = thread_cpu_time();
                    let wall_begin = Instant::now();
                    if updates {
                        let high = index % 2 == 0;
                        plugin
                            .set_parameter(
                                rt60.clone(),
                                ParameterValue::Float(if high { 6.0 } else { 0.3 }),
                            )
                            .unwrap();
                        plugin
                            .set_parameter(
                                room.clone(),
                                ParameterValue::Float(if high { 3.0 } else { 0.2 }),
                            )
                            .unwrap();
                    }
                    plugin
                        .process(
                            &input[index * block * input_channels
                                ..(index + 1) * block * input_channels],
                            &mut output[index * block * output_channels
                                ..(index + 1) * block * output_channels],
                            &context,
                        )
                        .unwrap();
                    let wall = wall_begin.elapsed();
                    let cpu = cpu_begin
                        .zip(thread_cpu_time())
                        .and_then(|(a, b)| b.checked_sub(a));
                    samples.push(Sample { wall, cpu });
                }
                assert!(output.iter().all(|value| value.is_finite()));
                let is_miss = |duration: Duration| {
                    duration.as_nanos() * u128::from(rate) >= block as u128 * 1_000_000_000
                };
                let wall_count = samples.iter().filter(|s| is_miss(s.wall)).count();
                let cpu_count = samples
                    .iter()
                    .filter(|s| s.cpu.is_some_and(is_miss))
                    .count();
                let missing = samples.iter().filter(|s| s.cpu.is_none()).count();
                wall_misses += wall_count;
                cpu_misses += cpu_count;
                missing_cpu += missing;
                let mut walls: Vec<_> = samples.iter().map(|s| s.wall.as_nanos()).collect();
                let mut cpus: Vec<_> = samples
                    .iter()
                    .filter_map(|s| s.cpu.map(|v| v.as_nanos()))
                    .collect();
                let wall_p50 = percentile(&mut walls, 50);
                let wall_p95 = percentile(&mut walls, 95);
                let cpu_p50 = percentile(&mut cpus, 50);
                let cpu_p95 = percentile(&mut cpus, 95);
                let worst = samples.iter().max_by_key(|s| s.wall).unwrap();
                let worst_cpu = worst.cpu.map(|v| v.as_nanos());
                let gap = worst_cpu.map(|cpu| worst.wall.as_nanos() as i128 - cpu as i128);
                let raw: Vec<_> = samples
                    .iter()
                    .map(|s| (s.wall.as_nanos(), s.cpu.map(|v| v.as_nanos())))
                    .collect();
                println!(
                    "{}",
                    serde_json::json!({"kind":"timing","observation_seconds":OBSERVATION_SECONDS,"rate":rate,"block_frames":block,"layout":"9.1.6","buffer_mode":"full_stream","warmup_blocks":11,"spatial_prelude":true,"updates_each_block":updates,"blocks":blocks,"frames":blocks*block,"deadline_ns":block as f64*1e9/f64::from(rate),"wall_p50_ns":wall_p50,"wall_p95_ns":wall_p95,"wall_max_ns":walls.last(),"cpu_p50_ns":cpu_p50,"cpu_p95_ns":cpu_p95,"cpu_max_ns":cpus.last(),"max_wall_callback_cpu_ns":worst_cpu,"max_wall_callback_gap_ns":gap,"wall_deadline_misses":wall_count,"cpu_deadline_misses":cpu_count,"missing_cpu_samples":missing,"sample_columns":["wall_ns","thread_cpu_ns"],"samples":raw})
                );
                cases += 1;
            }
        }
    }
    println!(
        "{}",
        serde_json::json!({"kind":"summary","observation_seconds":OBSERVATION_SECONDS,"cases":cases,"wall_deadline_misses":wall_misses,"cpu_deadline_misses":cpu_misses,"missing_cpu_samples":missing_cpu,"scope":"offline paired clocks; CPU interval includes sampling work; gap is not causal scheduling attribution; does not replace legacy wall/load gates or device validation"})
    );
    if missing_cpu != 0 {
        std::process::exit(2);
    }
    if wall_misses != 0 || cpu_misses != 0 {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::percentile;
    #[test]
    fn percentiles_keep_empty_and_endpoint_cases_explicit() {
        assert_eq!(percentile(&mut [], 50), None);
        assert_eq!(percentile(&mut [7], 95), Some(7));
        assert_eq!(percentile(&mut [9, 1, 5, 3], 50), Some(5));
        assert_eq!(percentile(&mut [9, 1, 5, 3], 100), Some(9));
    }
    #[test]
    fn paired_clocks_distinguish_known_waiting_from_cpu_execution() {
        let Some(cpu_begin) = super::thread_cpu_time() else {
            eprintln!("thread CPU clock unavailable; diagnostic must retain missing coverage");
            return;
        };
        let wall_begin = std::time::Instant::now();
        let wait = std::time::Duration::from_millis(20);
        std::thread::sleep(wait);
        let wall = wall_begin.elapsed();
        let cpu = super::thread_cpu_time()
            .unwrap()
            .checked_sub(cpu_begin)
            .unwrap();
        assert!(wall >= wait);
        assert!(cpu < wall / 2_u32, "CPU clock must exclude the known wait");
        println!(
            "known_wait wall_ns={} thread_cpu_ns={}",
            wall.as_nanos(),
            cpu.as_nanos()
        );
    }
}
