//! Audibly observe a mute before and after real downstream delay buffering.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterValue, host::DawHost};
use sotf_plugins::create_plugin;

#[test]
fn muting_new_input_preserves_pre_mute_audio_already_in_a_downstream_delay() {
    let rate = 48_000;
    let mut host = DawHost::new(1, rate);
    host.add_plugin(
        create_plugin(
            "channel_mute_solo",
            &serde_json::json!({
                "enabled":true,"fade_ms":0.0
            }),
            1,
            rate,
        )
        .unwrap(),
    )
    .unwrap();
    host.add_plugin(
        create_plugin(
            "delay",
            &serde_json::json!({
                "delay_ms":5.0,"feedback":0.0,"mix":1.0,"lfo_depth_ms":0.0
            }),
            1,
            rate,
        )
        .unwrap(),
    )
    .unwrap();
    host.build().unwrap();
    let input = vec![0.25; 512];
    let mut output = vec![0.0; 512];
    host.process(&input, &mut output).unwrap();
    assert!(output[..240].iter().all(|&x| x == 0.0));
    assert!(output[240..].iter().all(|&x| x == 0.25));
    host.set_plugin_parameter(0, "mute_0", ParameterValue::Bool(true))
        .unwrap();
    host.process(&input, &mut output).unwrap();
    assert!(
        output[..240].iter().all(|&x| x == 0.25),
        "queued pre-mute audio must keep its source gain"
    );
    assert!(
        output[240..].iter().all(|&x| x == 0.0),
        "newly muted programme must become silent after the delay"
    );
    host.process(&input, &mut output).unwrap();
    assert!(output.iter().all(|&x| x == 0.0));
}
