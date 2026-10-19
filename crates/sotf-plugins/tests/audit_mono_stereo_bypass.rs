//! Host bypass preserves channel geometry and the populated widening state.

// Rust guideline compliant 2026-02-21
use sotf_plugins::{DawHost, create_plugin};

fn host() -> DawHost {
    let mut host = DawHost::new(1, 48_000);
    host.add_plugin(
        create_plugin(
            "mono_to_stereo",
            &serde_json::json!({
                "stereo_width":1.0,"haas_delay_ms":5.0,"freq_dependent":true
            }),
            1,
            48_000,
        )
        .unwrap(),
    )
    .unwrap();
    host.build().unwrap();
    host
}

#[test]
fn width_changing_host_bypass_is_rejected_without_disturbing_audio() {
    let mut tested = host();
    let mut reference = host();
    let input: Vec<_> = (0..513).map(|i| (i as f32 * 0.031).sin() * 0.1).collect();
    let mut output = vec![0.0; input.len() * 2];
    let mut expected = output.clone();
    tested.process(&input, &mut output).unwrap();
    reference.process(&input, &mut expected).unwrap();
    assert!(tested.bypass_plugin(0).unwrap_err().contains("channels"));
    assert!(!tested.is_plugin_bypassed(0).unwrap());
    tested.process(&input, &mut output).unwrap();
    reference.process(&input, &mut expected).unwrap();
    assert_eq!(
        output, expected,
        "rejected bypass must preserve populated histories"
    );
}
