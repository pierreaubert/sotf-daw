//! Manual pre-feature audio/state capture for AUD139.
//!
//! This test intentionally uses the public Dynamic EQ constructor and process
//! API. It is ignored during normal test runs because it writes durable binary
//! artifacts when `SOTF_AUDIT_BASELINE_DIR` is set.

use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_plugin_dynamic_eq::{DynEqBandParams, DynamicEqPlugin, DynamicEqPluginParams};
use std::f32::consts::TAU;
use std::fs;
use std::path::PathBuf;

const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: usize = 2;
const FRAMES: usize = 8_192;
const PARTITIONS: [usize; 9] = [1, 17, 251, 3, 509, 64, 1021, 127, 5];

fn input_signal() -> Vec<f32> {
    let mut samples = Vec::with_capacity(FRAMES * CHANNELS);
    for frame in 0..FRAMES {
        let phase = TAU * frame as f32 / SAMPLE_RATE as f32;
        let envelope = match frame {
            0..=1_999 => 0.90,
            2_000..=3_999 => 0.20,
            4_000..=6_399 => 0.72,
            _ => 0.35,
        };
        let left = envelope
            * (0.15 * (phase * 220.0).sin()
                + 0.12 * (phase * 980.0).sin()
                + 0.045 * (phase * 4_600.0).sin());
        let right = envelope
            * (0.055 * (phase * 220.0 + 0.2).sin()
                + 0.19 * (phase * 980.0 + 0.6).sin()
                + 0.075 * (phase * 4_600.0 + 1.1).sin());
        samples.extend([left, right]);
    }
    samples
}

fn params_for_case(name: &'static str) -> DynamicEqPluginParams {
    let mut params = DynamicEqPluginParams {
        num_bands: 2,
        threshold: -24.0,
        ratio: 4.0,
        attack_ms: 2.0,
        release_ms: 55.0,
        knee: 3.0,
        link_channels: !name.starts_with("unlinked"),
        mix: 1.0,
        bands: vec![DynEqBandParams::default(); 2],
        stereo_pairs: None,
    };
    for band in &mut params.bands {
        band.active = false;
        band.gain = 0.0;
        band.solo = false;
    }

    match name {
        "linked_boost" | "linked_cut" | "unlinked_boost" | "unlinked_cut" => {
            params.bands[0].frequency = 980.0;
            params.bands[0].q = 0.9;
            params.bands[0].gain = if name.ends_with("boost") { 12.0 } else { -12.0 };
            params.bands[0].active = true;
        }
        "inactive_peak" => {
            params.bands[0].frequency = 980.0;
            params.bands[0].gain = 12.0;
        }
        "solo_peak" => {
            params.bands[0].frequency = 700.0;
            params.bands[0].q = 1.2;
            params.bands[0].gain = 9.0;
            params.bands[0].active = true;
            params.bands[0].solo = true;
            params.bands[1].frequency = 4_600.0;
            params.bands[1].gain = -12.0;
            params.bands[1].active = true;
        }
        _ => panic!("unknown AUD139 baseline case: {name}"),
    }
    params
}

fn process_irregularly(params: DynamicEqPluginParams, input: &[f32]) -> Vec<f32> {
    let mut plugin = DynamicEqPlugin::try_from_params_at_sample_rate(CHANNELS, params, SAMPLE_RATE)
        .expect("valid pre-edit peak parameters");
    let mut output = input.to_vec();
    let mut frame_offset = 0;
    let mut partition_index = 0;
    while frame_offset < FRAMES {
        let frames = PARTITIONS[partition_index % PARTITIONS.len()].min(FRAMES - frame_offset);
        let sample_offset = frame_offset * CHANNELS;
        let sample_end = sample_offset + frames * CHANNELS;
        let context = ProcessContext::new(SAMPLE_RATE, frames);
        plugin
            .process_in_place(&mut output[sample_offset..sample_end], &context)
            .expect("valid bounded callback partition");
        frame_offset += frames;
        partition_index += 1;
    }
    output
}

fn write_f32le(path: PathBuf, samples: &[f32]) {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(samples));
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    fs::write(path, bytes).expect("write AUD139 baseline samples");
}

fn rms_difference(left: &[f32], right: &[f32]) -> f64 {
    assert_eq!(left.len(), right.len());
    let squared_error = left
        .iter()
        .zip(right)
        .map(|(left, right)| f64::from(left - right).powi(2))
        .sum::<f64>();
    (squared_error / left.len().max(1) as f64).sqrt()
}

#[test]
#[ignore = "manual pre-edit AUD139 Dynamic EQ audio/state artifact capture"]
fn capture_aud139_pre_edit_peak_audio_and_serialized_parameter_baseline() {
    let output_dir = PathBuf::from(
        std::env::var_os("SOTF_AUDIT_BASELINE_DIR")
            .expect("set SOTF_AUDIT_BASELINE_DIR to the durable AUD139 baseline directory"),
    );
    fs::create_dir_all(&output_dir).expect("create AUD139 baseline directory");

    let input = input_signal();
    assert!(input.iter().all(|sample| sample.is_finite()));
    assert!(
        input
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0_f32, f32::max)
            < 0.5
    );
    write_f32le(output_dir.join("stereo-input.f32le"), &input);

    let names = [
        "linked_boost",
        "linked_cut",
        "unlinked_boost",
        "unlinked_cut",
        "inactive_peak",
        "solo_peak",
    ];
    let mut manifest_cases = Vec::with_capacity(names.len());
    let mut outputs = Vec::with_capacity(names.len());
    for name in names {
        let params = params_for_case(name);
        let params_json = serde_json::to_vec_pretty(&params).expect("serialize legacy peak params");
        let restored: DynamicEqPluginParams =
            serde_json::from_slice(&params_json).expect("restore legacy peak params");
        let output = process_irregularly(restored, &input);
        assert!(output.iter().all(|sample| sample.is_finite()), "{name}");
        assert!(
            output
                .iter()
                .map(|sample| sample.abs())
                .fold(0.0_f32, f32::max)
                > 0.05,
            "{name} must produce meaningful audio"
        );
        if name == "inactive_peak" {
            assert!(
                output
                    .iter()
                    .zip(&input)
                    .all(|(actual, source)| (actual - source).abs() <= 1.0e-7),
                "inactive legacy peak should remain transparent"
            );
        }

        write_f32le(output_dir.join(format!("{name}.f32le")), &output);
        fs::write(output_dir.join(format!("{name}.params.json")), &params_json)
            .expect("write serialized legacy peak parameters");
        manifest_cases.push(serde_json::json!({
            "name": name,
            "params_file": format!("{name}.params.json"),
            "output_file": format!("{name}.f32le"),
            "linked_detection": !name.starts_with("unlinked"),
            "sample_rate_hz": SAMPLE_RATE,
            "channels": CHANNELS,
            "frames": FRAMES,
            "partition_frames": PARTITIONS,
            "parameter_roundtrip": "DynamicEqPluginParams serde serialize -> deserialize before processing",
        }));
        outputs.push((name, output));
    }

    let output_for = |name: &str| {
        outputs
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, output)| output.as_slice())
            .expect("named AUD139 capture case exists")
    };
    let boost_cut_rms = rms_difference(output_for("linked_boost"), output_for("linked_cut"));
    let link_mode_rms = rms_difference(output_for("linked_boost"), output_for("unlinked_boost"));
    assert!(
        boost_cut_rms > 1.0e-3,
        "boost and cut controls must be distinct"
    );
    assert!(
        link_mode_rms > 1.0e-4,
        "linked and unlinked detection must be distinct"
    );

    let manifest = serde_json::json!({
        "feature": "AUD139 Dynamic EQ shelf baseline",
        "implementation_state": "pre-edit Peak-only DynamicEqPlugin",
        "sample_rate_hz": SAMPLE_RATE,
        "channels": CHANNELS,
        "frames": FRAMES,
        "input_file": "stereo-input.f32le",
        "cases": manifest_cases,
        "input_signal": "stereo deterministic multi-tone with four level regions; L/R amplitudes and phases differ",
        "behavior_checks": {
            "linked_boost_vs_linked_cut_rms": boost_cut_rms,
            "linked_vs_unlinked_boost_rms": link_mode_rms,
            "inactive_peak_is_transparent": true
        },
        "capture_scope": "plugin crate public constructor/process and serialized parameter roundtrip; C ABI full state/preset capture is separate",
    });
    fs::write(
        output_dir.join("capture-manifest.json"),
        serde_json::to_vec_pretty(&manifest).expect("serialize capture manifest"),
    )
    .expect("write capture manifest");
}
