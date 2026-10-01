//! Public-API flow checks for dynamic band updates (R2/A2).
//!
//! Sample-exact blend proofs live in the unit suite; these tests drive the
//! snapshot/prepare/commit/reclaim flow and end-to-end behavior through the
//! public API only.

use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_linear_phase_eq::{
    BandConfig, CommitRefusal, LinearPhaseEqPlugin, LinearPhaseEqPluginParams,
};

const RATE: u32 = 48_000;
const CHANNELS: usize = 2;

fn band(filter_type: &str, frequency: f64, gain_db: f64) -> BandConfig {
    BandConfig {
        filter_type: filter_type.to_string(),
        frequency,
        q: 1.0,
        gain_db,
        active: true,
        placement: None,
    }
}

fn params_for(bands: Vec<BandConfig>) -> LinearPhaseEqPluginParams {
    let num_filters = bands.len();
    LinearPhaseEqPluginParams {
        num_filters,
        fir_length_index: 0,
        phase_mode_index: 0,
        auto_gain: false,
        mix: 1.0,
        filters: bands,
        stereo_pairs: None,
    }
}

fn sine(frames: usize, freq: f32) -> Vec<f32> {
    let mut buffer = vec![0.0; frames * CHANNELS];
    for frame in 0..frames {
        let t = frame as f32 / f64::from(RATE) as f32;
        let sample = (2.0 * std::f32::consts::PI * freq * t).sin() * 0.5;
        for ch in 0..CHANNELS {
            buffer[frame * CHANNELS + ch] = sample;
        }
    }
    buffer
}

fn stream_all(plugin: &mut LinearPhaseEqPlugin, input: &[f32]) -> Vec<f32> {
    let frames = input.len() / CHANNELS;
    let mut output = input.to_vec();
    let mut position = 0;
    while position < frames {
        let count = 256.min(frames - position);
        plugin
            .process_in_place(
                &mut output[position * CHANNELS..(position + count) * CHANNELS],
                &ProcessContext::new(RATE, count),
            )
            .unwrap();
        position += count;
    }
    output
}

fn drain_all(plugin: &mut LinearPhaseEqPlugin) -> Vec<f32> {
    let mut result = Vec::new();
    for _ in 0..20000 {
        let mut output = vec![0.0; 257 * CHANNELS];
        let drained = plugin
            .drain(&mut output, &ProcessContext::new(RATE, 0))
            .unwrap();
        result.extend_from_slice(&output[..drained.frames * CHANNELS]);
        if drained.complete {
            return result;
        }
    }
    panic!("dynamic drain did not finish");
}

fn rms(output: &[f32], from: usize, to: usize) -> f64 {
    let mut sum = 0.0;
    let mut count = 0;
    for frame in from..to {
        for ch in 0..CHANNELS {
            let sample = f64::from(output[frame * CHANNELS + ch]);
            sum += sample * sample;
            count += 1;
        }
    }
    (sum / count as f64).sqrt()
}

#[test]
fn prepare_commit_reclaim_flow_updates_live_config() {
    let mut plugin = LinearPhaseEqPlugin::from_params(
        CHANNELS,
        RATE,
        params_for(vec![band("Peak", 1000.0, 0.0), band("Peak", 3000.0, 0.0)]),
    )
    .unwrap();
    let latency = plugin.latency_samples();
    let snapshot = plugin.snapshot_config();
    assert_eq!(snapshot.bands.len(), 2);

    let prepared =
        LinearPhaseEqPlugin::prepare_band_update(&snapshot, 0, band("Peak", 1000.0, 9.0)).unwrap();
    plugin.commit_prepared_update(prepared).unwrap();
    assert!(plugin.update_in_progress());
    assert_eq!(plugin.latency_samples(), latency);
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("band_0_gain")),
        Some(ParameterValue::Float(9.0))
    );

    // A second commit is refused until the blend completes and the retired
    // route is reclaimed.
    let retry = LinearPhaseEqPlugin::prepare_band_update(
        &plugin.snapshot_config(),
        1,
        band("Peak", 3000.0, 6.0),
    )
    .unwrap();
    assert!(plugin.commit_prepared_update(retry).is_err());
    stream_all(&mut plugin, &sine(1024, 440.0));
    assert!(!plugin.update_in_progress());
    assert!(plugin.take_retired_route().is_some());

    let retry = LinearPhaseEqPlugin::prepare_band_update(
        &plugin.snapshot_config(),
        1,
        band("Peak", 3000.0, 6.0),
    )
    .unwrap();
    plugin.commit_prepared_update(retry).unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("band_1_gain")),
        Some(ParameterValue::Float(6.0))
    );
}

#[test]
fn updated_stream_matches_fresh_reference_without_steps() {
    let input = sine(3000, 440.0);
    let mut plugin = LinearPhaseEqPlugin::from_params(
        CHANNELS,
        RATE,
        params_for(vec![band("Peak", 1000.0, 0.0), band("Peak", 3000.0, 0.0)]),
    )
    .unwrap();
    let mut fresh = LinearPhaseEqPlugin::from_params(
        CHANNELS,
        RATE,
        params_for(vec![band("Peak", 1000.0, 9.0), band("Peak", 3000.0, 0.0)]),
    )
    .unwrap();

    let mut output = stream_all(&mut plugin, &input[..800 * CHANNELS]);
    let snapshot = plugin.snapshot_config();
    plugin
        .commit_prepared_update(
            LinearPhaseEqPlugin::prepare_band_update(&snapshot, 0, band("Peak", 1000.0, 9.0))
                .unwrap(),
        )
        .unwrap();
    output.extend(stream_all(&mut plugin, &input[800 * CHANNELS..]));
    assert!(output.iter().all(|sample| sample.is_finite()));

    // The commit frame blends with weight 0: the boundary step is the
    // signal's own natural step, far below any glitch scale.
    let boundary_step = (output[800 * CHANNELS] - output[800 * CHANNELS - CHANNELS]).abs();
    assert!(
        boundary_step < 0.06,
        "commit boundary must not step: {boundary_step}"
    );

    // Post-blend output matches a from-scratch reference within 5%.
    let reference = stream_all(&mut fresh, &input);
    let rms_actual = rms(&output, 2000, 3000);
    let rms_expected = rms(&reference, 2000, 3000);
    assert!(
        (rms_actual - rms_expected).abs() / rms_expected < 0.05,
        "post-blend rms {rms_actual} vs fresh {rms_expected}"
    );
}

#[test]
fn stale_and_drain_commits_are_refused_publicly() {
    let mut plugin = LinearPhaseEqPlugin::from_params(
        CHANNELS,
        RATE,
        params_for(vec![band("Peak", 1000.0, 0.0), band("Peak", 3000.0, 0.0)]),
    )
    .unwrap();
    let stale = plugin.snapshot_config();
    plugin
        .commit_prepared_update(
            LinearPhaseEqPlugin::prepare_band_update(&stale, 0, band("Peak", 1000.0, 6.0)).unwrap(),
        )
        .unwrap();
    stream_all(&mut plugin, &sine(1024, 440.0));
    assert!(plugin.take_retired_route().is_some());

    // The pre-update snapshot no longer matches the advanced live config.
    let outdated =
        LinearPhaseEqPlugin::prepare_band_update(&stale, 1, band("Peak", 3000.0, 6.0)).unwrap();
    assert!(plugin.commit_prepared_update(outdated).is_err());
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("band_1_gain")),
        Some(ParameterValue::Float(0.0))
    );

    // Post-drain commits wait for a reset.
    stream_all(&mut plugin, &sine(64, 440.0));
    drain_all(&mut plugin);
    let drained = LinearPhaseEqPlugin::prepare_band_update(
        &plugin.snapshot_config(),
        0,
        band("Peak", 1000.0, 1.0),
    )
    .unwrap();
    assert!(plugin.commit_prepared_update(drained).is_err());
    plugin.reset();
    let drained = LinearPhaseEqPlugin::prepare_band_update(
        &plugin.snapshot_config(),
        0,
        band("Peak", 1000.0, 1.0),
    )
    .unwrap();
    plugin.commit_prepared_update(drained).unwrap();
}

#[test]
fn updated_stream_drain_length_is_exact_publicly() {
    let input = sine(2500, 440.0);
    let mut plugin = LinearPhaseEqPlugin::from_params(
        CHANNELS,
        RATE,
        params_for(vec![band("Peak", 1000.0, 0.0), band("Peak", 3000.0, 0.0)]),
    )
    .unwrap();
    let mut output = stream_all(&mut plugin, &input[..200 * CHANNELS]);
    let snapshot = plugin.snapshot_config();
    plugin
        .commit_prepared_update(
            LinearPhaseEqPlugin::prepare_band_update(&snapshot, 1, band("Peak", 3000.0, -9.0))
                .unwrap(),
        )
        .unwrap();
    output.extend(stream_all(&mut plugin, &input[200 * CHANNELS..]));
    output.extend(drain_all(&mut plugin));
    let support = match plugin.tail_length() {
        sotf_host::plugin::TailLength::Finite(frames) => frames as usize,
        _ => panic!("expected a finite tail"),
    };
    assert_eq!(output.len(), (2500 + support) * CHANNELS);
    assert!(output.iter().all(|sample| sample.is_finite()));
}

#[test]
fn realtime_commit_retains_prepared_on_refusal_publicly() {
    // Public-API retry regression for the realtime entrypoint: the host keeps
    // a caller-owned slot, drives the real in-flight error branch (no
    // preflight), retains the prepared update, and retries the same slot after
    // the first blend completes and the retired route is reclaimed.
    let mut plugin = LinearPhaseEqPlugin::from_params(
        CHANNELS,
        RATE,
        params_for(vec![band("Peak", 1000.0, 0.0), band("Peak", 3000.0, 0.0)]),
    )
    .unwrap();
    stream_all(&mut plugin, &sine(1500, 440.0));
    let mut first = Some(
        LinearPhaseEqPlugin::prepare_band_update(
            &plugin.snapshot_config(),
            0,
            band("Peak", 1000.0, 6.0),
        )
        .unwrap(),
    );
    plugin.try_commit_prepared_update(&mut first).unwrap();
    assert!(first.is_none());
    assert!(plugin.update_in_progress());

    let mut slot = Some(
        LinearPhaseEqPlugin::prepare_band_update(
            &plugin.snapshot_config(),
            1,
            band("Peak", 3000.0, 6.0),
        )
        .unwrap(),
    );
    assert_eq!(
        plugin.try_commit_prepared_update(&mut slot),
        Err(CommitRefusal::UpdateInProgress)
    );
    assert!(slot.is_some(), "refusal must retain the prepared update");
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("band_1_gain")),
        Some(ParameterValue::Float(0.0))
    );

    stream_all(&mut plugin, &sine(1024, 440.0));
    assert!(!plugin.update_in_progress());
    assert_eq!(
        plugin.try_commit_prepared_update(&mut slot),
        Err(CommitRefusal::RetiredUnclaimed)
    );
    assert!(slot.is_some());
    assert!(plugin.take_retired_route().is_some());

    plugin.try_commit_prepared_update(&mut slot).unwrap();
    assert!(slot.is_none());
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("band_1_gain")),
        Some(ParameterValue::Float(6.0))
    );
}
