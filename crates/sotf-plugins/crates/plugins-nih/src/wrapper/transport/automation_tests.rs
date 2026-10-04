//! Native event-offset regressions, using the actual NIH CLAP dispatcher.

// Rust guideline compliant 2026-02-21
use super::*;
use clap_sys::ext::params::{CLAP_EXT_PARAMS, clap_param_info, clap_plugin_params};
use std::ffi::CStr;

crate::sotf_nih_plugin!(NativeGain, plugin_type: "Gain", name: "Gain Automation Probe", clap_id: "org.sotf.gain-automation-probe", vst3_class_id: *b"SotfGainAutom001", channels: 2);

impl<P: nih::ClapPlugin> NativeHost<P> {
    pub(super) fn parameter_id(&self, name: &str) -> u32 {
        let plugin = self.wrapper.clap_plugin.as_ptr();
        // SAFETY: query the initialized NIH plugin's parameter extension, whose
        // table and returned metadata remain valid for the plugin lifetime.
        unsafe {
            let extension = ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_PARAMS.as_ptr());
            assert!(!extension.is_null());
            let params = &*extension.cast::<clap_plugin_params>();
            for index in 0..(params.count.unwrap())(plugin) {
                let mut info = std::mem::MaybeUninit::<clap_param_info>::uninit();
                assert!((params.get_info.unwrap())(plugin, index, info.as_mut_ptr()));
                let info = info.assume_init();
                if CStr::from_ptr(info.name.as_ptr()).to_str().unwrap() == name {
                    return info.id;
                }
            }
        }
        panic!("missing native parameter {name}");
    }
}

pub(super) fn change(id: u32, time: u32, value: f64) -> clap_event_param_value {
    clap_event_param_value {
        header: clap_event_header {
            size: std::mem::size_of::<clap_event_param_value>() as u32,
            time,
            space_id: CLAP_CORE_EVENT_SPACE_ID,
            type_: CLAP_EVENT_PARAM_VALUE,
            flags: 0,
        },
        param_id: id,
        cookie: std::ptr::null_mut(),
        note_id: -1,
        port_index: -1,
        channel: -1,
        key: -1,
        value,
    }
}

#[test]
fn native_gain_change_begins_at_the_exact_clap_event_sample() {
    let mut host = NativeHost::<NativeGain>::new(1);
    host.input[0].fill(0.25);
    host.input[1].fill(-0.125);
    let gain = host.parameter_id("Gain");
    let smoothing = host.parameter_id("Smoothing");
    // NIH's native float domain is normalized. Gain spans -60..20 dB;
    // zero smoothing makes the expected waveform an independent exact step.
    let events = [change(smoothing, 0, 0.0), change(gain, 23, 0.625)];
    host.process_events(127, Some(&native(3.0, 19.5)), &events);
    for frame in 0..127 {
        let expected_gain = if frame < 23 {
            1.0
        } else {
            10.0f64.powf(-10.0 / 20.0)
        };
        for (channel, amplitude) in [(0, 0.25), (1, -0.125)] {
            let actual = f64::from(host.output[channel][frame]);
            assert!(
                (actual - amplitude * expected_gain).abs() < 1e-7,
                "frame={frame}, channel={channel}: {actual} vs {}",
                amplitude * expected_gain
            );
        }
    }
}

#[test]
fn one_native_gain_event_preserves_all_pre_event_samples() {
    for event_frame in [0, 1, 23, 126] {
        let mut host = NativeHost::<NativeGain>::new(1);
        host.input[0].fill(0.25);
        host.input[1].fill(-0.125);
        let gain = host.parameter_id("Gain");
        // No offset-zero sentinel: this is the host's complete one-event list.
        host.process_events(
            127,
            Some(&native(3.0, 19.5)),
            &[change(gain, event_frame, 0.625)],
        );
        let target = 10.0f64.powf(-10.0 / 20.0);
        let coefficient = (-1.0f64 / 480.0).exp();
        for frame in 0..127 {
            let envelope = if frame < event_frame {
                1.0
            } else {
                target + coefficient.powi((frame - event_frame + 1) as i32) * (1.0 - target)
            };
            for (channel, amplitude) in [(0, 0.25), (1, -0.125)] {
                assert!(
                    (f64::from(host.output[channel][frame as usize]) - amplitude * envelope).abs()
                        < 2e-6,
                    "one event at {event_frame}: frame={frame}, channel={channel}, actual={}",
                    host.output[channel][frame as usize]
                );
            }
        }
    }
}

fn render_gain(partitions: &[usize], smoothing_ms: f64) -> Vec<[f32; 2]> {
    let mut host = NativeHost::<NativeGain>::new(1);
    host.input[0].fill(0.25);
    host.input[1].fill(-0.125);
    let gain = host.parameter_id("Gain");
    let smoothing = host.parameter_id("Smoothing");
    let changes = [(0, -3.0), (23, -10.0), (89, -4.0), (89, -20.0), (256, -6.0)];
    let mut output = Vec::new();
    let mut position = 0;
    for &frames in partitions {
        let mut events = Vec::new();
        if position == 0 {
            events.push(change(smoothing, 0, smoothing_ms / 100.0));
        }
        for &(sample, db) in &changes {
            if sample >= position && sample < position + frames {
                events.push(change(gain, (sample - position) as u32, (db + 60.0) / 80.0));
            }
        }
        host.process_events(
            frames,
            Some(&native(
                3.0 + position as f64 / 48_000.0,
                19.5 + position as f64 / 32_000.0,
            )),
            &events,
        );
        output.extend((0..frames).map(|frame| [host.output[0][frame], host.output[1][frame]]));
        position += frames;
    }
    output
}

#[test]
fn native_gain_timing_and_smoothing_match_an_independent_sample_envelope() {
    for smoothing_ms in [0.0, 10.0] {
        let single = render_gain(&[257], smoothing_ms);
        let split = render_gain(&[1, 22, 1, 65, 3, 17, 147, 1], smoothing_ms);
        assert_eq!(single.len(), split.len());
        let coefficient = if smoothing_ms == 0.0 {
            0.0
        } else {
            (-1.0f64 / (smoothing_ms * 0.001 * 48_000.0)).exp()
        };
        let mut envelope = 1.0;
        for frame in 0..257 {
            let db = match frame {
                0..23 => -3.0,
                23..89 => -10.0,
                89..256 => -20.0,
                _ => -6.0,
            };
            let target = 10.0f64.powf(db / 20.0);
            envelope = target + coefficient * (envelope - target);
            for (channel, amplitude) in [(0, 0.25), (1, -0.125)] {
                assert!(
                    (f64::from(single[frame][channel]) - amplitude * envelope).abs() < 2e-6,
                    "smoothing={smoothing_ms}, frame={frame}, channel={channel}"
                );
                assert!(
                    (single[frame][channel] - split[frame][channel]).abs() < 1e-7,
                    "partition changed frame={frame}, channel={channel}"
                );
            }
        }
    }
}

#[test]
fn seconds_only_native_split_positions_follow_actual_samples_without_tempo() {
    let mut host = Host::new(1);
    let gain = host.parameter_id("Gain");
    let mut clock = native(3.0, 0.0);
    clock.flags = CLAP_TRANSPORT_HAS_SECONDS_TIMELINE | CLAP_TRANSPORT_IS_PLAYING;
    host.process_events(63, Some(&clock), &[change(gain, 17, 0.5)]);
    let mut next_clock = clock;
    next_clock.song_pos_seconds += (63.0 / 48_000.0 * CLAP_SECTIME_FACTOR as f64).round() as i64;
    host.process_events(31, Some(&next_clock), &[change(gain, 30, 0.625)]);
    let log = host.log.lock().unwrap();
    assert_eq!(log.len(), 4);
    assert_eq!((log[0].num_frames, log[1].num_frames), (17, 46));
    assert_eq!(log[0].transport.sample_position, 144_000);
    assert_eq!(log[1].transport.sample_position, 144_017);
    assert_eq!(log[2].transport.sample_position, 144_063);
    assert_eq!(log[3].transport.sample_position, 144_093);
    assert_eq!((log[2].num_frames, log[3].num_frames), (30, 1));
}

fn check_streaming_automation<P: nih::ClapPlugin>(name: &str, key: &str, values: [f64; 3]) {
    let specs = crate::wrapper::get_param_specs(name);
    let spec = specs.iter().find(|spec| spec.engine_key == key).unwrap();
    let bridge = plugins_bridge::param_bridge::ParamBridge::new(specs);
    let infos: Vec<_> = (0..bridge.count())
        .map(|index| bridge.info(index).unwrap())
        .collect();
    let params = crate::params::DynamicParams::from_infos(&infos);
    let create = || {
        let mut plugin =
            crate::params::configuration::create_plugin(name, 48_000, &params).unwrap();
        assert!(
            matches!(plugin.preferred_oversampling(), None | Some(1)),
            "{name} needs a buffered automation design"
        );
        plugin.initialize(48_000.0).unwrap();
        params.sync_to_plugin(plugin.as_mut()).unwrap();
        plugin
    };
    let mut reference = create();
    let mut unchanged = create();
    let id = ParameterId::from(key);
    let mut host = NativeHost::<P>::new(1);
    let param = host.parameter_id(spec.name);
    let automation = [(233, values[0]), (2719, values[1]), (7217, values[2])];
    let mut position = 0;
    let mut post_event_difference = 0.0f32;
    for frames in [1024, 1, 17, 255, 511, 240].into_iter().cycle().take(30) {
        for frame in 0..frames {
            let index = position + frame;
            host.input[0][frame] = 0.2 + (index as f32 * 0.019).sin() * 0.03;
            host.input[1][frame] = -0.15 + (index as f32 * 0.053).sin() * 0.04;
        }
        let events: Vec<_> = automation
            .iter()
            .filter(|(sample, _)| *sample >= position && *sample < position + frames)
            .map(|&(sample, value)| {
                change(
                    param,
                    (sample - position) as u32,
                    (value - spec.min_f64()) / (spec.max_f64() - spec.min_f64()),
                )
            })
            .collect();
        host.process_events(
            frames,
            Some(&native(
                3.0 + position as f64 / 48_000.0,
                19.5 + position as f64 / 32_000.0,
            )),
            &events,
        );
        for frame in 0..frames {
            let index = position + frame;
            if let Some(&(_, raw)) = automation.iter().find(|(sample, _)| *sample == index) {
                reference
                    .set_parameter(id.clone(), ParameterValue::Float(raw as f32))
                    .unwrap();
            }
            let input = [host.input[0][frame], host.input[1][frame]];
            let mut expected = [0.0; 2];
            let mut baseline = [0.0; 2];
            let context =
                ProcessContext::new(48_000, 1).with_sample_position(144_000 + index as u64);
            assert_eq!(
                reference.process(&input, &mut expected, &context).unwrap(),
                1
            );
            assert_eq!(
                unchanged.process(&input, &mut baseline, &context).unwrap(),
                1
            );
            for channel in 0..2 {
                let actual = host.output[channel][frame];
                assert!(
                    (actual - expected[channel]).abs() < 1e-5,
                    "{name}.{key}, frame={index}, ch={channel}: {actual} vs {}",
                    expected[channel]
                );
                if index < 233 {
                    assert_eq!(
                        expected[channel], baseline[channel],
                        "{name}: reference changed before event"
                    );
                    assert!(
                        (actual - baseline[channel]).abs() < 1e-6,
                        "{name}: native change arrived early"
                    );
                } else {
                    post_event_difference =
                        post_event_difference.max((expected[channel] - baseline[channel]).abs());
                }
            }
        }
        position += frames;
    }
    assert!(
        post_event_difference > 1e-4,
        "{name}.{key}: fixture did not exercise audible change"
    );
}

crate::sotf_nih_plugin!(NativeGate, plugin_type: "Gate", name: "Gate Automation Probe", clap_id: "org.sotf.gate-automation-probe", vst3_class_id: *b"SotfGateAutom001", channels: 2);

#[test]
fn native_gate_events_match_independent_sample_split_processing() {
    check_streaming_automation::<NativeGate>("Gate", "threshold", [-6.0, -12.0, -50.0]);
}
