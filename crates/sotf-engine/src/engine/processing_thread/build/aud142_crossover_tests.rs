use super::{PluginConfig, build_plugin_host};
use crate::plugins::PluginSettings;
use serde_json::{Value, json};
use sotf_plugins::{
    ExternalPlugin, ExternalPluginSandboxMode, ExternalPluginState, PluginDescriptor, PluginFormat,
    PluginScanStatus,
};
use std::path::PathBuf;

const ENGINE_ROUTE_SAMPLE_RATE: u32 = 48_000;
const ENGINE_ROUTE_FRAMES: usize = 257;
const ENGINE_ROUTE_INPUT_CHANNELS: usize = 8;
const ENGINE_ROUTE_BANDS: usize = 4;
const ENGINE_ROUTE_CUTOFFS: [f32; 3] = [840.0, 2_100.0, 5_300.0];
const ENGINE_ROUTE_BLOCKS: [usize; 4] = [1, 63, 127, 66];
const ENGINE_ROUTE_SELECTED_CHANNELS: [usize; 2] = [0, 7];

fn three_band_both() -> PluginConfig {
    PluginConfig::new(
        "crossover",
        serde_json::json!({
            "type": "LR24",
            "frequency": 420.0,
            "output": "both",
            "topology": "bands",
            "extra_frequencies": [2_200.0],
        }),
    )
}

#[test]
fn crossover_both_width_and_band_merge_match_full_audio_reference() {
    const INPUT_CHANNELS: usize = 2;
    const BANDS: usize = 3;
    const FRAMES: usize = 257;

    let crossover = three_band_both();
    let mut split_host =
        build_plugin_host(std::slice::from_ref(&crossover), 48_000, INPUT_CHANNELS)
            .expect("valid Crossover route must build")
            .0;
    assert_eq!(split_host.output_channels(), INPUT_CHANNELS * BANDS);
    assert!(
        split_host.bypass_plugin(0).is_err(),
        "bypassing a width-changing Crossover must be refused"
    );

    let configs = [
        crossover,
        PluginConfig::new("band_merge", serde_json::json!({ "bands": BANDS })),
    ];
    let mut merged_host = build_plugin_host(&configs, 48_000, INPUT_CHANNELS)
        .expect("Crossover output width must feed BandMerge")
        .0;
    assert_eq!(merged_host.output_channels(), INPUT_CHANNELS);

    let input: Vec<f32> = (0..FRAMES)
        .flat_map(|frame| {
            let time = frame as f32 / 48_000.0;
            [
                (std::f32::consts::TAU * 310.0 * time).sin()
                    + 0.27 * (std::f32::consts::TAU * 3_100.0 * time).sin(),
                0.63 * (std::f32::consts::TAU * 730.0 * time + 0.2).sin()
                    - 0.19 * (std::f32::consts::TAU * 4_200.0 * time).sin(),
            ]
        })
        .collect();
    let mut split_audio = vec![0.0; FRAMES * INPUT_CHANNELS * BANDS];
    let split_frames = split_host.process(&input, &mut split_audio).unwrap();
    assert_eq!(split_frames, FRAMES);
    assert!(split_audio.iter().all(|sample| sample.is_finite()));
    assert!(
        split_audio.iter().any(|sample| sample.abs() > 1.0e-4),
        "the split reference must contain nontrivial audio"
    );

    let mut merged_audio = vec![0.0; FRAMES * INPUT_CHANNELS];
    let merged_frames = merged_host.process(&input, &mut merged_audio).unwrap();
    assert_eq!(merged_frames, FRAMES);
    assert!(merged_audio.iter().all(|sample| sample.is_finite()));
    assert!(
        merged_audio.iter().any(|sample| sample.abs() > 1.0e-4),
        "the composed route must contain nontrivial audio"
    );

    for frame in 0..FRAMES {
        for channel in 0..INPUT_CHANNELS {
            let expected = (0..BANDS)
                .map(|band| {
                    split_audio[frame * INPUT_CHANNELS * BANDS + band * INPUT_CHANNELS + channel]
                })
                .sum::<f32>();
            let actual = merged_audio[frame * INPUT_CHANNELS + channel];
            assert!(
                (actual - expected).abs() <= 2.0e-6,
                "BandMerge differs from the independently processed band sum at frame {frame}, channel {channel}: actual={actual}, expected={expected}"
            );
        }
    }
}

#[test]
fn invalid_crossover_candidate_is_not_silently_skipped() {
    let invalid = PluginConfig::new(
        "crossover",
        serde_json::json!({
            "type": "LR24",
            "frequency": 1_000.0,
            "output": "both",
            "topology": "per_channel",
        }),
    );
    assert!(
        build_plugin_host(&[invalid], 48_000, 2).is_err(),
        "an invalid requested Crossover route must abort candidate construction"
    );
}

#[test]
fn actual_crossover_route_width_matches_each_topology_and_mode() {
    let cases = [
        serde_json::json!({
            "type": "LR24",
            "frequency": 800.0,
            "output": "lowpass",
            "topology": "bands",
        }),
        serde_json::json!({
            "type": "LR24",
            "frequency": 800.0,
            "output": "highpass",
            "topology": "bands",
            "extra_frequencies": [3_000.0],
        }),
    ];
    for parameters in cases {
        let config = PluginConfig::new("crossover", parameters);
        let host = build_plugin_host(&[config], 48_000, 4)
            .expect("valid in-place Crossover route must build")
            .0;
        assert_eq!(host.output_channels(), 4);
    }

    let per_channel = PluginConfig::new(
        "crossover",
        serde_json::json!({
            "type": "LR24",
            "frequency": 800.0,
            "output": "both",
            "topology": "per_channel",
            "extra_frequencies": [3_000.0],
            "channel_frequencies_hz": [500.0, 1_500.0, 2_500.0, 3_500.0],
            "channel_modes": ["lowpass", "highpass", "mute", "passthrough"],
        }),
    );
    let host = build_plugin_host(&[per_channel], 48_000, 4)
        .expect("valid explicit per-channel route must build")
        .0;
    assert_eq!(host.output_channels(), 4);
}

#[test]
#[ignore = "requires fresh Crossover CLAP and VST3 artifacts and SOTF_CROSSOVER_ENGINE_CAPTURE_DIR"]
fn loaded_native_crossover_flows_through_engine_merge_and_matrix() {
    use sotf_plugins::Plugin;

    let capture_root = PathBuf::from(
        std::env::var_os("SOTF_CROSSOVER_ENGINE_CAPTURE_DIR")
            .expect("set SOTF_CROSSOVER_ENGINE_CAPTURE_DIR to save loaded route captures"),
    );
    let routes = [
        (
            "clap",
            PluginFormat::Clap,
            "org.spinorama.sotf.crossover",
            "SOTF_TEST_CROSSOVER_CLAP_PLUGIN",
            "clap_packed",
        ),
        (
            "vst3",
            PluginFormat::Vst3,
            "536F746643726F73736F766572303031",
            "SOTF_TEST_CROSSOVER_VST3_PLUGIN",
            "vst3_buses",
        ),
    ];

    for (format_name, format, plugin_id, library_env, output_layout) in routes {
        let descriptor = PluginDescriptor {
            id: plugin_id.into(),
            name: "SOTF: Crossover".into(),
            vendor: "SOTF".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            format,
            path: PathBuf::from(
                std::env::var_os(library_env)
                    .unwrap_or_else(|| panic!("set {library_env} to the fresh plugin artifact")),
            ),
            // Keep the scanned stereo declaration stale to exercise typed setup
            // through the consuming engine builder.
            audio_inputs: 2,
            audio_outputs: 2,
            is_instrument: false,
            categories: vec!["audio-effect".into()],
            scan_status: PluginScanStatus::Loadable,
        };
        let audio_setup = json!({
            "type": "crossover",
            "input_layout": "seven_one",
            "num_bands": ENGINE_ROUTE_BANDS,
            "topology": "bands",
            "mode": "both",
            "output_layout": output_layout,
        });
        // Capture native defaults from a correctly negotiated direct instance.
        // The consuming engine route below uses its supported isolated-worker
        // configuration because serialized PluginConfig cannot grant trust.
        let mut seed_state = ExternalPluginState::new(
            descriptor.clone(),
            ExternalPluginSandboxMode::InProcess,
            Vec::new(),
        );
        let mut seed_value =
            serde_json::to_value(&seed_state).expect("serialize native seed state");
        seed_value["audio_setup"] = audio_setup.clone();
        seed_state =
            serde_json::from_value(seed_value).expect("deserialize typed native seed setup");
        let seed = ExternalPlugin::from_placeholder_state(&seed_state, ENGINE_ROUTE_SAMPLE_RATE)
            .unwrap_or_else(|error| panic!("load {format_name} Crossover seed: {error}"));
        let mut opaque_state = seed
            .save_opaque_state()
            .unwrap_or_else(|error| panic!("save {format_name} Crossover seed: {error}"));
        set_engine_route_crossover_state(&mut opaque_state, format);
        drop(seed);

        let mut external_state = ExternalPluginState::new(
            descriptor,
            ExternalPluginSandboxMode::Isolated,
            opaque_state,
        );
        let mut serialized_state =
            serde_json::to_value(&external_state).expect("serialize isolated placeholder state");
        serialized_state["audio_setup"] = audio_setup;
        external_state = serde_json::from_value(serialized_state)
            .expect("deserialize supported typed Crossover setup");

        let mut crossover_config = PluginSettings::External {
            state: external_state.clone(),
        }
        .to_plugin_config(f64::from(ENGINE_ROUTE_SAMPLE_RATE));
        crossover_config
            .parameters
            .as_object_mut()
            .expect("external plugin config has an object")
            .insert("max_block_frames".into(), json!(127));

        let (mut split_host, split_diagnostics) = build_plugin_host(
            std::slice::from_ref(&crossover_config),
            ENGINE_ROUTE_SAMPLE_RATE,
            ENGINE_ROUTE_INPUT_CHANNELS,
        )
        .unwrap_or_else(|error| panic!("build {format_name} native split host: {error}"));
        assert!(
            split_diagnostics.is_empty(),
            "{format_name} native split route must not be silently skipped: {split_diagnostics:?}"
        );
        assert_eq!(split_host.plugin_count(), 1);
        assert_eq!(split_host.input_channels(), ENGINE_ROUTE_INPUT_CHANNELS);
        let transport_latency = split_host.total_latency_samples();
        assert_eq!(transport_latency, 127);
        assert_eq!(
            split_host.output_channels(),
            ENGINE_ROUTE_INPUT_CHANNELS * ENGINE_ROUTE_BANDS,
            "{format_name} native Both4 must expose 8→32 to the engine builder"
        );
        let split_workers = split_host.ensure_isolated_external_plugin_workers_running();
        assert_eq!(
            split_workers.len(),
            1,
            "{format_name} split worker is present"
        );
        assert!(
            split_workers.iter().all(|report| {
                report.error.is_none()
                    && report.worker_start_count > 0
                    && report.worker_launch_failure_count == 0
                    && !report.worker_quarantined
            }),
            "{format_name} split worker did not start cleanly: {split_workers:?}"
        );

        let mut matrix = vec![0.0_f32; ENGINE_ROUTE_SELECTED_CHANNELS.len() * 8];
        matrix[ENGINE_ROUTE_SELECTED_CHANNELS[0]] = 1.0;
        matrix[8 + ENGINE_ROUTE_SELECTED_CHANNELS[1]] = 1.0;
        let preset_plugins = json!([
            {
                "id": 0,
                "enabled": true,
                "permanent": false,
                "settings": PluginSettings::External {
                    state: external_state.clone(),
                },
            },
            {
                "id": 1,
                "enabled": true,
                "permanent": false,
                "settings": PluginSettings::BandMerge {
                    channels: ENGINE_ROUTE_INPUT_CHANNELS,
                    bands: ENGINE_ROUTE_BANDS,
                },
            },
            {
                "id": 2,
                "enabled": true,
                "permanent": false,
                "settings": PluginSettings::Matrix {
                    input_channels: ENGINE_ROUTE_INPUT_CHANNELS,
                    output_channels: ENGINE_ROUTE_SELECTED_CHANNELS.len(),
                    matrix,
                    channel_states: Vec::new(),
                },
            },
        ]);
        let chain_preset = json!({ "version": 2, "plugins": preset_plugins });
        let preset_dir =
            tempfile::tempdir().expect("create native Crossover chain preset directory");
        std::fs::write(
            preset_dir.path().join("native-crossover-route.json"),
            serde_json::to_vec(&chain_preset).expect("serialize native Crossover chain preset"),
        )
        .expect("write native Crossover chain preset");
        let mut plugin_chain = crate::plugins::PluginChain::new();
        let preset_warnings = plugin_chain
            .load_from_file(preset_dir.path(), "native-crossover-route")
            .expect("load native Crossover chain preset");
        assert!(
            preset_warnings.is_empty(),
            "{format_name} typed native chain preset must load without skipped rows: {preset_warnings:?}"
        );
        assert_eq!(
            plugin_chain.plugins().len(),
            7,
            "three user rows are wrapped by the four permanent rack plugins"
        );
        assert_eq!(
            plugin_chain
                .plugins()
                .iter()
                .filter(|plugin| plugin.permanent)
                .count(),
            4,
            "the normal input monitor, replay gain, matrix, and output monitor remain present"
        );
        assert!(
            plugin_chain
                .find_channel_conflicts(ENGINE_ROUTE_INPUT_CHANNELS)
                .is_empty(),
            "{format_name} PluginChain planner must accept typed 8→32→8→2 geometry"
        );
        assert_eq!(
            plugin_chain.output_channels_for_input(ENGINE_ROUTE_INPUT_CHANNELS),
            ENGINE_ROUTE_SELECTED_CHANNELS.len(),
            "{format_name} PluginChain planner must propagate typed output width through BandMerge and Matrix"
        );

        let mut chain_configs = plugin_chain.to_plugin_configs(f64::from(ENGINE_ROUTE_SAMPLE_RATE));
        let config_types = chain_configs
            .iter()
            .map(|config| config.plugin_type.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            config_types,
            [
                "loudness_monitor",
                "external",
                "band_merge",
                "matrix",
                "matrix",
                "loudness_monitor",
            ],
            "the normal permanent rack rows must be included in chain conversion"
        );
        let external_chain_config = chain_configs
            .iter_mut()
            .find(|config| config.plugin_type == "external")
            .expect("PluginChain conversion must retain its typed External Crossover row");
        external_chain_config
            .parameters
            .as_object_mut()
            .expect("external plugin config has an object")
            .insert("max_block_frames".into(), json!(127));
        let (mut engine_host, engine_diagnostics) = build_plugin_host(
            &chain_configs,
            ENGINE_ROUTE_SAMPLE_RATE,
            ENGINE_ROUTE_INPUT_CHANNELS,
        )
        .unwrap_or_else(|error| panic!("build {format_name} Crossover→BandMerge→Matrix: {error}"));
        assert!(
            engine_diagnostics.is_empty(),
            "{format_name} full native chain must not skip plugins: {engine_diagnostics:?}"
        );
        assert_eq!(engine_host.plugin_count(), chain_configs.len());
        assert_eq!(engine_host.input_channels(), ENGINE_ROUTE_INPUT_CHANNELS);
        assert_eq!(engine_host.total_latency_samples(), transport_latency);
        assert_eq!(
            engine_host.output_channels(),
            ENGINE_ROUTE_SELECTED_CHANNELS.len()
        );
        let chain_workers = engine_host.ensure_isolated_external_plugin_workers_running();
        assert_eq!(
            chain_workers.len(),
            1,
            "{format_name} chain worker is present"
        );
        assert!(
            chain_workers.iter().all(|report| {
                report.error.is_none()
                    && report.worker_start_count > 0
                    && report.worker_launch_failure_count == 0
                    && !report.worker_quarantined
            }),
            "{format_name} chain worker did not start cleanly: {chain_workers:?}"
        );

        let input = engine_route_input();
        let split_audio = process_in_blocks(
            &mut split_host,
            &input,
            ENGINE_ROUTE_INPUT_CHANNELS,
            ENGINE_ROUTE_INPUT_CHANNELS * ENGINE_ROUTE_BANDS,
            transport_latency,
        );
        let actual = process_in_blocks(
            &mut engine_host,
            &input,
            ENGINE_ROUTE_INPUT_CHANNELS,
            ENGINE_ROUTE_SELECTED_CHANNELS.len(),
            transport_latency,
        );
        let expected = merged_selected_channels(&split_audio);
        let split_worker_reports = split_host.poll_isolated_external_plugin_workers();
        let chain_worker_reports = engine_host.poll_isolated_external_plugin_workers();
        for (route, reports) in [
            ("split", split_worker_reports),
            ("chain", chain_worker_reports),
        ] {
            assert_eq!(
                reports.len(),
                1,
                "{format_name} {route} worker report exists"
            );
            assert!(
                reports.iter().all(|report| {
                    report.error.is_none()
                        && report.block_timeout_count == 0
                        && report.block_worker_failure_count == 0
                        && report.block_wrong_sequence_count == 0
                        && report.worker_launch_failure_count == 0
                        && !report.worker_quarantined
                }),
                "{format_name} {route} emitted fallback or worker failure: {reports:?}"
            );
        }
        assert_eq!(
            actual.len(),
            (ENGINE_ROUTE_FRAMES + transport_latency) * ENGINE_ROUTE_SELECTED_CHANNELS.len()
        );
        assert!(actual.iter().all(|sample| sample.is_finite()));
        assert!(actual.iter().any(|sample| sample.abs() > 1.0e-4));
        for (index, (actual_sample, expected_sample)) in actual.iter().zip(&expected).enumerate() {
            assert!(
                (actual_sample - expected_sample).abs() <= 2.0e-6,
                "{format_name} engine chain differs from explicit native band sum and channel selection at sample {index}: actual={actual_sample}, expected={expected_sample}"
            );
        }

        let format_capture = capture_root.join(format_name);
        std::fs::create_dir_all(&format_capture).expect("create native route capture directory");
        std::fs::write(
            format_capture.join("input-sotf-order.f32le"),
            f32_le_bytes(&input),
        )
        .expect("save canonical engine input");
        std::fs::write(
            format_capture.join("actual-engine-chain.f32le"),
            f32_le_bytes(&actual),
        )
        .expect("save loaded engine route output");
        let merge_gains = vec![1.0; ENGINE_ROUTE_BANDS];
        std::fs::write(
            format_capture.join("case.json"),
            serde_json::to_vec_pretty(&json!({
                "format": format_name,
                "sample_rate": ENGINE_ROUTE_SAMPLE_RATE,
                "frames": ENGINE_ROUTE_FRAMES,
                "output_frames": actual.len() / ENGINE_ROUTE_SELECTED_CHANNELS.len(),
                "transport_latency_frames": transport_latency,
                "input_channels": ENGINE_ROUTE_INPUT_CHANNELS,
                "split_channels": ENGINE_ROUTE_INPUT_CHANNELS * ENGINE_ROUTE_BANDS,
                "merged_channels": ENGINE_ROUTE_INPUT_CHANNELS,
                "output_channels": ENGINE_ROUTE_SELECTED_CHANNELS.len(),
                "cutoffs": ENGINE_ROUTE_CUTOFFS,
                "selected_channels": ENGINE_ROUTE_SELECTED_CHANNELS,
                "merge_gains": merge_gains,
                "merge_normalization": "none",
                "channel_order": "sotf_interleaved",
                "test_worker_pacing_ms": 25,
                "realtime_claim": false,
            }))
            .expect("serialize capture metadata"),
        )
        .expect("save route metadata");
    }
}

fn set_engine_route_crossover_state(opaque_state: &mut Vec<u8>, format: PluginFormat) {
    let (prefix, payload) = if format == PluginFormat::Clap {
        let prefix = opaque_state
            .get(..8)
            .expect("CLAP state has its length prefix")
            .to_vec();
        let payload = opaque_state.get(8..).unwrap_or_default();
        assert_eq!(
            payload.len(),
            usize::try_from(u64::from_le_bytes(prefix.as_slice().try_into().unwrap())).unwrap()
        );
        (Some(prefix), payload)
    } else {
        (None, opaque_state.as_slice())
    };
    let mut state: Value = serde_json::from_slice(payload)
        .unwrap_or_else(|error| panic!("parse {format:?} Crossover state: {error}"));
    let params = state
        .get_mut("params")
        .and_then(Value::as_object_mut)
        .expect("native Crossover state has parameter map");
    params.insert("family".into(), json!({ "i32": 0 }));
    params.insert(
        "frequency".into(),
        json!({ "f32": ENGINE_ROUTE_CUTOFFS[0] }),
    );
    params.insert(
        "frequency_2".into(),
        json!({ "f32": ENGINE_ROUTE_CUTOFFS[1] }),
    );
    params.insert(
        "frequency_3".into(),
        json!({ "f32": ENGINE_ROUTE_CUTOFFS[2] }),
    );
    params.insert("mode".into(), json!({ "i32": 2 }));
    params.insert("topology".into(), json!({ "i32": 0 }));
    params.insert("band_count".into(), json!({ "i32": 2 }));
    let payload = serde_json::to_vec(&state).expect("serialize native Crossover state");
    opaque_state.clear();
    if let Some(mut prefix) = prefix {
        prefix.copy_from_slice(
            &u64::try_from(payload.len())
                .expect("CLAP state length fits u64")
                .to_le_bytes(),
        );
        opaque_state.extend_from_slice(&prefix);
    }
    opaque_state.extend_from_slice(&payload);
}

fn engine_route_input() -> Vec<f32> {
    (0..ENGINE_ROUTE_FRAMES)
        .flat_map(|frame| {
            (0..ENGINE_ROUTE_INPUT_CHANNELS).map(move |channel| {
                let time = frame as f32 / ENGINE_ROUTE_SAMPLE_RATE as f32;
                let phase = channel as f32 * 0.173;
                let scale = 0.55 + channel as f32 * 0.021;
                let sample =
                    (std::f32::consts::TAU * (127.0 + channel as f32 * 13.0) * time + phase).sin()
                        * 0.11
                        + (std::f32::consts::TAU * 1_240.0 * time + phase * 0.71).sin() * 0.047
                        + (std::f32::consts::TAU * 4_100.0 * time - phase * 0.37).cos() * 0.031;
                sample * scale
            })
        })
        .collect()
}

fn process_in_blocks(
    host: &mut sotf_plugins::PluginHost,
    input: &[f32],
    input_channels: usize,
    output_channels: usize,
    continuation_frames: usize,
) -> Vec<f32> {
    assert_eq!(input.len(), ENGINE_ROUTE_FRAMES * input_channels);
    assert_eq!(
        ENGINE_ROUTE_BLOCKS.iter().sum::<usize>(),
        ENGINE_ROUTE_FRAMES
    );
    let mut output =
        Vec::with_capacity((ENGINE_ROUTE_FRAMES + continuation_frames) * output_channels);
    let mut frame_offset = 0;
    for frames in ENGINE_ROUTE_BLOCKS {
        let input_start = frame_offset * input_channels;
        let input_end = (frame_offset + frames) * input_channels;
        let mut block_output = vec![0.0; frames * output_channels];
        assert_eq!(
            host.process(&input[input_start..input_end], &mut block_output)
                .expect("engine host processes loaded route block"),
            frames
        );
        assert!(block_output.iter().all(|sample| sample.is_finite()));
        output.extend_from_slice(&block_output);
        frame_offset += frames;
        // This harness checks offline numerical routing, not realtime worker
        // deadlines. Let the isolated worker finish each request before the
        // next callback so fallback audio cannot masquerade as DSP output.
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let mut remaining = continuation_frames;
    while remaining > 0 {
        let frames = remaining.min(127);
        let silence = vec![0.0_f32; frames * input_channels];
        let mut block_output = vec![0.0_f32; frames * output_channels];
        assert_eq!(
            host.process(&silence, &mut block_output)
                .expect("engine host processes declared-latency silence continuation"),
            frames
        );
        assert!(block_output.iter().all(|sample| sample.is_finite()));
        output.extend_from_slice(&block_output);
        remaining -= frames;
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    output
}

fn merged_selected_channels(split_audio: &[f32]) -> Vec<f32> {
    let frames = split_audio.len() / (ENGINE_ROUTE_INPUT_CHANNELS * ENGINE_ROUTE_BANDS);
    assert_eq!(
        split_audio.len(),
        frames * ENGINE_ROUTE_INPUT_CHANNELS * ENGINE_ROUTE_BANDS
    );
    let mut merged = vec![0.0_f32; frames * ENGINE_ROUTE_SELECTED_CHANNELS.len()];
    for frame in 0..frames {
        for (output_channel, &source_channel) in ENGINE_ROUTE_SELECTED_CHANNELS.iter().enumerate() {
            merged[frame * ENGINE_ROUTE_SELECTED_CHANNELS.len() + output_channel] = (0
                ..ENGINE_ROUTE_BANDS)
                .map(|band| {
                    split_audio[frame * ENGINE_ROUTE_INPUT_CHANNELS * ENGINE_ROUTE_BANDS
                        + band * ENGINE_ROUTE_INPUT_CHANNELS
                        + source_channel]
                })
                .sum();
        }
    }
    merged
}

fn f32_le_bytes(samples: &[f32]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect()
}
