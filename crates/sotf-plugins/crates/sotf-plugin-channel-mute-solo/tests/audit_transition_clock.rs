//! Public mute/solo priority, layout, transport, and sample clock checks.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_channel_mute_solo::{ChannelMuteSoloParams, ChannelMuteSoloPlugin, ChannelState};

fn configured(channels: usize, muted: bool) -> ChannelMuteSoloPlugin {
    ChannelMuteSoloPlugin::try_from_params(
        channels,
        ChannelMuteSoloParams {
            enabled: true,
            channel_states: vec![
                ChannelState {
                    muted,
                    soloed: false,
                    dimmed: false
                };
                channels
            ],
            dim_gain_db: -20.0,
            fade_ms: 5.0,
        },
    )
    .unwrap()
}

fn automated(rate: u32, channels: usize, pattern: &[usize]) -> Vec<f32> {
    let mut plugin = configured(channels, false);
    plugin.initialize(f64::from(rate)).unwrap();
    let frames = 30_000;
    let mut output = vec![1.0; frames * channels];
    let events = [0, 511, 1023, 2049, 4097, 17001, 22003];
    let mut event = 0;
    let mut position = 0;
    let mut block = 0;
    while position < frames {
        if event < events.len() && position == events[event] {
            let mut states = vec![ChannelState::default(); channels];
            match event {
                0 | 5 => {
                    for state in &mut states {
                        state.muted = true;
                    }
                }
                1 => {
                    states[1 % channels] = ChannelState {
                        muted: true,
                        soloed: true,
                        dimmed: true,
                    }
                }
                2 => {
                    states[1 % channels].soloed = true;
                    states[2 % channels].soloed = true;
                }
                3 => {
                    states[0].dimmed = true;
                    states[1 % channels].muted = true;
                }
                _ => {}
            }
            plugin.set_channel_states(&states).unwrap();
            event += 1;
        }
        let boundary = events.get(event).copied().unwrap_or(frames);
        let count = pattern[block % pattern.len()].min(boundary - position);
        plugin
            .process_in_place(
                &mut output[position * channels..(position + count) * channels],
                &ProcessContext::new(rate, count),
            )
            .unwrap();
        position += count;
        block += 1;
    }
    output
}

#[test]
fn rapid_mute_solo_dim_and_reentry_have_an_absolute_sample_clock() {
    for rate in [44_100, 48_000, 96_000] {
        for channels in [1, 7, 128] {
            let reference = automated(rate, channels, &[1]);
            for pattern in [&[8192][..], &[7, 31, 257][..]] {
                let actual = automated(rate, channels, pattern);
                let error = actual
                    .iter()
                    .zip(&reference)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0_f32, f32::max);
                assert_eq!(
                    error, 0.0,
                    "rate={rate}, channels={channels}, pattern={pattern:?}"
                );
            }
        }
    }
}

#[test]
fn transport_reset_preserves_an_inflight_transition_sample_for_sample() {
    let mut reset = configured(7, false);
    let mut continuous = configured(7, false);
    for plugin in [&mut reset, &mut continuous] {
        plugin.initialize(48_000.0).unwrap();
        plugin.set_channel_state(3, true, false, false).unwrap();
        plugin
            .process_in_place(&mut vec![1.0; 137 * 7], &ProcessContext::new(48_000, 137))
            .unwrap();
    }
    reset.reset();
    let mut a = vec![1.0; 1024 * 7];
    let mut b = a.clone();
    reset
        .process_in_place(&mut a, &ProcessContext::new(48_000, 1024))
        .unwrap();
    continuous
        .process_in_place(&mut b, &ProcessContext::new(48_000, 1024))
        .unwrap();
    assert!(
        a == b,
        "stop/resume must continue the fade rather than snap to the target"
    );
    assert!(a[3] > 0.0 && a[3] < 1.0);
}

#[test]
fn initially_muted_layouts_are_silent_from_the_first_audible_sample() {
    for channels in [1, 7, 128] {
        let mut plugin = configured(channels, true);
        plugin.initialize(96_000.0).unwrap();
        let mut buffer = vec![1.0; 8193 * channels];
        plugin
            .process_in_place(&mut buffer, &ProcessContext::new(96_000, 8193))
            .unwrap();
        assert!(buffer.iter().all(|&x| x == 0.0));
        assert!(
            plugin
                .set_channel_state(channels, false, false, false)
                .is_err()
        );
        assert!(plugin.set_channel_states(&[]).is_err());
    }
    let params = ChannelMuteSoloParams {
        enabled: true,
        channel_states: vec![],
        dim_gain_db: -20.0,
        fade_ms: 5.0,
    };
    assert!(ChannelMuteSoloPlugin::try_from_params(0, params).is_err());
}

#[test]
fn solo_priority_is_independent_of_mute_dim_and_channel_order() {
    for solo in 0..7 {
        let mut states = vec![ChannelState::default(); 7];
        states[solo] = ChannelState {
            muted: true,
            soloed: true,
            dimmed: true,
        };
        let mut plugin = ChannelMuteSoloPlugin::try_from_params(
            7,
            ChannelMuteSoloParams {
                enabled: true,
                channel_states: states,
                dim_gain_db: -20.0,
                fade_ms: 0.0,
            },
        )
        .unwrap();
        plugin.initialize(48_000.0).unwrap();
        let input: Vec<f32> = (1..=7).map(|x| x as f32 * 0.125).collect();
        let mut output = input.clone();
        plugin
            .process_in_place(&mut output, &ProcessContext::new(48_000, 1))
            .unwrap();
        for channel in 0..7 {
            assert_eq!(
                output[channel],
                if channel == solo { input[channel] } else { 0.0 }
            );
        }
    }
}

#[test]
fn malformed_bulk_controls_preserve_all_prior_targets() {
    use sotf_host::{ParameterId, ParameterValue, parametric_plugin::ParameterSet};
    for bad in [
        ParameterValue::Float(f32::NAN),
        ParameterValue::String("bad".into()),
    ] {
        let mut plugin = configured(7, false);
        let before = plugin.current_values();
        let mut values = ParameterSet::new();
        values.insert(ParameterId::from("enabled"), ParameterValue::Bool(false));
        values.insert(ParameterId::from("fade_ms"), bad);
        assert!(plugin.apply_values(values).is_err());
        assert_eq!(plugin.current_values(), before);
    }
    let mut plugin = configured(7, false);
    let before = plugin.current_values();
    let mut values = ParameterSet::new();
    values.insert(ParameterId::from("enabled"), ParameterValue::Bool(false));
    values.insert(
        ParameterId::from("channel_states"),
        ParameterValue::String("[]".into()),
    );
    assert!(plugin.apply_values(values).is_err());
    assert_eq!(plugin.current_values(), before);
}

#[test]
fn wrong_rate_is_rejected_before_direct_or_compiled_output_mutation() {
    use sotf_host::plugin::PluginCompiledOp;
    let mut plugin = configured(7, false);
    plugin.initialize(48_000.0).unwrap();
    let mut output = vec![0.25; 7 * 31];
    let before = output.clone();
    let context = ProcessContext::new(96_000, 31);
    assert!(plugin.process_in_place(&mut output, &context).is_err());
    assert_eq!(output, before);
    assert!(
        plugin
            .process_compiled_f32(
                PluginCompiledOp::ChannelMuteSolo,
                &vec![1.0; 7 * 31],
                &mut output,
                &context
            )
            .unwrap()
            .is_err()
    );
    assert_eq!(output, before);
}
