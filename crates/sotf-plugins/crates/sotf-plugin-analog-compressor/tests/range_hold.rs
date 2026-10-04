//! Gain-domain limits and detector-hold timing, checked against closed-form references.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static OPERATIONS: Cell<usize> = const { Cell::new(0) };
}
struct TrackingAllocator;
fn record() {
    let _ = TRACKING.try_with(|tracking| {
        if tracking.get() {
            let _ = OPERATIONS.try_with(|count| count.set(count.get() + 1));
        }
    });
}
// SAFETY: all memory operations delegate unchanged to System. The thread-local
// counters neither allocate nor inspect the allocated memory.
unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record();
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        record();
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

#[test]
fn new_controls_processing_and_reset_allocate_nothing_on_cold_thread() {
    let mut plugin = AnalogCompressorPlugin::from_params(2, params()).unwrap();
    plugin.initialize(48_000.0).unwrap();
    let ids = ["range_db", "hold_ms"].map(ParameterId::from);
    std::thread::spawn(move || {
        let mut buffer = [0.2; 514];
        OPERATIONS.set(0);
        TRACKING.set(true);
        for repeat in 0..32 {
            for (id, value) in ids.iter().zip([3.0, 7.5]) {
                plugin
                    .set_parameter(id.clone(), ParameterValue::Float(value))
                    .unwrap();
            }
            let frames = [1, 127, 257, 3][repeat % 4];
            buffer.fill(0.2);
            plugin
                .process_in_place(
                    &mut buffer[..frames * 2],
                    &ProcessContext::new(48_000, frames),
                )
                .unwrap();
        }
        plugin.reset();
        TRACKING.set(false);
        assert_eq!(
            OPERATIONS.get(),
            0,
            "allocations or deallocations on callback"
        );
    })
    .join()
    .unwrap();
}

#[test]
fn range_zero_preserves_linked_signal_and_automation_caps_existing_reduction() {
    let mut plugin = AnalogCompressorPlugin::from_params(2, params()).unwrap();
    plugin.initialize(48_000.0).unwrap();
    let mut hot = [1.0; 4096];
    plugin
        .process_in_place(&mut hot, &ProcessContext::new(48_000, 2048))
        .unwrap();
    plugin
        .set_parameter(ParameterId::from("range_db"), ParameterValue::Float(3.0))
        .unwrap();
    let mut pair = [1.0, 0.125];
    plugin
        .process_in_place(&mut pair, &ProcessContext::new(48_000, 1))
        .unwrap();
    assert!((reduction(pair[0], 1.0) - 3.0).abs() < 1e-6);
    assert!((reduction(pair[1], 0.125) - 3.0).abs() < 1e-6);
    plugin
        .set_parameter(ParameterId::from("range_db"), ParameterValue::Float(0.0))
        .unwrap();
    pair = [1.0, 0.125];
    plugin
        .process_in_place(&mut pair, &ProcessContext::new(48_000, 1))
        .unwrap();
    assert_eq!(pair, [1.0, 0.125]);
}

use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_plugin_analog_compressor::{AnalogCompressorPlugin, AnalogCompressorPluginParams};

fn params() -> AnalogCompressorPluginParams {
    AnalogCompressorPluginParams {
        threshold: -24.0,
        ratio: 4.0,
        attack: 0.1,
        release: 20.0,
        knee: 0.0,
        analog_color: 0.0,
        ..Default::default()
    }
}

fn render(p: AnalogCompressorPluginParams, sr: u32, input: &[f32], blocks: &[usize]) -> Vec<f32> {
    let mut plugin = AnalogCompressorPlugin::from_params(1, p).unwrap();
    plugin.initialize(f64::from(sr)).unwrap();
    let mut output = input.to_vec();
    let mut pos = 0;
    let mut block = 0;
    while pos < output.len() {
        let n = blocks[block % blocks.len()].min(output.len() - pos);
        plugin
            .process_in_place(&mut output[pos..pos + n], &ProcessContext::new(sr, n))
            .unwrap();
        pos += n;
        block += 1;
    }
    output
}

fn curve(level_db: f64, threshold: f64, ratio: f64, knee: f64) -> f64 {
    let over = level_db - threshold;
    let slope = 1.0 - 1.0 / ratio;
    if knee == 0.0 {
        over.max(0.0) * slope
    } else if over <= -knee / 2.0 {
        0.0
    } else if over >= knee / 2.0 {
        over * slope
    } else {
        slope * (over + knee / 2.0).powi(2) / (2.0 * knee)
    }
}

fn reduction(output: f32, input: f32) -> f64 {
    -20.0 * (output as f64 / input as f64).log10()
}

#[test]
fn range_caps_static_curve_before_makeup_mix_and_color() {
    for ratio in [1.0_f32, 4.0, 20.0] {
        for knee in [0.0_f32, 12.0] {
            for level_db in [-48.0_f64, -24.0, -20.0, 0.0] {
                for range in [0.0_f32, 3.0, 12.0, 120.0] {
                    let level = 10.0_f64.powf(level_db / 20.0) as f32;
                    let p = AnalogCompressorPluginParams {
                        ratio,
                        knee,
                        range_db: range,
                        makeup: 2.0,
                        mix: 0.6,
                        ..params()
                    };
                    let output = render(p, 48_000, &vec![level; 2048], &[127, 1, 251]);
                    let raw = curve(level_db, -24.0, ratio as f64, knee as f64);
                    let gr = if range == 120.0 {
                        raw
                    } else {
                        raw.min(range as f64)
                    };
                    let expected = 0.4 + 0.6 * 10.0_f64.powf((2.0 - gr) / 20.0);
                    let actual = output[2047] as f64 / level as f64;
                    assert!(
                        (actual - expected).abs() < 5e-6,
                        "ratio={ratio} knee={knee} level={level_db} range={range}: {actual} != {expected}"
                    );
                }
            }
        }
    }
}

#[test]
fn hold_keeps_reduction_then_resumes_original_release_at_each_rate() {
    for sr in [44_100, 48_000, 96_000] {
        for hold_ms in [0.0_f32, 1.75, 20.0] {
            for (ratio, knee, low) in [(4.0_f32, 0.0_f32, 0.01_f32), (2.0, 12.0, 0.08)] {
                let high = sr as usize / 50;
                let hold = (hold_ms as f64 * sr as f64 / 1000.0).round() as usize;
                let tail = sr as usize / 50;
                let mut input = vec![1.0; high];
                input.resize(high + hold + tail + 1, low);
                let p = AnalogCompressorPluginParams {
                    hold_ms,
                    ratio,
                    knee,
                    ..params()
                };
                let output = render(p.clone(), sr, &input, &[1, 7, 127, 3, 257]);
                assert_eq!(output, render(p, sr, &input, &[511, 13]));
                for (offset, &sample) in output[high..].iter().enumerate() {
                    let released = (offset + 1).saturating_sub(hold);
                    let env = low as f64
                        + (1.0 - low as f64) * (-(released as f64) / (0.020 * sr as f64)).exp();
                    let expected = curve(20.0 * env.log10(), -24.0, ratio as f64, knee as f64);
                    assert!(
                        (reduction(sample, low) - expected).abs() < 0.002,
                        "sr={sr} hold={hold_ms} ratio={ratio} knee={knee} offset={offset}"
                    );
                }
            }
        }
    }
}

#[test]
fn later_peak_retriggers_hold_and_reset_removes_it() {
    let p = AnalogCompressorPluginParams {
        hold_ms: 10.0,
        range_db: 9.0,
        ..params()
    };
    let mut input = vec![1.0; 960];
    input.resize(1200, 0.01);
    input.extend(vec![1.0; 240]);
    input.resize(1921, 0.01);
    let output = render(p.clone(), 48_000, &input, &[17, 256, 3]);
    for index in [959, 960, 1199, 1439, 1440, 1919] {
        assert!((reduction(output[index], input[index]) - 9.0).abs() < 0.002);
    }
    let mut plugin = AnalogCompressorPlugin::from_params(1, p).unwrap();
    plugin.initialize(48_000.0).unwrap();
    let mut hot = [1.0; 960];
    plugin
        .process_in_place(&mut hot, &ProcessContext::new(48_000, 960))
        .unwrap();
    plugin.reset();
    let mut quiet = [0.01; 16];
    plugin
        .process_in_place(&mut quiet, &ProcessContext::new(48_000, 16))
        .unwrap();
    assert_eq!(quiet, [0.01; 16]);
    for key in ["range_db", "hold_ms"] {
        for invalid in [-1.0, f32::NAN, f32::INFINITY, 1001.0] {
            assert!(
                plugin
                    .set_parameter(ParameterId::from(key), ParameterValue::Float(invalid))
                    .is_err()
            );
        }
    }
}

#[test]
fn old_presets_and_appended_indices_roundtrip() {
    use sotf_host::plugin_params::PluginParamDef;
    let old: AnalogCompressorPluginParams = serde_json::from_str("{}").unwrap();
    assert_eq!((old.range_db, old.hold_ms), (120.0, 0.0));
    let mut p = params();
    p.set_param_value(13, 4.0);
    p.set_param_value(14, 23.0);
    let decoded: AnalogCompressorPluginParams =
        serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
    assert_eq!((decoded.range_db, decoded.hold_ms), (4.0, 23.0));
}
