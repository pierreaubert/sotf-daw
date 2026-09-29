//! Independent gain and state-transition oracles through the public audio API.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_gate::{GateMode, GatePlugin, GatePluginParams};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static DEALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct CallbackAllocator;

// SAFETY: Original layouts/pointers are forwarded to System. The counters use
// constant-initialized TLS cells without allocating, sharing, or borrowing.
unsafe impl GlobalAlloc for CallbackAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = TRACKING.try_with(|tracking| {
            if tracking.get() {
                ALLOCATIONS.with(|count| count.set(count.get() + 1));
            }
        });
        // SAFETY: Forward the caller's allocation contract unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        let _ = TRACKING.try_with(|tracking| {
            if tracking.get() {
                DEALLOCATIONS.with(|count| count.set(count.get() + 1));
            }
        });
        // SAFETY: Forward the original allocation's pointer and layout.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CallbackAllocator = CallbackAllocator;

const SAMPLE_RATE: u32 = 48_000;
const THRESHOLD_DB: f64 = -20.0;
const RATIO: f64 = 4.0;
const KNEE_DB: f64 = 6.0;
// The production fast logarithm/exponential and f32 envelope incur small errors.
// This is also the settled-audio tolerance used for the existing hard-knee oracle.
const GAIN_TOLERANCE_DB: f64 = 0.02;

fn amplitude(level_db: f64) -> f32 {
    10.0_f64.powf(level_db / 20.0) as f32
}

fn attenuation(level_db: f64, threshold_db: f64, knee_db: f64) -> f64 {
    let distance = threshold_db - level_db;
    let attenuation = if knee_db < 0.1 {
        distance.max(0.0) * (RATIO - 1.0)
    } else if distance <= -knee_db / 2.0 {
        0.0
    } else if distance >= knee_db / 2.0 {
        distance * (RATIO - 1.0)
    } else {
        (RATIO - 1.0) * (distance + knee_db / 2.0).powi(2) / (2.0 * knee_db)
    };
    attenuation.min(120.0)
}

fn params(linked: bool) -> GatePluginParams {
    GatePluginParams {
        threshold_db: THRESHOLD_DB as f32,
        ratio: RATIO as f32,
        knee_db: KNEE_DB as f32,
        attack_ms: 1.0,
        hold_ms: 0.0,
        release_ms: 10.0,
        hysteresis_db: 0.0,
        range_db: 120.0,
        link_channels: linked,
        ..GatePluginParams::default()
    }
}

fn gate(channels: usize, params: GatePluginParams) -> GatePlugin {
    let mut gate = GatePlugin::try_from_params(channels, params).unwrap();
    gate.initialize(SAMPLE_RATE).unwrap();
    gate
}

fn process_dc(gate: &mut GatePlugin, levels_db: &[f64], frames: usize) -> Vec<f32> {
    let mut audio = Vec::with_capacity(frames * levels_db.len());
    for _ in 0..frames {
        audio.extend(levels_db.iter().map(|&level| amplitude(level)));
    }
    gate.process_in_place(&mut audio, &ProcessContext::new(SAMPLE_RATE, frames))
        .unwrap();
    audio
}

fn gain_db(output: f32, input: f32) -> f64 {
    20.0 * (f64::from(output) / f64::from(input)).log10()
}

#[test]
fn full_soft_knee_reaches_audio_in_linked_and_unlinked_paths() {
    // At the center the analytical gain is -2.25 dB, not unity. Include both
    // knee edges and close probes on either side of the old discontinuity.
    let offsets = [
        -6.0, -3.01, -3.0, -2.99, -1.0, -0.01, 0.0, 0.01, 1.0, 2.99, 3.0, 3.01, 6.0,
    ];
    for (channels, linked) in [(1, false), (2, false), (2, true)] {
        for offset in offsets {
            let level = THRESHOLD_DB + offset;
            let mut gate = gate(channels, params(linked));
            let levels = vec![level; channels];
            // 25 release time constants suppress initialization history.
            let audio = process_dc(&mut gate, &levels, SAMPLE_RATE as usize / 4);
            let expected = -attenuation(level, THRESHOLD_DB, KNEE_DB);
            for &output in &audio[audio.len() - channels..] {
                let measured = gain_db(output, amplitude(level));
                assert!(
                    (measured - expected).abs() < GAIN_TOLERANCE_DB,
                    "channels={channels} linked={linked} level={level}: gain {measured} dB, expected {expected} dB"
                );
            }
        }
    }
}

#[test]
fn linked_knee_uses_the_loudest_detector_without_changing_channel_balance() {
    let levels = [-19.0, -23.0];
    for linked in [false, true] {
        let mut gate = gate(2, params(linked));
        let audio = process_dc(&mut gate, &levels, SAMPLE_RATE as usize / 4);
        for channel in 0..2 {
            let detector_db = if linked { levels[0] } else { levels[channel] };
            let expected = -attenuation(detector_db, THRESHOLD_DB, KNEE_DB);
            let measured = gain_db(audio[audio.len() - 2 + channel], amplitude(levels[channel]));
            assert!((measured - expected).abs() < GAIN_TOLERANCE_DB);
        }
    }
}

#[test]
fn default_hard_knee_retains_its_transfer_curve() {
    for (channels, linked) in [(1, false), (2, false), (2, true)] {
        for level in [-23.0, -20.01, -20.0, -19.99, -17.0] {
            let mut configuration = params(linked);
            configuration.knee_db = GatePluginParams::default().knee_db;
            let mut gate = gate(channels, configuration);
            let audio = process_dc(&mut gate, &vec![level; channels], SAMPLE_RATE as usize / 4);
            let expected = -attenuation(level, THRESHOLD_DB, 0.0);
            let measured = gain_db(audio[audio.len() - 1], amplitude(level));
            assert!((measured - expected).abs() < GAIN_TOLERANCE_DB);
        }
    }
}

#[test]
fn hysteresis_and_hold_are_anchored_to_the_upper_knee_edge() {
    // Opening = -17 dB; closing = -21 dB. A level of -18 dB therefore
    // attenuates from a closed state, but retains unity after opening.
    for (channels, linked) in [(1, false), (2, false), (2, true)] {
        let mut configuration = params(linked);
        configuration.hysteresis_db = 4.0;
        configuration.hold_ms = 2.0;
        let mut gate = gate(channels, configuration);
        let closed = process_dc(&mut gate, &vec![-18.0; channels], SAMPLE_RATE as usize / 4);
        let closed_gain = gain_db(closed[closed.len() - 1], amplitude(-18.0));
        assert!(
            (closed_gain + attenuation(-18.0, THRESHOLD_DB, KNEE_DB)).abs() < GAIN_TOLERANCE_DB
        );

        process_dc(&mut gate, &vec![-16.0; channels], SAMPLE_RATE as usize / 4);
        let latched = process_dc(&mut gate, &vec![-18.0; channels], 257);
        assert!(latched.iter().all(|&sample| sample == amplitude(-18.0)));

        let hold_frames = SAMPLE_RATE as usize * 2 / 1000;
        let closing = process_dc(&mut gate, &vec![-22.0; channels], hold_frames + 480);
        let target_db = attenuation(-22.0, THRESHOLD_DB, KNEE_DB);
        for (frame, samples) in closing.chunks_exact(channels).enumerate() {
            let release_samples = (frame + 1).saturating_sub(hold_frames);
            // 10 ms release is a one-pole time constant in dB.
            let expected_db = -target_db * (1.0 - (-(release_samples as f64) / 480.0).exp());
            for &sample in samples {
                let actual_db = gain_db(sample, amplitude(-22.0));
                assert!(
                    (actual_db - expected_db).abs() < GAIN_TOLERANCE_DB,
                    "channels={channels} linked={linked} frame={frame}: {actual_db} vs {expected_db} dB"
                );
                if release_samples == 0 {
                    assert_eq!(sample, amplitude(-22.0));
                }
            }
        }
        assert!(gain_db(closing[hold_frames * channels], amplitude(-22.0)) < -0.005);
    }
}

fn process_partitioned(gate: &mut GatePlugin, frames: usize, partitions: &[usize]) -> Vec<f32> {
    let channels = gate.channels();
    let mut result = Vec::with_capacity(frames * channels);
    let mut remaining = frames;
    for &capacity in partitions.iter().cycle() {
        if remaining == 0 {
            break;
        }
        let count = remaining.min(capacity);
        result.extend(process_dc(gate, &vec![-19.0; channels], count));
        remaining -= count;
    }
    result
}

#[test]
fn live_knee_and_smoothed_threshold_updates_preserve_partition_and_reset_behavior() {
    for linked in [false, true] {
        let mut configuration = params(linked);
        configuration.knee_db = 0.0;
        let mut whole = gate(2, configuration.clone());
        let mut split = gate(2, configuration);
        for (threshold, knee) in [(-20.0, 0.0), (-20.0, 6.0), (-16.0, 6.0), (-20.0, 12.0)] {
            for plugin in [&mut whole, &mut split] {
                plugin
                    .parametric_set_parameter(
                        ParameterId::from("threshold"),
                        ParameterValue::Float(threshold),
                    )
                    .unwrap();
                plugin
                    .parametric_set_parameter(
                        ParameterId::from("knee_db"),
                        ParameterValue::Float(knee),
                    )
                    .unwrap();
            }
            let rendered = process_partitioned(&mut whole, 12_000, &[12_000]);
            let partitioned = process_partitioned(&mut split, 12_000, &[1, 7, 61, 256]);
            assert_eq!(rendered, partitioned);
            let expected_db = -attenuation(-19.0, f64::from(threshold), f64::from(knee));
            let actual_db = gain_db(rendered[rendered.len() - 1], amplitude(-19.0));
            assert!((actual_db - expected_db).abs() < GAIN_TOLERANCE_DB);
        }
        whole.reset();
        let mut fresh_configuration = params(linked);
        fresh_configuration.knee_db = 12.0;
        let mut fresh = gate(2, fresh_configuration);
        assert_eq!(
            process_partitioned(&mut whole, 2048, &[127]),
            process_partitioned(&mut fresh, 2048, &[31, 256])
        );
    }
}

#[test]
fn cold_audio_knee_updates_and_reset_allocate_and_free_nothing() {
    for mode in [GateMode::Downward, GateMode::Upward, GateMode::Duck] {
        for linked in [false, true] {
            for external in [false, true] {
                for filtered in [false, true] {
                    let mut configuration = params(linked);
                    configuration.mode = mode;
                    configuration.sidechain_external = external;
                    if filtered {
                        configuration.sidechain_hpf_hz = 120.0;
                        configuration.sidechain_hpf_order = "4th".into();
                        configuration.detection_mode = "rms".into();
                        configuration.lookahead_ms = 10.0;
                    }
                    let mut gate = gate(2, configuration);
                    let mut buffer = vec![0.1; 4096 * gate.input_channels()];
                    let boost_id = ParameterId::from("max_boost_db");
                    let mode_id = ParameterId::from("mode");
                    let knee_id = ParameterId::from("knee_db");
                    let threshold_id = ParameterId::from("threshold");
                    let counts = std::thread::spawn(move || {
                        let context = ProcessContext::new(SAMPLE_RATE, 4096);
                        ALLOCATIONS.with(|counter| counter.set(0));
                        DEALLOCATIONS.with(|counter| counter.set(0));
                        TRACKING.with(|tracking| tracking.set(true));
                        gate.process_in_place(&mut buffer, &context).unwrap();
                        gate.parametric_set_parameter(
                            mode_id.clone(),
                            ParameterValue::Int(mode.index() as i32),
                        )
                        .unwrap();
                        for knee in [12.0, 0.0, 6.0] {
                            gate.parametric_set_parameter(
                                boost_id.clone(),
                                ParameterValue::Float(knee),
                            )
                            .unwrap();
                            gate.parametric_set_parameter(
                                knee_id.clone(),
                                ParameterValue::Float(knee),
                            )
                            .unwrap();
                            gate.parametric_set_parameter(
                                threshold_id.clone(),
                                ParameterValue::Float(-25.0),
                            )
                            .unwrap();
                            buffer.fill(0.1);
                            gate.process_in_place(&mut buffer, &context).unwrap();
                        }
                        gate.reset();
                        buffer.fill(0.1);
                        gate.process_in_place(&mut buffer, &context).unwrap();
                        TRACKING.with(|tracking| tracking.set(false));
                        (ALLOCATIONS.with(Cell::get), DEALLOCATIONS.with(Cell::get))
                    })
                    .join()
                    .unwrap();
                    assert_eq!(
                        counts,
                        (0, 0),
                        "linked={linked} external={external} filtered={filtered}"
                    );
                }
            }
        }
    }
}
