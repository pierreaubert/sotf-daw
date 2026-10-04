//! Structural Gate choices survive the bridge lifecycle and preset roundtrips.

use plugins_bridge::{
    create_plugin,
    state::{load_state, save_state},
};
use sotf_host::{ParameterId, ParameterValue, ProcessContext};

#[test]
fn gate_choice_labels_indices_and_saved_state_preserve_audio_sign_and_cap() {
    for (mode, label, signed_cap) in [
        (0, "Downward", 0.0_f64),
        (1, "Upward", 6.0),
        (2, "Duck", -6.0),
    ] {
        for choice in [
            serde_json::json!(mode),
            serde_json::json!(label.to_lowercase()),
        ] {
            let config = serde_json::json!({
                "mode": choice, "max_boost_db": 6.0, "range_db": 6.0,
                "threshold_db": -30.0, "ratio": 3.0, "attack_ms": 0.1,
                "hold_ms": 0.0, "release_ms": 10.0,
            })
            .to_string();
            let mut plugin = create_plugin("Gate", 2, 48_000, &config).unwrap();
            plugin.initialize(48_000.0).unwrap();
            let mode_id = ParameterId::from("mode");
            assert_eq!(
                plugin.get_parameter(&mode_id),
                Some(ParameterValue::Int(mode))
            );
            let saved = save_state(plugin.as_ref());
            let mut restored = create_plugin("Gate", 2, 48_000, &config).unwrap();
            load_state(restored.as_mut(), &saved).unwrap();
            restored.initialize(48_000.0).unwrap();
            assert_eq!(save_state(restored.as_ref()), saved);
            let mut last = Vec::new();
            for frames in [1, 17, 257, 63].into_iter().cycle().take(128) {
                let input: Vec<_> = (0..frames).flat_map(|_| [0.1, -0.1]).collect();
                let mut output = vec![f32::NAN; input.len()];
                let mut reference = vec![f32::NAN; input.len()];
                let context = ProcessContext::new(48_000, frames);
                assert_eq!(
                    plugin.process(&input, &mut output, &context).unwrap(),
                    frames
                );
                assert_eq!(
                    restored.process(&input, &mut reference, &context).unwrap(),
                    frames
                );
                assert_eq!(output, reference, "{label} preset changed the waveform");
                last = output;
            }
            // The scalar fast-power DSP contract permits 0.02 dB error.
            let expected = 0.1 * 10.0_f64.powf(signed_cap / 20.0);
            for pair in last.as_chunks::<2>().0 {
                assert!(
                    (20.0 * (f64::from(pair[0]) / expected).log10()).abs() < 0.02,
                    "{label}: actual={pair:?}, expected={expected}"
                );
                assert!(
                    (20.0 * (-f64::from(pair[1]) / expected).log10()).abs() < 0.02,
                    "{label}: actual={pair:?}, expected={expected}"
                );
            }
            let changed = serde_json::to_vec(&serde_json::json!({"mode": (mode + 1) % 3})).unwrap();
            assert!(
                load_state(restored.as_mut(), &changed).is_err(),
                "active mode change must require rebuild"
            );
            assert_eq!(save_state(restored.as_ref()), saved);
        }
    }
}

#[test]
fn gate_external_key_drives_program_channels_through_bridge() {
    for (mode, sign) in [("Upward", 1.0), ("Duck", -1.0)] {
        for linked in [false, true] {
            // Gate's constructor channel argument is the program width.
            let config = serde_json::json!({"mode":mode,"sidechain_external":true,
                "link_channels":linked,"threshold_db":-30.0,"ratio":3.0,
                "attack_ms":0.1,"release_ms":10.0,"hold_ms":0.0,
                "max_boost_db":6.0,"range_db":6.0})
            .to_string();
            let mut plugin = create_plugin("Gate", 2, 48_000, &config).unwrap();
            assert_eq!((plugin.input_channels(), plugin.output_channels()), (4, 2));
            plugin.initialize(48_000.0).unwrap();
            let mut output = [0.0; 126];
            for _ in 0..200 {
                let input: Vec<_> = (0..63).flat_map(|_| [0.004, -0.002, 0.1, 0.0]).collect();
                assert_eq!(
                    plugin
                        .process(&input, &mut output, &ProcessContext::new(48_000, 63))
                        .unwrap(),
                    63
                );
            }
            let gains = [
                10.0_f64.powf(sign * 6.0 / 20.0),
                if linked {
                    10.0_f64.powf(sign * 6.0 / 20.0)
                } else {
                    1.0
                },
            ];
            for pair in output.as_chunks::<2>().0 {
                for (channel, input) in [0.004, -0.002].into_iter().enumerate() {
                    let error_db =
                        20.0 * (f64::from(pair[channel]) / (input * gains[channel])).log10();
                    assert!(
                        error_db.abs() < 0.02,
                        "{mode} linked={linked} channel={channel}: {error_db} dB"
                    );
                }
            }
        }
    }
}
