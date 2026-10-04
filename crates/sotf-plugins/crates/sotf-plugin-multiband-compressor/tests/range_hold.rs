//! Independent gain and timing oracles for compressor range and hold.

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
fn scalar_automation_preserves_running_audio_without_callback_heap_operations() {
    for bands in [1, 3, 5] {
        let config = MultibandCompressorPluginParams {
            num_bands: bands,
            ..Default::default()
        };
        let mut actual = MultibandCompressorPlugin::with_params(2, config.clone());
        let mut reference = MultibandCompressorPlugin::with_params(2, config);
        actual.initialize(48_000.0).unwrap();
        reference.initialize(48_000.0).unwrap();
        let mut changes = vec![
            ("threshold".to_owned(), ParameterValue::Float(-31.0)),
            ("ratio".to_owned(), ParameterValue::Float(7.0)),
            ("attack".to_owned(), ParameterValue::Float(3.0)),
            ("release".to_owned(), ParameterValue::Float(87.0)),
            ("knee".to_owned(), ParameterValue::Float(4.0)),
            ("mix".to_owned(), ParameterValue::Float(0.63)),
            ("link_amount".to_owned(), ParameterValue::Float(0.4)),
            ("sidechain_tilt_db".to_owned(), ParameterValue::Float(2.0)),
        ];
        for band in 0..bands {
            for (field, value) in [
                ("threshold", ParameterValue::Float(-22.0)),
                ("ratio", ParameterValue::Float(3.0)),
                ("attack", ParameterValue::Float(7.0)),
                ("release", ParameterValue::Float(120.0)),
                ("knee", ParameterValue::Float(9.0)),
                ("makeup", ParameterValue::Float(2.5)),
                ("auto_makeup", ParameterValue::Bool(true)),
                ("measured_auto_makeup", ParameterValue::Bool(true)),
                ("active", ParameterValue::Bool(false)),
                ("active", ParameterValue::Bool(true)),
                ("solo", ParameterValue::Bool(true)),
                ("solo", ParameterValue::Bool(false)),
                ("bypass", ParameterValue::Bool(true)),
                ("bypass", ParameterValue::Bool(false)),
            ] {
                changes.push((format!("band_{band}_{field}"), value));
            }
        }
        if bands == 1 {
            changes.extend([
                ("makeup_gain".to_owned(), ParameterValue::Float(2.5)),
                ("auto_makeup".to_owned(), ParameterValue::Bool(true)),
                (
                    "measured_auto_makeup".to_owned(),
                    ParameterValue::Bool(true),
                ),
            ]);
        }
        // Single-band exports publish aliases instead of expanded band IDs.
        let schema = actual.parameter_schema();
        changes.retain(|(name, _)| schema.iter().any(|parameter| parameter.id.as_str() == name));
        assert!(changes.len() >= if bands == 1 { 8 } else { bands * 10 });
        let mut warmup = [0.7; 514];
        actual
            .process_in_place(&mut warmup, &ProcessContext::new(48_000, 257))
            .unwrap();
        warmup.fill(0.7);
        reference
            .process_in_place(&mut warmup, &ProcessContext::new(48_000, 257))
            .unwrap();
        // The reference uses the existing inherent setter, avoiding the trait's
        // schema/map adapter. Keep its running filter/envelope history intact.
        let fixtures: Vec<_> = changes
            .into_iter()
            .enumerate()
            .map(|(index, (name, value))| {
                let id = ParameterId::from(name);
                let frames = [1, 17, 257][index % 3];
                let input: Vec<f32> = (0..frames * 2)
                    .map(|sample| ((sample + index * 31) as f32 * 0.127).sin() * 0.8)
                    .collect();
                let mut expected = input.clone();
                reference.set_parameter(id.clone(), value.clone()).unwrap();
                reference
                    .process_in_place(&mut expected, &ProcessContext::new(48_000, frames))
                    .unwrap();
                (id, value, input, expected)
            })
            .collect();
        std::thread::spawn(move || {
            let mut output = [0.0; 514];
            OPERATIONS.set(0);
            TRACKING.set(true);
            for (id, value, input, expected) in &fixtures {
                actual.parametric_validate_parameter(id, value).unwrap();
                actual
                    .parametric_set_parameter(id.clone(), value.clone())
                    .unwrap();
                assert_eq!(actual.parametric_get_parameter(id), Some(value.clone()));
                output[..input.len()].copy_from_slice(input);
                actual
                    .process_in_place(
                        &mut output[..input.len()],
                        &ProcessContext::new(48_000, input.len() / 2),
                    )
                    .unwrap();
                assert_eq!(&output[..input.len()], expected);
            }
            TRACKING.set(false);
            assert_eq!(
                OPERATIONS.get(),
                0,
                "bands={bands}: callback heap operations"
            );
        })
        .join()
        .unwrap();
    }
}

#[test]
fn scalar_validation_keeps_schema_errors_transactional() {
    let mut plugin = MultibandCompressorPlugin::with_params(
        2,
        MultibandCompressorPluginParams {
            num_bands: 3,
            ..Default::default()
        },
    );
    plugin.initialize(48_000.0).unwrap();
    let before = plugin.current_values();
    let schema = plugin.parameter_schema();
    for (name, value) in [
        ("threshold", ParameterValue::Float(f32::NAN)),
        ("ratio", ParameterValue::Float(f32::INFINITY)),
        ("attack", ParameterValue::Float(-1.0)),
        ("band_2_ratio", ParameterValue::Bool(true)),
        ("band_1_solo", ParameterValue::Float(0.3)),
    ] {
        let id = ParameterId::from(name);
        let parameter = schema.iter().find(|parameter| parameter.id == id).unwrap();
        let expected = parameter.validate(&value).unwrap_err();
        assert_eq!(
            plugin.parametric_set_parameter(id, value).unwrap_err(),
            format!("{name}: {expected}")
        );
        assert_eq!(plugin.current_values(), before);
    }
    assert_eq!(
        plugin
            .parametric_set_parameter(
                ParameterId::from("band_99_ratio"),
                ParameterValue::Float(2.0)
            )
            .unwrap_err(),
        "Unknown parameter: band_99_ratio"
    );
    assert_eq!(plugin.current_values(), before);
}

#[test]
fn scalar_controls_reach_the_independent_static_gain_curve() {
    let mut plugin = MultibandCompressorPlugin::with_params(1, params());
    plugin.initialize(48_000.0).unwrap();
    for (name, value) in [
        ("threshold", -30.0),
        ("ratio", 5.0),
        ("attack", 0.1),
        ("knee", 0.0),
        ("makeup_gain", 3.0),
        ("mix", 0.6),
    ] {
        plugin
            .parametric_set_parameter(ParameterId::from(name), ParameterValue::Float(value))
            .unwrap();
    }
    let mut last = 0.0;
    for _ in 0..64 {
        let mut input = [1.0; 257];
        plugin
            .process_in_place(&mut input, &ProcessContext::new(48_000, 257))
            .unwrap();
        last = input[256];
    }
    // Input0dB, threshold-30dB, ratio5: reduction24dB; +3dB makeup,
    // then 60% wet. Existing fast log/exp approximations bound the error.
    let expected = 0.4 + 0.6 * 10.0_f64.powf(-21.0 / 20.0);
    assert!((f64::from(last) - expected).abs() < 0.0015);
}

#[test]
fn multiband_controls_processing_and_reset_allocate_nothing_on_cold_thread() {
    let mut plugin = MultibandCompressorPlugin::with_params(
        2,
        MultibandCompressorPluginParams {
            num_bands: 3,
            range_db: 6.0,
            hold_ms: 20.0,
            ..Default::default()
        },
    );
    plugin.initialize(48_000.0).unwrap();
    let ids = ["range_db", "hold_ms", "band_1_range_db", "band_2_hold_ms"].map(ParameterId::from);
    std::thread::spawn(move || {
        let mut buffer = [0.2; 514];
        OPERATIONS.set(0);
        TRACKING.set(true);
        for repeat in 0..32 {
            for (id, value) in ids.iter().zip([3.0, 7.5, 2.0, 3.0]) {
                ParametricInPlacePlugin::parametric_set_parameter(
                    &mut plugin,
                    id.clone(),
                    ParameterValue::Float(value),
                )
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
fn cap_is_enforced_immediately_when_automated_during_reduction() {
    let mut plugin = MultibandCompressorPlugin::with_params(1, params());
    plugin.initialize(48_000.0).unwrap();
    let mut hot = [1.0; 2048];
    plugin
        .process_in_place(&mut hot, &ProcessContext::new(48_000, 2048))
        .unwrap();
    assert!(reduction(hot[2047], 1.0) > 17.99);
    plugin
        .set_parameter(ParameterId::from("range_db"), ParameterValue::Float(3.0))
        .unwrap();
    let mut one = [1.0];
    plugin
        .process_in_place(&mut one, &ProcessContext::new(48_000, 1))
        .unwrap();
    assert!(reduction(one[0], 1.0) <= 3.000001);
    plugin
        .set_parameter(ParameterId::from("range_db"), ParameterValue::Float(0.0))
        .unwrap();
    let before = one;
    plugin
        .process_in_place(&mut one, &ProcessContext::new(48_000, 1))
        .unwrap();
    assert_eq!(one, before);
    plugin
        .set_parameter(ParameterId::from("range_db"), ParameterValue::Float(12.0))
        .unwrap();
    plugin
        .set_parameter(ParameterId::from("hold_ms"), ParameterValue::Float(100.0))
        .unwrap();
    hot.fill(1.0);
    plugin
        .process_in_place(&mut hot, &ProcessContext::new(48_000, 2048))
        .unwrap();
    let mut quiet = [0.01; 1];
    plugin.reset();
    plugin
        .process_in_place(&mut quiet, &ProcessContext::new(48_000, 1))
        .unwrap();
    assert_eq!(quiet, [0.01]);
}

use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_plugin_multiband_compressor::{
    BandCompressorParams, MultibandCompressorPlugin, MultibandCompressorPluginParams,
};

fn params() -> MultibandCompressorPluginParams {
    MultibandCompressorPluginParams {
        num_bands: 1,
        threshold_db: -24.0,
        ratio: 4.0,
        attack_ms: 0.1,
        release_ms: 20.0,
        knee_db: 0.0,
        ..Default::default()
    }
}

fn render(
    p: MultibandCompressorPluginParams,
    sr: u32,
    input: &[f32],
    blocks: &[usize],
) -> Vec<f32> {
    let mut plugin = MultibandCompressorPlugin::try_from_params(1, p, sr).unwrap();
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

fn reduction(output: f32, input: f32) -> f64 {
    -20.0 * (output as f64 / input as f64).log10()
}

#[test]
fn range_caps_static_curve_before_makeup_and_parallel_mix() {
    for ratio in [1.0_f32, 4.0, 20.0] {
        for knee in [0.0_f32, 12.0] {
            for level_db in [-48.0_f64, -24.0, -20.0, 0.0, 180.0] {
                for range in [0.0_f32, 3.0, 12.0, 120.0] {
                    let level = 10.0_f64.powf(level_db / 20.0) as f32;
                    let p = MultibandCompressorPluginParams {
                        ratio,
                        knee_db: knee,
                        range_db: range,
                        makeup_gain: Some(2.0),
                        mix: 0.6,
                        ..params()
                    };
                    let output = render(p, 48_000, &vec![level; 2048], &[127, 1, 251]);
                    let over = level_db + 24.0;
                    let slope = 1.0 - 1.0 / ratio as f64;
                    let raw = if knee == 0.0 {
                        over.max(0.0) * slope
                    } else if over <= -(knee as f64) / 2.0 {
                        0.0
                    } else if over >= knee as f64 / 2.0 {
                        over * slope
                    } else {
                        slope * (over + knee as f64 / 2.0).powi(2) / (2.0 * knee as f64)
                    };
                    let gr = if range == 120.0 {
                        raw
                    } else {
                        raw.min(range as f64)
                    };
                    let expected = 0.4 + 0.6 * 10.0_f64.powf((2.0 - gr) / 20.0);
                    let actual = output[2047] as f64 / level as f64;
                    // Existing fast log/exp approximations have up to 0.08%
                    // relative error each; the mixed output is bounded here.
                    assert!(
                        (actual - expected).abs() < 0.0015,
                        "ratio={ratio} knee={knee} level={level_db} range={range}: {actual} != {expected}"
                    );
                }
            }
        }
    }
}

#[test]
fn hold_duration_and_release_match_sample_clock_and_partitioning() {
    for sr in [44_100, 48_000, 96_000] {
        for hold_ms in [0.0_f32, 1.75, 20.0] {
            let high_frames = sr as usize / 50;
            let hold_frames = (hold_ms as f64 * sr as f64 / 1000.0).round() as usize;
            let release_frames = sr as usize / 50;
            let mut input = vec![1.0; high_frames];
            input.resize(high_frames + hold_frames + release_frames + 1, 0.01);
            let p = MultibandCompressorPluginParams {
                hold_ms,
                ..params()
            };
            let output = render(p.clone(), sr, &input, &[1, 7, 127, 3, 257]);
            let alternate = render(p, sr, &input, &[511, 13]);
            assert_eq!(output, alternate, "partition sr={sr} hold={hold_ms}");
            let peak = reduction(output[high_frames - 1], 1.0);
            assert!((peak - 18.0).abs() < 0.003);
            for (offset, &sample) in output[high_frames..].iter().enumerate() {
                let released = (offset + 1).saturating_sub(hold_frames);
                let expected = peak * (-(released as f64) / (0.020 * sr as f64)).exp();
                // Includes the documented fast log/exp approximation error.
                assert!(
                    (reduction(sample, 0.01) - expected).abs() < 0.015,
                    "sr={sr} hold={hold_ms} offset={offset}"
                );
            }
        }
    }
}

#[test]
fn per_band_overrides_inherit_and_new_peaks_retrigger_hold() {
    let mut p = params();
    p.hold_ms = 1.0;
    p.range_db = 3.0;
    p.bands = vec![BandCompressorParams {
        range_db: Some(9.0),
        hold_ms: Some(10.0),
        ..Default::default()
    }];
    let mut input = vec![1.0; 960];
    input.resize(960 + 240, 0.01);
    input.extend(vec![1.0; 240]);
    input.resize(1440 + 481, 0.01);
    let output = render(p, 48_000, &input, &[17, 256, 3]);
    for index in [959, 960, 1199, 1439, 1440, 1919] {
        assert!(
            (reduction(output[index], input[index]) - 9.0).abs() < 0.003,
            "index={index}"
        );
    }
    assert!(reduction(output[1920], input[1920]) < 8.999);

    let mut plugin = MultibandCompressorPlugin::with_params(
        1,
        MultibandCompressorPluginParams {
            num_bands: 3,
            ..Default::default()
        },
    );
    plugin.initialize(48_000.0).unwrap();
    plugin
        .set_parameter(ParameterId::from("range_db"), ParameterValue::Float(6.0))
        .unwrap();
    plugin
        .set_parameter(ParameterId::from("hold_ms"), ParameterValue::Float(40.0))
        .unwrap();
    plugin
        .set_parameter(
            ParameterId::from("band_1_range_db"),
            ParameterValue::Float(2.0),
        )
        .unwrap();
    plugin
        .set_parameter(
            ParameterId::from("band_1_hold_ms"),
            ParameterValue::Float(5.0),
        )
        .unwrap();
    plugin
        .set_parameter(ParameterId::from("range_db"), ParameterValue::Float(8.0))
        .unwrap();
    for (key, expected) in [
        ("band_0_range_db", 8.0),
        ("band_1_range_db", 2.0),
        ("band_2_range_db", 8.0),
        ("band_0_hold_ms", 40.0),
        ("band_1_hold_ms", 5.0),
    ] {
        assert_eq!(
            plugin.get_parameter(&ParameterId::from(key)),
            Some(ParameterValue::Float(expected))
        );
    }
    for key in ["range_db", "hold_ms", "band_1_range_db", "band_1_hold_ms"] {
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
fn old_presets_keep_unlimited_zero_hold_and_new_fields_roundtrip() {
    let old: MultibandCompressorPluginParams = serde_json::from_str("{}").unwrap();
    assert_eq!((old.range_db, old.hold_ms), (120.0, 0.0));
    let mut p = params();
    p.range_db = 4.0;
    p.hold_ms = 23.0;
    p.bands = vec![BandCompressorParams {
        range_db: Some(7.0),
        hold_ms: Some(11.0),
        ..Default::default()
    }];
    let decoded: MultibandCompressorPluginParams =
        serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
    assert_eq!((decoded.range_db, decoded.hold_ms), (4.0, 23.0));
    assert_eq!(
        (decoded.bands[0].range_db, decoded.bands[0].hold_ms),
        (Some(7.0), Some(11.0))
    );
}
