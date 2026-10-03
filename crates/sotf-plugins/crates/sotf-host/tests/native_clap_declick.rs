//! Loaded-CLAP Declick consumer test (Declick-specific native-host leaf).
//!
//! Mirrors `native_clap_gain_processes_and_round_trips_state` for the
//! Declick bundle: load by plugin id, pin the 9-control schema, drive a
//! live sensitivity edit, process a settled sine plus an interior click,
//! and prove state save/restore determinism. Latency (8 = lookahead +
//! default zero width) is derived from the canonical `PARAMS` defaults,
//! never trusted from the instance; settled comparisons shift by the
//! REPORTED latency so a host-padding surprise fails loudly on the
//! constant, not silently in the audio.
//!
//! Run with the freshly built bundle (root builds it):
//! `SOTF_TEST_DECLICK_CLAP_PLUGIN=<target>/release/libplugins_nih.so cargo
//! test -p sotf-host --features external-plugin-clap --test
//! native_clap_declick -- --ignored --nocapture`.

#![cfg(feature = "external-plugin-clap")]

use sotf_host::external_plugin::{
    ExternalHostingBackend, ExternalPlugin, PluginDescriptor, PluginFormat, PluginScanStatus,
};
use sotf_host::parameters::ParameterValue;
use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_host::serialization::SerializablePlugin;
use std::path::PathBuf;

/// Lookahead latency plus the default zero repair width (canonical PARAMS).
const EXPECTED_LATENCY: usize = 8;
/// Mono-equivalent settle prefix: startup context plus margin.
const SETTLED_FROM: usize = 48;
/// Click amplitude injected mid-block (DSP-proven detectable at sens 5).
const CLICK_AMP: f32 = 3.0;

#[test]
#[ignore = "requires SOTF_TEST_DECLICK_CLAP_PLUGIN to point to the built plugins-nih declick CLAP library"]
fn native_clap_declick_processes_repairs_and_round_trips_state() {
    let descriptor = clap_declick_descriptor();
    let mut plugin = ExternalPlugin::new(&descriptor, 48_000).expect("load native CLAP declick");
    assert_eq!(plugin.hosting_backend(), ExternalHostingBackend::Clap);
    assert_eq!(plugin.descriptor().id, "org.spinorama.sotf.declick");
    assert_eq!(plugin.input_channels(), 2);
    assert_eq!(plugin.output_channels(), 2);
    // Captured early, asserted last: the value must reach the log even
    // if the host reports something unexpected (R44 report-first).
    let reported_latency = plugin.latency_samples();
    eprintln!("[loaded-clap] reported latency={reported_latency}");

    let parameters = plugin.parameters();
    // R44 report-first ordering: dump every exposed control plus the
    // canonical-9 presence map BEFORE any assertion, so the log names
    // exactly which structural controls the host cannot see. The 9-count
    // pin below (moved last) is the unweakened requirement.
    eprintln!("[loaded-clap] exposed params count={}", parameters.len());
    for parameter in &parameters {
        eprintln!(
            "[loaded-clap] param name={} min={:?} max={:?} default={:?}",
            parameter.name, parameter.min_value, parameter.max_value, parameter.default_value
        );
    }
    for key in [
        "enabled",
        "sensitivity",
        "link_channels",
        "mode",
        "bands",
        "crossover_hz",
        "frequency_skew",
        "repair_width",
        "audition_residual",
    ] {
        let wanted = key.replace('_', "");
        let present = parameters.iter().any(|parameter| {
            parameter
                .name
                .to_ascii_lowercase()
                .replace(' ', "")
                .contains(&wanted)
        });
        eprintln!("[loaded-clap] canonical {key} present={present}");
    }
    let exposed_count = parameters.len();
    let sensitivity = parameters
        .into_iter()
        .find(|parameter| parameter.name.to_ascii_lowercase().contains("sensitivity"))
        .expect("CLAP sensitivity parameter metadata");
    // NIH exposes normalized values to CLAP for continuous parameters
    // (gain precedent); the declick DSP range is 1..=100 linear.
    assert_eq!(sensitivity.min_value, Some(ParameterValue::Float(0.0)));
    assert_eq!(sensitivity.max_value, Some(ParameterValue::Float(1.0)));
    // H4-twin live regime (sensitivity is realtime per the pinned NIH
    // structural/live split, so no reactivation is needed).
    let sens_five = (5.0_f32 - 1.0) / 99.0;
    plugin
        .set_parameter(sensitivity.id.clone(), ParameterValue::Float(sens_five))
        .expect("queue CLAP sensitivity edit");
    assert_eq!(
        plugin.get_parameter(&sensitivity.id),
        Some(ParameterValue::Float(sens_five))
    );

    let frames = 512_usize;
    let channels = 2_usize;
    let mut clean = vec![0.0_f32; frames * channels];
    for (frame, slot) in clean.chunks_mut(channels).enumerate() {
        let sample = (frame as f32 * 440.0 / 48_000.0 * std::f32::consts::TAU).sin() * 0.25;
        slot.fill(sample);
    }
    let click_frame = 256_usize;
    let mut input = clean.clone();
    for sample in &mut input[click_frame * channels..(click_frame + 1) * channels] {
        *sample += CLICK_AMP;
    }
    let mut output = vec![f32::NAN; input.len()];
    let context = ProcessContext::new(48_000, frames);
    assert_eq!(
        plugin.process(&input, &mut output, &context).unwrap(),
        frames
    );
    assert!(output.iter().all(|sample| sample.is_finite()));
    // Leading latency is exact silence (NIH leading-silence precedent).
    for (index, sample) in output.iter().take(EXPECTED_LATENCY * channels).enumerate() {
        assert_eq!(*sample, 0.0, "leading silence sample={index}");
    }
    let latency = plugin.latency_samples();
    let aligned = output
        .chunks_exact(channels)
        .skip(latency)
        .zip(clean.chunks_exact(channels))
        .enumerate();
    for (frame, (actual_frame, expected_frame)) in aligned {
        if frame < SETTLED_FROM {
            continue;
        }
        for (ch, (actual, expected)) in actual_frame.iter().zip(expected_frame.iter()).enumerate() {
            if frame == click_frame {
                let error = (actual - expected).abs();
                assert!(error < 0.15, "click must repair ch={ch} error={error}");
            } else if frame.abs_diff(click_frame) > 1 {
                let damage = (actual - expected).abs();
                assert!(
                    damage < 1.0e-4,
                    "settled dry frame={frame} ch={ch} damage={damage}"
                );
            }
        }
    }

    let preset = plugin.serialize().expect("save CLAP state");
    let state = preset
        .external_plugin_state()
        .unwrap()
        .expect("external state envelope");
    assert!(!state.opaque_state.is_empty());
    let mut restored =
        ExternalPlugin::from_placeholder_state(&state, 48_000).expect("restore CLAP state");
    let mut restored_output = vec![0.0; input.len()];
    restored
        .process(&input, &mut restored_output, &context)
        .unwrap();
    for (index, (actual, expected)) in restored_output.iter().zip(output.iter()).enumerate() {
        assert!(
            (actual - expected).abs() < 1.0e-6,
            "restored determinism sample={index}"
        );
    }
    // Requirement pins, asserted last (R44 report-first ordering):
    // latency 8 = lookahead + default zero width (PARAMS-derived), and
    // the unweakened 9-control requirement — all nine canonical
    // controls host-accessible, the four structural ones via
    // restart/rebuild semantics (NIH handoff proposal in
    // consumers-r44-result.md), never hidden.
    assert_eq!(reported_latency, EXPECTED_LATENCY);
    assert_eq!(exposed_count, 9, "declick carries nine controls");
}

fn clap_declick_descriptor() -> PluginDescriptor {
    let path = PathBuf::from(
        std::env::var_os("SOTF_TEST_DECLICK_CLAP_PLUGIN")
            .expect("SOTF_TEST_DECLICK_CLAP_PLUGIN must point to a .clap file or bundle"),
    );
    PluginDescriptor {
        id: "org.spinorama.sotf.declick".into(),
        name: "SOTF: Declick".into(),
        vendor: "SOTF".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        format: PluginFormat::Clap,
        path,
        audio_inputs: 2,
        audio_outputs: 2,
        is_instrument: false,
        categories: vec!["audio-effect".into()],
        scan_status: PluginScanStatus::Loadable,
    }
}
