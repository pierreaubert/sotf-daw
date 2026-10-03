//! Engine production-path proof for graph sidechain routing.
//!
//! Program/key source nodes feed a keyed dynamics node through persisted
//! [`PluginGraphConfig`] built by [`build_plugin_graph_host`]; the key bus
//! carries genuinely independent channel data selected from the host input
//! by matrix entry nodes, never a copy of the program stream.

use super::super::build::build_plugin_graph_host;
use crate::engine::{
    PluginBuildTarget, PluginGraphConfig, PluginGraphEdgeConfig, PluginGraphEdgeKind,
    PluginGraphNodeConfig,
};
use sotf_plugins::PluginHost;

const SAMPLE_RATE: u32 = 48_000;

/// 4-in/2-out channel selector: output `i` carries host input `select[i]`.
fn sidechain_matrix_params(select: [usize; 2]) -> serde_json::Value {
    let mut matrix = vec![0.0f32; 8];
    matrix[select[0]] = 1.0;
    matrix[4 + select[1]] = 1.0;
    serde_json::json!({
        "input_channels": 4,
        "output_channels": 2,
        "matrix": matrix,
    })
}

fn sidechain_gate_params() -> serde_json::Value {
    serde_json::json!({
        "threshold_db": -20.0,
        "ratio": 10.0,
        "attack_ms": 0.1,
        "hold_ms": 0.0,
        "release_ms": 10.0,
        "mix": 1.0,
        "link_channels": false,
        "sidechain_external": true,
        "range_db": 80.0,
        "hysteresis_db": 0.0,
        "knee_db": 0.0,
        "lookahead_ms": 0.0,
    })
}

fn sidechain_deesser_params() -> serde_json::Value {
    serde_json::json!({
        "frequency": 7000.0,
        "q": 1.5,
        "threshold": -20.0,
        "ratio": 8.0,
        "attack_ms": 0.5,
        "release_ms": 20.0,
        "mode": "Wideband",
        "mix": 1.0,
        "range_db": 60.0,
        "stereo_link": 0.0,
        "lookahead_ms": 0.0,
        "split_topology": "Minimum-Phase",
        "ms_mode": false,
        "sidechain_external": false,
    })
}

fn sidechain_deesser_key_params() -> serde_json::Value {
    let mut params = sidechain_deesser_params();
    params["sidechain_external"] = serde_json::json!(true);
    params
}

/// Program matrix -> dynamics node via audio, key matrix -> dynamics node
/// via sidechain. Selections index the 4-channel host input.
fn sidechain_key_graph(program_select: [usize; 2], key_select: [usize; 2]) -> PluginGraphConfig {
    PluginGraphConfig::try_new(
        vec![
            PluginGraphNodeConfig::try_new(1, "matrix", sidechain_matrix_params(program_select), 4)
                .unwrap(),
            PluginGraphNodeConfig::try_new(2, "matrix", sidechain_matrix_params(key_select), 4)
                .unwrap(),
            PluginGraphNodeConfig::try_new(3, "gate", sidechain_gate_params(), 2).unwrap(),
        ],
        vec![
            PluginGraphEdgeConfig::new(1, 3),
            PluginGraphEdgeConfig::sidechain(2, 3),
        ],
    )
    .unwrap()
}

/// Program matrix -> external-key DeEsser via audio, key matrix -> DeEsser
/// via sidechain. Selections index the 4-channel host input.
fn sidechain_deesser_key_graph(
    program_select: [usize; 2],
    key_select: [usize; 2],
) -> PluginGraphConfig {
    PluginGraphConfig::try_new(
        vec![
            PluginGraphNodeConfig::try_new(1, "matrix", sidechain_matrix_params(program_select), 4)
                .unwrap(),
            PluginGraphNodeConfig::try_new(2, "matrix", sidechain_matrix_params(key_select), 4)
                .unwrap(),
            PluginGraphNodeConfig::try_new(3, "de_esser", sidechain_deesser_key_params(), 2)
                .unwrap(),
        ],
        vec![
            PluginGraphEdgeConfig::new(1, 3),
            PluginGraphEdgeConfig::sidechain(2, 3),
        ],
    )
    .unwrap()
}

/// 4-channel host input: quiet 1 kHz program on [0, 1], independent 1.5 kHz
/// key at `key_peak` on [2, 3].
fn sidechain_key_input(frames: usize, key_peak: f32) -> Vec<f32> {
    let mut input = vec![0.0f32; frames * 4];
    for i in 0..frames {
        let t = i as f32 / SAMPLE_RATE as f32;
        let program = 0.03 * (std::f32::consts::TAU * 1_000.0 * t).sin();
        let key = key_peak * (std::f32::consts::TAU * 1_500.0 * t).sin();
        input[i * 4] = program;
        input[i * 4 + 1] = program;
        input[i * 4 + 2] = key;
        input[i * 4 + 3] = key;
    }
    input
}

/// 4-channel host input: 8 kHz program at `program_peak` on [0, 1],
/// independent 8 kHz key at `key_peak` on [2, 3].
fn sidechain_deesser_key_input(frames: usize, program_peak: f32, key_peak: f32) -> Vec<f32> {
    let mut input = vec![0.0f32; frames * 4];
    for i in 0..frames {
        let t = i as f32 / SAMPLE_RATE as f32;
        let program = program_peak * (std::f32::consts::TAU * 8_000.0 * t).sin();
        let key = key_peak * (std::f32::consts::TAU * 8_000.0 * t).sin();
        input[i * 4] = program;
        input[i * 4 + 1] = program;
        input[i * 4 + 2] = key;
        input[i * 4 + 3] = key;
    }
    input
}

fn sidechain_rms(samples: &[f32]) -> f32 {
    let sum: f32 = samples.iter().map(|sample| sample * sample).sum();
    (sum / samples.len() as f32).sqrt()
}

fn sidechain_channel_rms(
    interleaved: &[f32],
    width: usize,
    channel: usize,
    from_frame: usize,
) -> f32 {
    let channel: Vec<f32> = interleaved
        .chunks(width)
        .skip(from_frame)
        .map(|frame| frame[channel])
        .collect();
    sidechain_rms(&channel)
}

fn sidechain_build(graph: &PluginGraphConfig, channels: usize) -> PluginHost {
    let (host, warnings) = build_plugin_graph_host(graph, SAMPLE_RATE, channels).unwrap();
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    host
}

fn sidechain_render(host: &mut PluginHost, input: &[f32], frames: usize) -> Vec<f32> {
    let mut output = vec![0.0f32; frames * 2];
    let done = host
        .process(input, &mut output)
        .expect("graph must process");
    assert_eq!(done, frames);
    output
}

#[test]
fn graph_sidechain_key_drives_gate_processing() {
    // Hot key opens the gate on a quiet program; silent key leaves it
    // closed. Both renders run through the persisted engine graph build,
    // and the program-2ch output carries a sentinel guard proving no key
    // leak past the program bus.
    let frames = 8_192usize;
    let graph = sidechain_key_graph([0, 1], [2, 3]);

    let mut hot = sidechain_build(&graph, 4);
    assert_eq!(hot.total_latency_samples(), 0);
    let hot_input = sidechain_key_input(frames, 0.5);
    let mut hot_output = vec![-999.0f32; frames * 4];
    let done = hot
        .process(&hot_input, &mut hot_output)
        .expect("graph must process");
    assert_eq!(done, frames);
    assert!(
        hot_output[frames * 2..]
            .iter()
            .all(|&x| x.to_bits() == (-999.0f32).to_bits()),
        "program output must stay 2ch with no key leak"
    );
    let program_rms = sidechain_channel_rms(&hot_input, 4, 0, frames / 2);
    // Measure the 2ch program region only: the guard half of `hot_output`
    // still holds the -999 sentinel and must not enter the RMS window.
    let hot_rms = sidechain_channel_rms(&hot_output[..frames * 2], 2, 0, frames / 2);
    let hot_db = 20.0 * (hot_rms / program_rms).log10();
    assert!(
        hot_db.abs() < 1.0,
        "open gate must pass the program within 1 dB, got {hot_db:.2} dB"
    );
    assert!(hot_output[..frames * 2].iter().all(|x| x.is_finite()));

    let mut silent = sidechain_build(&graph, 4);
    let silent_input = sidechain_key_input(frames, 0.0);
    let silent_output = sidechain_render(&mut silent, &silent_input, frames);
    let silent_rms = sidechain_channel_rms(&silent_output, 2, 0, frames / 2);
    let silent_db = 20.0 * (silent_rms / program_rms).log10();
    assert!(
        silent_db < -60.0,
        "closed gate must attenuate past -60 dB, got {silent_db:.2} dB"
    );
}

#[test]
fn graph_sidechain_bus_ordering_is_program_then_key() {
    // Swapped selections put the hot tone on the program bus and the quiet
    // tone on the key bus: the detector sees quiet and the hot program is
    // gated. A swapped or duplicated key bus would open the gate instead.
    let frames = 8_192usize;
    let graph = sidechain_key_graph([2, 3], [0, 1]);
    let mut host = sidechain_build(&graph, 4);
    let input = sidechain_key_input(frames, 0.5);
    let output = sidechain_render(&mut host, &input, frames);
    let hot_program_rms = sidechain_channel_rms(&input, 4, 2, frames / 2);
    let out_rms = sidechain_channel_rms(&output, 2, 0, frames / 2);
    let ratio_db = 20.0 * (out_rms / hot_program_rms).log10();
    assert!(
        ratio_db < -60.0,
        "quiet key must gate the hot program past -60 dB, got {ratio_db:.2} dB"
    );
}

#[test]
fn graph_sidechain_source_nodes_emit_selected_channels_exactly() {
    // The key-source layer is pinned independently of any keyed dynamics:
    // single-node matrix graphs reproduce the selected host channels.
    let frames = 1_024usize;
    let input = sidechain_key_input(frames, 0.5);
    for (select, channel) in [([0, 1], 0usize), ([2, 3], 2)] {
        let graph = PluginGraphConfig::try_new(
            vec![
                PluginGraphNodeConfig::try_new(1, "matrix", sidechain_matrix_params(select), 4)
                    .unwrap(),
            ],
            vec![],
        )
        .unwrap();
        let mut host = sidechain_build(&graph, 4);
        let output = sidechain_render(&mut host, &input, frames);
        for frame in 0..frames {
            for channel_offset in 0..2 {
                let got = output[frame * 2 + channel_offset];
                let want = input[frame * 4 + channel + channel_offset];
                assert!(
                    (got - want).abs() < 1e-6,
                    "matrix must select host channels exactly: frame {frame} got {got} want {want}"
                );
            }
        }
    }
}

#[test]
fn graph_sidechain_config_save_reload_renders_bitwise() {
    // Persisted external-key graphs (edge kind + sidechain toggle) reload
    // to bit-identical renders.
    let frames = 4_096usize;
    let graph = sidechain_key_graph([0, 1], [2, 3]);
    let saved = serde_json::to_string(&graph).unwrap();
    assert!(saved.contains("\"sidechain\""), "edge kind must persist");
    assert!(
        saved.contains("\"sidechain_external\":true"),
        "external toggle must persist"
    );
    let reloaded: PluginGraphConfig = serde_json::from_str(&saved).unwrap();
    assert_eq!(
        serde_json::to_value(&reloaded).unwrap(),
        serde_json::to_value(&graph).unwrap()
    );

    let input = sidechain_key_input(frames, 0.5);
    let mut original = sidechain_build(&graph, 4);
    let mut rebuilt = sidechain_build(&reloaded, 4);
    let first = sidechain_render(&mut original, &input, frames);
    let second = sidechain_render(&mut rebuilt, &input, frames);
    assert_eq!(first, second, "save/reload must render bit-identical audio");
}

#[test]
fn graph_sidechain_chunked_render_matches_single_block() {
    // Sample-clock continuity across process calls on a keyed graph.
    let frames = 4_096usize;
    let graph = sidechain_key_graph([0, 1], [2, 3]);
    let input = sidechain_key_input(frames, 0.5);

    let mut single = sidechain_build(&graph, 4);
    let once = sidechain_render(&mut single, &input, frames);

    let mut chunked = sidechain_build(&graph, 4);
    let mut twice = Vec::with_capacity(once.len());
    for half in input.chunks(frames * 2) {
        twice.extend(sidechain_render(&mut chunked, half, frames / 2));
    }
    assert_eq!(once, twice, "chunked keyed render must match one block");
}

#[test]
fn graph_sidechain_refuses_key_bus_less_target_and_continues_twin() {
    // A sidechain edge into a 2-in/2-out node has no key bus to land on
    // and is refused with an edge-targeted diagnostic. The accepted twin
    // keeps rendering across the refusal event.
    let bad = PluginGraphConfig::try_new(
        vec![
            PluginGraphNodeConfig::try_new(1, "matrix", sidechain_matrix_params([0, 1]), 4)
                .unwrap(),
            PluginGraphNodeConfig::try_new(2, "matrix", sidechain_matrix_params([2, 3]), 4)
                .unwrap(),
            PluginGraphNodeConfig::try_new(3, "gain", serde_json::json!({"gain_db": 0.0}), 2)
                .unwrap(),
        ],
        vec![
            PluginGraphEdgeConfig::new(1, 3),
            PluginGraphEdgeConfig::sidechain(2, 3),
        ],
    )
    .unwrap();
    let error = match build_plugin_graph_host(&bad, SAMPLE_RATE, 4) {
        Ok(_) => panic!("sidechain edge into a bus-less node must be refused"),
        Err(error) => error,
    };
    assert!(
        matches!(
            error.target,
            PluginBuildTarget::GraphEdge {
                from_node: 2,
                to_node: 3
            }
        ),
        "refusal must target the sidechain edge, got {target:?}",
        target = error.target.clone()
    );
    let message = error.message.as_str();
    assert!(message.contains("sidechain"), "got: {message}");
    assert!(message.contains("key bus"), "got: {message}");

    // Twin continuation: an accepted keyed route renders across the event.
    let frames = 4_096usize;
    let graph = sidechain_key_graph([0, 1], [2, 3]);
    let input = sidechain_key_input(frames, 0.5);
    let mut twin = sidechain_build(&graph, 4);
    let mut continued = sidechain_render(&mut twin, &input[..frames * 2], frames / 2);
    // Populated-history proof: the twin was engaged (gate open) before the
    // refusal event, not idling on zeros.
    let first_rms = sidechain_rms(&continued);
    let first_ref = sidechain_channel_rms(&input[..frames * 2], 4, 0, 0);
    let first_db = 20.0 * (first_rms / first_ref).log10();
    assert!(
        first_db.abs() < 1.0,
        "twin must hold engaged history across refusal, got {first_db:.2} dB"
    );
    let refused = build_plugin_graph_host(&bad, SAMPLE_RATE, 4);
    assert!(refused.is_err(), "refusal must stay loud on repeat");
    continued.extend(sidechain_render(
        &mut twin,
        &input[frames * 2..],
        frames / 2,
    ));

    let mut reference = sidechain_build(&graph, 4);
    let mut expected = sidechain_render(&mut reference, &input[..frames * 2], frames / 2);
    expected.extend(sidechain_render(
        &mut reference,
        &input[frames * 2..],
        frames / 2,
    ));
    assert_eq!(continued, expected, "twin history must survive refusal");
}

#[test]
fn graph_sidechain_refuses_overwide_key_routing() {
    // Key width past the target key bus would truncate silently in the
    // host merge, so the build refuses it: once via a single 4ch key
    // source, once via cumulative packing across two key edges.
    let identity_4x4: Vec<f32> = (0..16).map(|i| f32::from((i % 5 == 0) as u8)).collect();
    let wide_key = PluginGraphConfig::try_new(
        vec![
            PluginGraphNodeConfig::try_new(1, "matrix", sidechain_matrix_params([0, 1]), 4)
                .unwrap(),
            PluginGraphNodeConfig::try_new(
                2,
                "matrix",
                serde_json::json!({
                    "input_channels": 4,
                    "output_channels": 4,
                    "matrix": identity_4x4,
                }),
                4,
            )
            .unwrap(),
            PluginGraphNodeConfig::try_new(3, "gate", sidechain_gate_params(), 2).unwrap(),
        ],
        vec![
            PluginGraphEdgeConfig::new(1, 3),
            PluginGraphEdgeConfig::sidechain(2, 3),
        ],
    )
    .unwrap();
    let error = match build_plugin_graph_host(&wide_key, SAMPLE_RATE, 4) {
        Ok(_) => panic!("over-wide key routing must be refused"),
        Err(error) => error,
    };
    assert!(
        error.message.contains("key bus"),
        "got: {message}",
        message = error.message.as_str()
    );

    let packed_keys = PluginGraphConfig::try_new(
        vec![
            PluginGraphNodeConfig::try_new(1, "matrix", sidechain_matrix_params([0, 1]), 4)
                .unwrap(),
            PluginGraphNodeConfig::try_new(2, "matrix", sidechain_matrix_params([2, 3]), 4)
                .unwrap(),
            PluginGraphNodeConfig::try_new(3, "matrix", sidechain_matrix_params([0, 1]), 4)
                .unwrap(),
            PluginGraphNodeConfig::try_new(4, "gate", sidechain_gate_params(), 2).unwrap(),
        ],
        vec![
            PluginGraphEdgeConfig::new(1, 4),
            PluginGraphEdgeConfig::sidechain(2, 4),
            PluginGraphEdgeConfig::sidechain(3, 4),
        ],
    )
    .unwrap();
    let error = match build_plugin_graph_host(&packed_keys, SAMPLE_RATE, 4) {
        Ok(_) => panic!("cumulative key overflow must be refused"),
        Err(error) => error,
    };
    let message = error.message.as_str();
    assert!(
        message.contains("4") && message.contains("2-channel key bus"),
        "cumulative key width must be reported, got: {message}"
    );
}

#[test]
fn graph_sidechain_drain_delivers_branched_tail() {
    // Keyed graphs are branched: end-of-stream drain delivers every
    // branch tail instead of refusing. This zero-lookahead gate holds
    // no tail, so the drain completes immediately with zero frames.
    let graph = sidechain_key_graph([0, 1], [2, 3]);
    let mut host = sidechain_build(&graph, 4);
    let input = sidechain_key_input(512, 0.5);
    sidechain_render(&mut host, &input, 512);
    let mut frames = 0;
    let mut completed = false;
    for _ in 0..64 {
        let mut tail = vec![0.0f32; 8_192 * 4];
        let result = host.drain(&mut tail).unwrap();
        assert!(tail.iter().all(|x| x.is_finite()));
        frames += result.frames;
        if result.complete {
            completed = true;
            break;
        }
    }
    assert!(completed, "branched drain must complete");
    assert_eq!(frames, 0);
    let repeat = host.drain(&mut []).unwrap();
    assert_eq!(repeat.frames, 0);
    assert!(repeat.complete);
}

#[test]
fn graph_deesser_internal_render_reduces_sibilance() {
    // Internal-detection DeEsser behind an audio edge, through the engine
    // graph build: an 8 kHz sibilant at -6 dB over a -20 dB threshold
    // must reduce strongly without muting.
    let frames = 8_192usize;
    let graph = PluginGraphConfig::try_new(
        vec![PluginGraphNodeConfig::try_new(1, "de_esser", sidechain_deesser_params(), 2).unwrap()],
        vec![],
    )
    .unwrap();
    let mut host = sidechain_build(&graph, 2);
    assert_eq!(host.total_latency_samples(), 0);
    let mut input = vec![0.0f32; frames * 2];
    for i in 0..frames {
        let tone = 0.5 * (std::f32::consts::TAU * 8_000.0 * i as f32 / SAMPLE_RATE as f32).sin();
        input[i * 2] = tone;
        input[i * 2 + 1] = tone;
    }
    let output = sidechain_render(&mut host, &input, frames);
    assert!(output.iter().all(|x| x.is_finite()));
    let program_rms = sidechain_channel_rms(&input, 2, 0, frames / 2);
    let out_rms = sidechain_channel_rms(&output, 2, 0, frames / 2);
    let ratio_db = 20.0 * (out_rms / program_rms).log10();
    assert!(
        ratio_db < -6.0,
        "engaged de-esser must reduce past -6 dB, got {ratio_db:.2} dB"
    );
    assert!(
        ratio_db > -20.0,
        "de-esser must not mute the program, got {ratio_db:.2} dB"
    );
}

#[test]
fn graph_deesser_latency_reflects_lookahead_and_topology() {
    // Persisted timing controls surface through the graph schedule:
    // lookahead-only, linear-phase split plus lookahead, and dry.
    for (mode, topology, lookahead_ms, expected) in [
        ("Wideband", "Minimum-Phase", 0.0f64, 0usize),
        ("Wideband", "Minimum-Phase", 2.0, 96usize),
        ("Split-Band", "Linear-Phase", 2.0, 608usize),
    ] {
        let mut params = sidechain_deesser_params();
        params["mode"] = serde_json::json!(mode);
        params["split_topology"] = serde_json::json!(topology);
        params["lookahead_ms"] = serde_json::json!(lookahead_ms);
        let graph = PluginGraphConfig::try_new(
            vec![PluginGraphNodeConfig::try_new(1, "de_esser", params, 2).unwrap()],
            vec![],
        )
        .unwrap();
        let host = sidechain_build(&graph, 2);
        assert_eq!(
            host.total_latency_samples(),
            expected,
            "{mode}/{topology} with {lookahead_ms} ms lookahead"
        );
    }
}

#[test]
fn graph_deesser_save_reload_preserves_toggle_and_topology() {
    // Internal toggle plus linear-phase topology persist through JSON and
    // rebuild to bit-identical renders.
    let mut params = sidechain_deesser_params();
    params["mode"] = serde_json::json!("Split-Band");
    params["split_topology"] = serde_json::json!("Linear-Phase");
    params["lookahead_ms"] = serde_json::json!(2.0);
    let graph = PluginGraphConfig::try_new(
        vec![
            PluginGraphNodeConfig::try_new(1, "gain", serde_json::json!({"gain_db": 0.0}), 2)
                .unwrap(),
            PluginGraphNodeConfig::try_new(2, "de_esser", params, 2).unwrap(),
        ],
        vec![PluginGraphEdgeConfig::new(1, 2)],
    )
    .unwrap();
    let saved = serde_json::to_string(&graph).unwrap();
    assert!(
        saved.contains("\"sidechain_external\":false"),
        "internal toggle must persist"
    );
    assert!(
        saved.contains("\"split_topology\":\"Linear-Phase\""),
        "topology must persist"
    );
    let reloaded: PluginGraphConfig = serde_json::from_str(&saved).unwrap();

    let frames = 4_096usize;
    let mut input = vec![0.0f32; frames * 2];
    for i in 0..frames {
        let tone = 0.5 * (std::f32::consts::TAU * 8_000.0 * i as f32 / SAMPLE_RATE as f32).sin();
        input[i * 2] = tone;
        input[i * 2 + 1] = tone;
    }
    let mut original = sidechain_build(&graph, 2);
    let mut rebuilt = sidechain_build(&reloaded, 2);
    let first = sidechain_render(&mut original, &input, frames);
    let second = sidechain_render(&mut rebuilt, &input, frames);
    assert_eq!(first, second, "save/reload must render bit-identical audio");
}

#[test]
fn graph_audio_only_config_without_kind_renders_identically() {
    // Graphs persisted before sidechain routing (no kind field) parse to
    // audio edges and render exactly like explicit-audio graphs.
    let saved = serde_json::json!({
        "nodes": [
            {"id": 1, "plugin_type": "gain", "parameters": {"gain_db": 0.0}, "input_channels": 2},
            {
                "id": 2,
                "plugin_type": "de_esser",
                "parameters": sidechain_deesser_params(),
                "input_channels": 2
            },
        ],
        "edges": [{"from_node": 1, "to_node": 2}],
    });
    let legacy: PluginGraphConfig = serde_json::from_value(saved).unwrap();
    assert_eq!(legacy.edges.len(), 1);
    assert_eq!(legacy.edges[0].kind, PluginGraphEdgeKind::Audio);

    let explicit = PluginGraphConfig::try_new(
        vec![
            PluginGraphNodeConfig::try_new(1, "gain", serde_json::json!({"gain_db": 0.0}), 2)
                .unwrap(),
            PluginGraphNodeConfig::try_new(2, "de_esser", sidechain_deesser_params(), 2).unwrap(),
        ],
        vec![PluginGraphEdgeConfig::new(1, 2)],
    )
    .unwrap();

    let frames = 2_048usize;
    let mut input = vec![0.0f32; frames * 2];
    for i in 0..frames {
        let tone = 0.5 * (std::f32::consts::TAU * 8_000.0 * i as f32 / SAMPLE_RATE as f32).sin();
        input[i * 2] = tone;
        input[i * 2 + 1] = tone;
    }
    let mut legacy_host = sidechain_build(&legacy, 2);
    let mut explicit_host = sidechain_build(&explicit, 2);
    let first = sidechain_render(&mut legacy_host, &input, frames);
    let second = sidechain_render(&mut explicit_host, &input, frames);
    assert_eq!(first, second, "legacy audio graphs must render identically");
}

#[test]
fn graph_sidechain_key_drives_deesser_reduction() {
    // External-key DeEsser through the engine graph build: the detector
    // reads the key bus only, so a hot key reduces a quiet program while
    // a silent key passes it. The program-2ch output carries a sentinel
    // guard proving no key leak past the program bus.
    let frames = 8_192usize;
    let graph = sidechain_deesser_key_graph([0, 1], [2, 3]);

    let mut hot = sidechain_build(&graph, 4);
    assert_eq!(hot.total_latency_samples(), 0);
    let hot_input = sidechain_deesser_key_input(frames, 0.05, 0.5);
    let mut hot_output = vec![-999.0f32; frames * 4];
    let done = hot
        .process(&hot_input, &mut hot_output)
        .expect("graph must process");
    assert_eq!(done, frames);
    assert!(
        hot_output[frames * 2..]
            .iter()
            .all(|&x| x.to_bits() == (-999.0f32).to_bits()),
        "program output must stay 2ch with no key leak"
    );
    let program_rms = sidechain_channel_rms(&hot_input, 4, 0, frames / 2);
    // Measure the 2ch program region only: the guard half of `hot_output`
    // still holds the -999 sentinel and must not enter the RMS window.
    let hot_rms = sidechain_channel_rms(&hot_output[..frames * 2], 2, 0, frames / 2);
    let hot_db = 20.0 * (hot_rms / program_rms).log10();
    assert!(
        hot_db < -6.0,
        "hot key must reduce the program past -6 dB, got {hot_db:.2} dB"
    );
    assert!(
        hot_db > -20.0,
        "keyed de-esser must not mute the program, got {hot_db:.2} dB"
    );
    assert!(hot_output[..frames * 2].iter().all(|x| x.is_finite()));

    let mut silent = sidechain_build(&graph, 4);
    let silent_input = sidechain_deesser_key_input(frames, 0.05, 0.0);
    let silent_output = sidechain_render(&mut silent, &silent_input, frames);
    let silent_rms = sidechain_channel_rms(&silent_output, 2, 0, frames / 2);
    let silent_db = 20.0 * (silent_rms / program_rms).log10();
    assert!(
        silent_db.abs() < 1.0,
        "silent key must pass the program within 1 dB, got {silent_db:.2} dB"
    );
}

#[test]
fn graph_sidechain_deesser_bus_ordering_is_program_then_key() {
    // Swapped selections put the hot tone on the program bus and the quiet
    // tone on the key bus: with no key energy the hot program passes. A
    // swapped or duplicated key bus would reduce it instead.
    let frames = 8_192usize;
    let graph = sidechain_deesser_key_graph([2, 3], [0, 1]);
    let mut host = sidechain_build(&graph, 4);
    let input = sidechain_deesser_key_input(frames, 0.05, 0.5);
    let output = sidechain_render(&mut host, &input, frames);
    let hot_program_rms = sidechain_channel_rms(&input, 4, 2, frames / 2);
    let out_rms = sidechain_channel_rms(&output, 2, 0, frames / 2);
    let ratio_db = 20.0 * (out_rms / hot_program_rms).log10();
    assert!(
        ratio_db.abs() < 1.0,
        "quiet key must pass the hot program within 1 dB, got {ratio_db:.2} dB"
    );
}

#[test]
fn graph_sidechain_deesser_key_config_save_reload_renders_bitwise() {
    // Persisted external-key DeEsser graphs (edge kind + sidechain toggle)
    // reload to bit-identical renders.
    let frames = 4_096usize;
    let graph = sidechain_deesser_key_graph([0, 1], [2, 3]);
    let saved = serde_json::to_string(&graph).unwrap();
    assert!(saved.contains("\"sidechain\""), "edge kind must persist");
    assert!(
        saved.contains("\"sidechain_external\":true"),
        "external toggle must persist"
    );
    let reloaded: PluginGraphConfig = serde_json::from_str(&saved).unwrap();
    assert_eq!(
        serde_json::to_value(&reloaded).unwrap(),
        serde_json::to_value(&graph).unwrap()
    );

    let input = sidechain_deesser_key_input(frames, 0.05, 0.5);
    let mut original = sidechain_build(&graph, 4);
    let mut rebuilt = sidechain_build(&reloaded, 4);
    let first = sidechain_render(&mut original, &input, frames);
    let second = sidechain_render(&mut rebuilt, &input, frames);
    assert_eq!(first, second, "save/reload must render bit-identical audio");
}
