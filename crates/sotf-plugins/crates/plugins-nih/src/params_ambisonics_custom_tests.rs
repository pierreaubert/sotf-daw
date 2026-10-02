//! Ambisonics native custom-geometry activation tests.
//!
//! Drives actual `DynamicParams` plus bridge AmbisonicsDecoder construction
//! through staging, negotiation, save, fresh restore, and bit-exact render.
//! Geometries use exact standard angles so every role, mask, and permutation
//! is pinned; wide layouts construct at the macro level and report their
//! honest format-rejection reasons. No fixtures are generated.

// Rust guideline compliant 2026-02-21

use super::ambisonics_custom::{
    AMBISONICS_CUSTOM_TARGET_INDEX, ambisonics_custom_state_field,
    decode_ambisonics_custom_field,
};
use super::{AmbisonicsCustomRestoreAttempt, DynamicParams};
use nih_plug::prelude::Params;
use nih_plug::wrapper::state::{ParamValue, PluginState};
use plugins_bridge::param_bridge::{BridgedParamInfo, ParamBridge};
use sotf_host::external_plugin::NativeAmbisonicsCustomGeometry;
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::{Plugin, ProcessContext, TailLength};
use std::collections::BTreeMap;
use std::sync::Arc;

fn ambisonics_infos() -> Vec<BridgedParamInfo> {
    let bridge = ParamBridge::new(crate::wrapper::get_param_specs("AmbisonicsDecoder"));
    (0..bridge.count())
        .filter_map(|index| bridge.info(index))
        .collect()
}

fn ambisonics_params() -> Arc<DynamicParams> {
    DynamicParams::from_infos_for_plugin("AmbisonicsDecoder", &ambisonics_infos())
}

fn speaker_json(label: &str, azimuth_deg: f32, elevation_deg: f32, is_lfe: bool) -> String {
    serde_json::json!({
        "label": label,
        "azimuth_deg": azimuth_deg,
        "elevation_deg": elevation_deg,
        "is_lfe": is_lfe,
    })
    .to_string()
}

fn geometry_json(name: &str, speakers: &[(&str, f32, f32, bool)]) -> String {
    let entries = speakers
        .iter()
        .map(|(label, azimuth, elevation, is_lfe)| {
            speaker_json(label, *azimuth, *elevation, *is_lfe)
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("{{\"name\":\"{name}\",\"speakers\":[{entries}]}}")
}

/// 12-channel geometry exactly equal to the advertised 7.1.4 map.
fn geometry_7_1_4_json() -> String {
    geometry_json(
        "custom-7.1.4",
        &[
            ("FL", 30.0, 0.0, false),
            ("FR", -30.0, 0.0, false),
            ("FC", 0.0, 0.0, false),
            ("LFE", 0.0, 0.0, true),
            ("SL", 90.0, 0.0, false),
            ("SR", -90.0, 0.0, false),
            ("BL", 150.0, 0.0, false),
            ("BR", -150.0, 0.0, false),
            ("TFL", 30.0, 45.0, false),
            ("TFR", -30.0, 45.0, false),
            ("TBL", 150.0, 45.0, false),
            ("TBR", -150.0, 45.0, false),
        ],
    )
}

/// 6-channel geometry exactly equal to the advertised 5.1 map.
fn geometry_5_1_json() -> String {
    geometry_json(
        "custom-5.1",
        &[
            ("FL", 30.0, 0.0, false),
            ("FR", -30.0, 0.0, false),
            ("FC", 0.0, 0.0, false),
            ("LFE", 0.0, 0.0, true),
            ("SL", 110.0, 0.0, false),
            ("SR", -110.0, 0.0, false),
        ],
    )
}

/// 10-channel 7.1.2-shaped geometry (same width as 5.1.4, other roles).
fn geometry_7_1_2_json() -> String {
    geometry_json(
        "custom-7.1.2",
        &[
            ("FL", 30.0, 0.0, false),
            ("FR", -30.0, 0.0, false),
            ("FC", 0.0, 0.0, false),
            ("LFE", 0.0, 0.0, true),
            ("SL", 90.0, 0.0, false),
            ("SR", -90.0, 0.0, false),
            ("BL", 150.0, 0.0, false),
            ("BR", -150.0, 0.0, false),
            ("TFL", 30.0, 45.0, false),
            ("TFR", -30.0, 45.0, false),
        ],
    )
}

fn stereo_json() -> String {
    geometry_json(
        "stereo",
        &[("FL", 30.0, 0.0, false), ("FR", -30.0, 0.0, false)],
    )
}

/// Deterministic 64-speaker sphere for macro-level wide construction.
fn wide_64_json() -> String {
    let mut speakers = Vec::with_capacity(64);
    for index in 0..64 {
        let i = index as f32;
        let y = 1.0 - 2.0 * (i + 0.5) / 64.0;
        let radius = (1.0 - y * y).sqrt();
        let phi = i * 2.399_963_f32;
        let x = radius * phi.cos();
        let z = radius * phi.sin();
        let azimuth = x.atan2(y).to_degrees();
        let elevation = z.asin().to_degrees();
        speakers.push((format!("S{index:02}"), azimuth, elevation, false));
    }
    let entries = speakers
        .iter()
        .map(|(label, azimuth, elevation, is_lfe)| {
            speaker_json(label, *azimuth, *elevation, *is_lfe)
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("{{\"name\":\"wide-64\",\"speakers\":[{entries}]}}")
}

/// Simulate a host state restore: structural ints first (as nih-plug
/// applies `params`), then the opaque fields (marker plus staging).
fn restore_custom(params: &DynamicParams, order: usize, target: usize, field: Option<&str>) {
    params.set_ambisonics_layout(order, target).unwrap();
    let mut fields = BTreeMap::new();
    if let Some(json) = field {
        fields.insert(
            ambisonics_custom_state_field().to_string(),
            json.to_string(),
        );
    }
    params.deserialize_fields(&fields);
}

fn render_deterministic_block(
    plugin: &mut Box<dyn Plugin>,
    inputs: usize,
    outputs: usize,
    sample_rate: u32,
    frames: usize,
    seed: u32,
) -> Vec<f32> {
    let mut state = seed;
    let input = (0..frames * inputs)
        .map(|_| {
            state = state
                .wrapping_mul(1_664_525)
                .wrapping_add(1_013_904_223);
            (state as f32 / u32::MAX as f32) * 0.5
        })
        .collect::<Vec<_>>();
    let mut output = vec![0.0; frames * outputs];
    let produced = plugin
        .process(&input, &mut output, &ProcessContext::new(sample_rate, frames))
        .unwrap();
    assert_eq!(produced, frames, "custom render must consume the full block");
    output
}

fn assert_finite_nonzero(output: &[f32]) {
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert!(
        output.iter().any(|sample| sample.abs() > 1.0e-6),
        "custom render is silent"
    );
}

/// Silence threshold for drain proofs, matching the nonzero threshold.
const DRAIN_QUIET_PEAK: f32 = 1.0e-6;

fn set_bool(params: &DynamicParams, id: &str, value: bool) {
    let entry = params
        .param_map
        .get(id)
        .unwrap_or_else(|| panic!("bool param {id} exists"));
    params.bool_params[entry.index].set_plain_value_for_initialization(value);
}

/// Render one deterministic signal stream through explicit partitions.
///
/// Concatenates `partitions` (each a frame count) into a single LCG
/// stream, renders every partition with its own context, and asserts
/// exact frame accounting per block. Returns the concatenated output.
fn render_signal_partitions(
    plugin: &mut Box<dyn Plugin>,
    inputs: usize,
    outputs: usize,
    sample_rate: u32,
    partitions: &[usize],
    seed: u32,
) -> Vec<f32> {
    let total: usize = partitions.iter().sum();
    let mut state = seed;
    let input = (0..total * inputs)
        .map(|_| {
            state = state
                .wrapping_mul(1_664_525)
                .wrapping_add(1_013_904_223);
            (state as f32 / u32::MAX as f32) * 0.5
        })
        .collect::<Vec<_>>();
    let mut output = Vec::with_capacity(total * outputs);
    let mut offset = 0;
    for (block, &frames) in partitions.iter().enumerate() {
        let block_in = &input[offset * inputs..(offset + frames) * inputs];
        let mut block_out = vec![0.0; frames * outputs];
        let produced = plugin
            .process(
                block_in,
                &mut block_out,
                &ProcessContext::new(sample_rate, frames),
            )
            .unwrap();
        assert_eq!(
            produced, frames,
            "partition {block} must consume its full block"
        );
        output.extend_from_slice(&block_out);
        offset += frames;
    }
    output
}

/// Full end-of-stream evidence: signal plus bounded zero-input drain.
struct StreamEof {
    signal: Vec<f32>,
    drain_blocks: usize,
    first_drain_peak: f32,
}

/// Stream parameters shared by the signal and drain phases.
struct StreamEofSpec {
    inputs: usize,
    outputs: usize,
    sample_rate: u32,
    seed: u32,
    drain_block: usize,
    drain_cap_blocks: usize,
}

/// Render a signal stream, then zero-input blocks until quiet.
///
/// Asserts the signal is finite/nonzero, every block accounts its
/// frames exactly, drain output stays finite, and silence (peak below
/// `DRAIN_QUIET_PEAK`) arrives within `drain_cap_blocks` zero blocks
/// of `drain_block` frames. Tail audio flows through zero-input
/// `process` blocks; `begin_drain`/`drain` are exercised for API
/// coverage without pinning DSP-owned drain semantics.
fn render_stream_to_eof(
    plugin: &mut Box<dyn Plugin>,
    partitions: &[usize],
    spec: &StreamEofSpec,
) -> StreamEof {
    let signal = render_signal_partitions(
        plugin,
        spec.inputs,
        spec.outputs,
        spec.sample_rate,
        partitions,
        spec.seed,
    );
    assert_finite_nonzero(&signal);
    plugin
        .begin_drain(&ProcessContext::new(spec.sample_rate, spec.drain_block))
        .expect("begin drain");
    let mut drain_blocks = 0;
    let mut first_drain_peak = 0.0;
    let mut quiet = false;
    while !quiet && drain_blocks < spec.drain_cap_blocks {
        let zeros = vec![0.0; spec.drain_block * spec.inputs];
        let mut block_out = vec![0.0; spec.drain_block * spec.outputs];
        let produced = plugin
            .process(
                &zeros,
                &mut block_out,
                &ProcessContext::new(spec.sample_rate, spec.drain_block),
            )
            .unwrap();
        assert_eq!(
            produced, spec.drain_block,
            "drain block must consume its full block"
        );
        assert!(
            block_out.iter().all(|sample| sample.is_finite()),
            "drain output stays finite"
        );
        let peak = block_out
            .iter()
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        if drain_blocks == 0 {
            first_drain_peak = peak;
        }
        drain_blocks += 1;
        quiet = peak < DRAIN_QUIET_PEAK;
    }
    assert!(
        quiet,
        "drain must settle within {} blocks",
        spec.drain_cap_blocks
    );
    let mut api_out = vec![0.0; spec.drain_block * spec.outputs];
    let _ = plugin
        .drain(
            &mut api_out,
            &ProcessContext::new(spec.sample_rate, spec.drain_block),
        )
        .expect("drain api");
    StreamEof {
        signal,
        drain_blocks,
        first_drain_peak,
    }
}

#[test]
fn custom_layout_setter_bounds() {
    let params = ambisonics_params();
    for order in 1..=7 {
        params
            .set_ambisonics_layout(order, AMBISONICS_CUSTOM_TARGET_INDEX)
            .unwrap();
        assert_eq!(
            params.value("target_layout"),
            Some(ParameterValue::Int(8))
        );
    }
    assert!(params.set_ambisonics_layout(0, 8).is_err());
    assert!(params.set_ambisonics_layout(8, 8).is_err());
    assert!(params.set_ambisonics_layout(7, 9).is_err());
    // Named bounds are unchanged.
    params.set_ambisonics_layout(7, 7).unwrap();
    assert_eq!(
        params.value("target_layout"),
        Some(ParameterValue::Int(7))
    );
}

#[test]
fn custom_constructs_and_processes_at_order_7() {
    let params = ambisonics_params();
    restore_custom(&params, 7, AMBISONICS_CUSTOM_TARGET_INDEX, Some(&geometry_7_1_4_json()));
    let mut attempt = AmbisonicsCustomRestoreAttempt::new(params.clone());
    let plugin =
        crate::params::configuration::create_plugin("AmbisonicsDecoder", 48_000, &params).unwrap();
    attempt.commit();
    assert_eq!((plugin.input_channels(), plugin.output_channels()), (64, 12));
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("target_layout")),
        Some(ParameterValue::Int(8))
    );
    let mut plugin = plugins_bridge::prepare_standalone_plugin(plugin, 64).unwrap();
    plugin.initialize(48_000).unwrap();
    let output = render_deterministic_block(&mut plugin, 64, 12, 48_000, 64, 0xA980_1C4D);
    assert_finite_nonzero(&output);
}

#[test]
fn custom_save_reload_is_bit_exact() {
    let params = ambisonics_params();
    restore_custom(&params, 2, AMBISONICS_CUSTOM_TARGET_INDEX, Some(&geometry_5_1_json()));
    let mut attempt = AmbisonicsCustomRestoreAttempt::new(params.clone());
    let plugin =
        crate::params::configuration::create_plugin("AmbisonicsDecoder", 48_000, &params).unwrap();
    attempt.commit();
    let mut plugin = plugins_bridge::prepare_standalone_plugin(plugin, 48).unwrap();
    plugin.initialize(48_000).unwrap();
    let first = render_deterministic_block(&mut plugin, 9, 6, 48_000, 48, 0x51ED_0001);

    let saved_fields = params.serialize_fields();
    let saved_json = saved_fields
        .get(ambisonics_custom_state_field())
        .expect("custom save carries the geometry field")
        .clone();
    let saved_geometry = decode_ambisonics_custom_field(&saved_json).unwrap();
    assert_eq!(saved_geometry.total_channels(), 6);

    let fresh = ambisonics_params();
    let mut values = BTreeMap::new();
    values.insert("order".to_owned(), ParamValue::I32(2));
    values.insert("target_layout".to_owned(), ParamValue::I32(8));
    let state = PluginState {
        version: "ambisonics-custom-test-state".to_owned(),
        params: values,
        fields: saved_fields,
    };
    assert!(fresh.validate_state(&state, false, false, None));
    // Fresh structural ints arrive as nih-plug would apply them.
    for (id, value) in &state.params {
        let entry = fresh.param_map.get(id).expect("restored id exists");
        let ParamValue::I32(plain) = value else {
            panic!("structural restore carries integers");
        };
        fresh.int_params[entry.index].set_plain_value_for_initialization(*plain);
    }
    fresh.deserialize_fields(&state.fields);
    let mut fresh_attempt = AmbisonicsCustomRestoreAttempt::new(fresh.clone());
    let reloaded =
        crate::params::configuration::create_plugin("AmbisonicsDecoder", 48_000, &fresh).unwrap();
    fresh_attempt.commit();
    let mut reloaded = plugins_bridge::prepare_standalone_plugin(reloaded, 48).unwrap();
    reloaded.initialize(48_000).unwrap();
    let second = render_deterministic_block(&mut reloaded, 9, 6, 48_000, 48, 0x51ED_0001);
    assert_eq!(second, first, "reload must render bit-exact output");
    let resaved = fresh.serialize_fields();
    assert_eq!(
        resaved.get(ambisonics_custom_state_field()),
        Some(&saved_json),
        "reload must save the identical geometry"
    );
}

#[test]
fn malformed_restore_fails_and_preserves_accepted() {
    let params = ambisonics_params();
    restore_custom(&params, 2, AMBISONICS_CUSTOM_TARGET_INDEX, Some(&geometry_5_1_json()));
    let mut attempt = AmbisonicsCustomRestoreAttempt::new(params.clone());
    crate::params::configuration::create_plugin("AmbisonicsDecoder", 48_000, &params).unwrap();
    attempt.commit();

    // A malformed candidate stages invalid and fails construction.
    restore_custom(&params, 2, AMBISONICS_CUSTOM_TARGET_INDEX, Some("not json"));
    let error = params
        .ambisonics_custom_layout_for_construction()
        .unwrap_err();
    assert!(error.contains("invalid"), "unexpected: {error}");
    let Err(error) =
        crate::params::configuration::create_plugin("AmbisonicsDecoder", 48_000, &params)
    else {
        panic!("malformed restore must fail construction");
    };
    assert!(error.contains("invalid"), "unexpected: {error}");

    // The accepted geometry is untouched: restaging a valid candidate
    // clears invalid and constructs with the new geometry.
    restore_custom(&params, 7, AMBISONICS_CUSTOM_TARGET_INDEX, Some(&geometry_7_1_4_json()));
    let mut retry = AmbisonicsCustomRestoreAttempt::new(params.clone());
    let plugin =
        crate::params::configuration::create_plugin("AmbisonicsDecoder", 48_000, &params).unwrap();
    retry.commit();
    assert_eq!((plugin.input_channels(), plugin.output_channels()), (64, 12));
}

#[test]
fn target_8_without_geometry_fails_construction() {
    let params = ambisonics_params();
    restore_custom(&params, 3, AMBISONICS_CUSTOM_TARGET_INDEX, None);
    let Err(error) =
        crate::params::configuration::create_plugin("AmbisonicsDecoder", 48_000, &params)
    else {
        panic!("target 8 without geometry must fail construction");
    };
    assert!(
        error.contains("requires staged custom geometry"),
        "unexpected: {error}"
    );
}

#[test]
fn named_construction_ignores_stale_staged_geometry() {
    let params = ambisonics_params();
    restore_custom(&params, 7, AMBISONICS_CUSTOM_TARGET_INDEX, Some(&geometry_7_1_4_json()));
    let mut attempt = AmbisonicsCustomRestoreAttempt::new(params.clone());
    crate::params::configuration::create_plugin("AmbisonicsDecoder", 48_000, &params).unwrap();
    attempt.commit();
    // A later named selection on the same params must not trip over the
    // retained custom carrier.
    params.set_ambisonics_layout(2, 1).unwrap();
    let plugin =
        crate::params::configuration::create_plugin("AmbisonicsDecoder", 48_000, &params).unwrap();
    assert_eq!((plugin.input_channels(), plugin.output_channels()), (9, 8));
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("target_layout")),
        Some(ParameterValue::Int(1))
    );
}

#[test]
fn negotiation_rejects_width_order_and_wire_mismatches() {
    let params = ambisonics_params();
    restore_custom(&params, 7, AMBISONICS_CUSTOM_TARGET_INDEX, Some(&geometry_7_1_4_json()));
    let error = params
        .validate_restored_ambisonics_custom_layout(7, 16, None, None)
        .unwrap_err();
    assert!(error.contains("12 channels"), "unexpected: {error}");
    assert!(error.contains("offers 16"), "unexpected: {error}");

    let clap_5_1: &[u8] = &[0, 1, 2, 3, 9, 10];
    let error = params
        .validate_restored_ambisonics_custom_layout(7, 12, Some(clap_5_1), None)
        .unwrap_err();
    assert!(
        error.contains("do not match the selected CLAP"),
        "unexpected: {error}"
    );
    let error = params
        .validate_restored_ambisonics_custom_layout(7, 12, None, Some(0x3f))
        .unwrap_err();
    assert!(
        error.contains("does not match the selected VST3"),
        "unexpected: {error}"
    );

    // A restored order conflicting with the negotiated order fails.
    // Restage first: the earlier validations consumed the restore marker.
    restore_custom(&params, 7, AMBISONICS_CUSTOM_TARGET_INDEX, Some(&geometry_7_1_4_json()));
    params.set_ambisonics_layout(1, AMBISONICS_CUSTOM_TARGET_INDEX).unwrap();
    let error = params
        .validate_restored_ambisonics_custom_layout(7, 12, None, None)
        .unwrap_err();
    assert!(
        error.contains("conflicts with negotiated order"),
        "unexpected: {error}"
    );

    // Missing geometry and non-custom targets fail with clear reasons.
    let bare = ambisonics_params();
    bare.set_ambisonics_layout(7, AMBISONICS_CUSTOM_TARGET_INDEX).unwrap();
    let error = bare
        .validate_restored_ambisonics_custom_layout(7, 12, None, None)
        .unwrap_err();
    assert!(
        error.contains("no staged custom geometry"),
        "unexpected: {error}"
    );
    bare.set_ambisonics_layout(7, 5).unwrap();
    let error = bare
        .validate_restored_ambisonics_custom_layout(7, 12, None, None)
        .unwrap_err();
    assert!(
        error.contains("requires target layout 8"),
        "unexpected: {error}"
    );
}

#[test]
fn crossrate_custom_processes_finite_nonzero() {
    for sample_rate in [44_100, 48_000, 96_000] {
        let params = ambisonics_params();
        restore_custom(
            &params,
            2,
            AMBISONICS_CUSTOM_TARGET_INDEX,
            Some(&geometry_5_1_json()),
        );
        let mut attempt = AmbisonicsCustomRestoreAttempt::new(params.clone());
        let plugin =
            crate::params::configuration::create_plugin("AmbisonicsDecoder", sample_rate, &params)
                .unwrap();
        attempt.commit();
        assert_eq!((plugin.input_channels(), plugin.output_channels()), (9, 6));
        let mut plugin = plugins_bridge::prepare_standalone_plugin(plugin, 32).unwrap();
        plugin.initialize(sample_rate).unwrap();
        let output = render_deterministic_block(&mut plugin, 9, 6, sample_rate, 32, sample_rate);
        assert_finite_nonzero(&output);
    }
}

#[test]
fn wide_64ch_constructs_at_macro_level_with_honest_format_rejections() {
    let json = wide_64_json();
    let geometry: NativeAmbisonicsCustomGeometry = serde_json::from_str(&json).unwrap();
    assert_eq!(geometry.total_channels(), 64);
    let params = ambisonics_params();
    restore_custom(&params, 7, AMBISONICS_CUSTOM_TARGET_INDEX, Some(&json));
    let mut attempt = AmbisonicsCustomRestoreAttempt::new(params.clone());
    let plugin =
        crate::params::configuration::create_plugin("AmbisonicsDecoder", 48_000, &params).unwrap();
    attempt.commit();
    assert_eq!(
        (plugin.input_channels(), plugin.output_channels()),
        (64, 64)
    );
    let mut plugin = plugins_bridge::prepare_standalone_plugin(plugin, 16).unwrap();
    plugin.initialize(48_000).unwrap();
    let output = render_deterministic_block(&mut plugin, 64, 64, 48_000, 16, 0x91DE_0064);
    assert_finite_nonzero(&output);
    // The same geometry honestly reports why no static format claims it.
    let error = geometry.matching_clap_configuration(7).unwrap_err();
    assert!(error.contains("offer 6, 8, 10 or 12"), "unexpected: {error}");
    let error = geometry.vst3_arrangement(7).unwrap_err();
    assert!(
        error.contains("matches no standard VST3 speaker"),
        "unexpected: {error}"
    );
}

#[test]
fn serialize_omits_field_for_named_includes_pending() {
    let params = ambisonics_params();
    assert!(
        !params
            .serialize_fields()
            .contains_key(ambisonics_custom_state_field()),
        "named instances must not emit a custom field"
    );
    restore_custom(&params, 7, AMBISONICS_CUSTOM_TARGET_INDEX, Some(&geometry_7_1_4_json()));
    let staged = params
        .serialize_fields()
        .get(ambisonics_custom_state_field())
        .cloned()
        .expect("staged geometry saves before commit");
    assert_eq!(decode_ambisonics_custom_field(&staged).unwrap().total_channels(), 12);
}

#[test]
fn validate_state_gates_custom_field() {
    let params = ambisonics_params();
    let blank = PluginState {
        version: "ambisonics-custom-test-state".to_owned(),
        params: BTreeMap::new(),
        fields: BTreeMap::new(),
    };
    assert!(params.validate_state(&blank, false, false, None));
    let mut staged = blank.clone();
    staged.fields.insert(
        ambisonics_custom_state_field().to_string(),
        geometry_5_1_json(),
    );
    assert!(params.validate_state(&staged, false, false, None));
    assert!(!params.validate_state(&staged, true, false, None));
    assert!(!params.validate_state(&staged, false, true, None));
    let mut malformed = blank.clone();
    malformed.fields.insert(
        ambisonics_custom_state_field().to_string(),
        "not json".to_owned(),
    );
    assert!(!params.validate_state(&malformed, false, false, None));
    let mut suffixed = blank;
    suffixed.fields.insert(
        format!("{}2", ambisonics_custom_state_field()),
        geometry_5_1_json(),
    );
    assert!(!params.validate_state(&suffixed, false, false, None));
    // Schemas without Ambisonics structure are unaffected.
    let other = DynamicParams::from_infos(&[]);
    assert!(other.validate_state(&staged, true, true, None));
}

#[test]
fn attempt_drop_discards_pending_commit_publishes() {
    let params = ambisonics_params();
    restore_custom(&params, 2, AMBISONICS_CUSTOM_TARGET_INDEX, Some(&geometry_5_1_json()));
    {
        let _attempt = AmbisonicsCustomRestoreAttempt::new(params.clone());
    }
    assert!(
        params
            .ambisonics_custom_layout_for_construction()
            .unwrap()
            .is_none(),
        "dropped attempt must discard staged geometry"
    );
    restore_custom(&params, 2, AMBISONICS_CUSTOM_TARGET_INDEX, Some(&geometry_5_1_json()));
    {
        let mut attempt = AmbisonicsCustomRestoreAttempt::new(params.clone());
        attempt.commit();
    }
    assert_eq!(
        params
            .ambisonics_custom_layout_for_construction()
            .unwrap()
            .map(|geometry| geometry.total_channels()),
        Some(6)
    );
    restore_custom(&params, 7, AMBISONICS_CUSTOM_TARGET_INDEX, Some(&geometry_7_1_4_json()));
    {
        let _attempt = AmbisonicsCustomRestoreAttempt::new(params.clone());
    }
    assert_eq!(
        params
            .ambisonics_custom_layout_for_construction()
            .unwrap()
            .map(|geometry| geometry.total_channels()),
        Some(6),
        "dropped retry must preserve the accepted geometry"
    );
}

#[test]
fn all_lfe_geometry_rejected_at_staging() {
    let params = ambisonics_params();
    let lfe_only = geometry_json("lfe-only", &[("LFE", 0.0, 0.0, true)]);
    restore_custom(&params, 1, AMBISONICS_CUSTOM_TARGET_INDEX, Some(&lfe_only));
    let error = params
        .ambisonics_custom_layout_for_construction()
        .unwrap_err();
    assert!(error.contains("invalid"), "unexpected: {error}");
    let mut state = PluginState {
        version: "ambisonics-custom-test-state".to_owned(),
        params: BTreeMap::new(),
        fields: BTreeMap::new(),
    };
    state.fields.insert(
        ambisonics_custom_state_field().to_string(),
        lfe_only,
    );
    assert!(!params.validate_state(&state, false, false, None));
}

#[test]
fn custom_permutation_matches_host_for_7_1_4() {
    let params = ambisonics_params();
    restore_custom(&params, 7, AMBISONICS_CUSTOM_TARGET_INDEX, Some(&geometry_7_1_4_json()));
    let permutation = params
        .validate_restored_ambisonics_custom_layout(7, 12, None, None)
        .unwrap();
    assert_eq!(
        permutation,
        vec![0, 1, 2, 3, 6, 7, 4, 5, 8, 9, 10, 11]
    );
}

#[test]
fn custom_negotiation_matches_host_for_7_1_2() {
    use nih_plug::context::PluginApi;
    let params = ambisonics_params();
    restore_custom(&params, 7, AMBISONICS_CUSTOM_TARGET_INDEX, Some(&geometry_7_1_2_json()));
    // CLAP order-7 slot 4 is the 7.1.2 configuration (index 40). Same
    // width as 5.1.4, other roles: the permutation must route rears
    // before sides per the canonical VST3 bus order.
    let permutation = crate::wrapper::negotiate_ambisonics_custom_layout(
        &params,
        7,
        4,
        40,
        10,
        PluginApi::Clap,
    )
    .expect("matched CLAP configuration negotiates");
    assert_eq!(permutation, vec![0, 1, 2, 3, 6, 7, 4, 5, 8, 9]);
    // The 5.1 slot reports a different map for the same API.
    assert!(
        crate::wrapper::negotiate_ambisonics_custom_layout(&params, 7, 0, 36, 10, PluginApi::Clap)
            .is_none(),
        "CLAP map mismatch must not negotiate"
    );
    // VST3 order-7 index 4 is the 7.1.2 arrangement (index 52).
    assert!(
        crate::wrapper::negotiate_ambisonics_custom_layout(&params, 7, 4, 52, 10, PluginApi::Vst3)
            .is_some(),
        "matched VST3 arrangement negotiates"
    );
    assert!(
        crate::wrapper::negotiate_ambisonics_custom_layout(&params, 7, 0, 48, 10, PluginApi::Vst3)
            .is_none(),
        "VST3 mask mismatch must not negotiate"
    );
}

#[test]
fn stereo_constructs_but_reports_format_rejection_reasons() {
    let params = ambisonics_params();
    restore_custom(&params, 1, AMBISONICS_CUSTOM_TARGET_INDEX, Some(&stereo_json()));
    let mut attempt = AmbisonicsCustomRestoreAttempt::new(params.clone());
    let plugin =
        crate::params::configuration::create_plugin("AmbisonicsDecoder", 48_000, &params).unwrap();
    attempt.commit();
    assert_eq!((plugin.input_channels(), plugin.output_channels()), (4, 2));
    let geometry: NativeAmbisonicsCustomGeometry = serde_json::from_str(&stereo_json()).unwrap();
    let error = geometry.matching_clap_configuration(1).unwrap_err();
    assert!(error.contains("offer 6, 8, 10 or 12"), "unexpected: {error}");
    let error = geometry.vst3_arrangement(1).unwrap_err();
    assert!(
        error.contains("is not one of the natively advertised"),
        "unexpected: {error}"
    );
}

#[test]
fn negotiate_selects_wire_expectations_per_format() {
    use nih_plug::context::PluginApi;
    let params = ambisonics_params();
    restore_custom(&params, 7, AMBISONICS_CUSTOM_TARGET_INDEX, Some(&geometry_7_1_4_json()));
    // CLAP order-7 slot 5 is the 7.1.4 configuration (index 41).
    let permutation = crate::wrapper::negotiate_ambisonics_custom_layout(
        &params,
        7,
        5,
        41,
        12,
        PluginApi::Clap,
    )
    .expect("matched CLAP configuration negotiates");
    assert_eq!(
        permutation,
        vec![0, 1, 2, 3, 6, 7, 4, 5, 8, 9, 10, 11]
    );
    // The 5.1 slot reports a different map for the same API.
    assert!(
        crate::wrapper::negotiate_ambisonics_custom_layout(&params, 7, 0, 36, 12, PluginApi::Clap)
            .is_none(),
        "CLAP map mismatch must not negotiate"
    );
    // VST3 order-7 index 5 is the 7.1.4 arrangement (index 53).
    assert!(
        crate::wrapper::negotiate_ambisonics_custom_layout(&params, 7, 5, 53, 12, PluginApi::Vst3)
            .is_some(),
        "matched VST3 arrangement negotiates"
    );
    assert!(
        crate::wrapper::negotiate_ambisonics_custom_layout(&params, 7, 0, 48, 12, PluginApi::Vst3)
            .is_none(),
        "VST3 mask mismatch must not negotiate"
    );
    assert!(
        crate::wrapper::negotiate_ambisonics_custom_layout(
            &params,
            7,
            5,
            53,
            12,
            PluginApi::Standalone
        )
        .is_some(),
        "standalone follows the VST3 arrangement"
    );
}

#[test]
fn default_params_carry_no_custom_state() {
    let params = ambisonics_params();
    assert!(
        params
            .ambisonics_custom_layout_for_construction()
            .unwrap()
            .is_none()
    );
    assert!(
        !params
            .serialize_fields()
            .contains_key(ambisonics_custom_state_field())
    );
    let error = params
        .validate_restored_ambisonics_custom_layout(1, 6, None, None)
        .unwrap_err();
    assert!(
        error.contains("requires target layout 8"),
        "unexpected: {error}"
    );
}

#[test]
fn serialize_omits_stale_committed_after_custom_to_named() {
    let params = ambisonics_params();
    restore_custom(&params, 7, AMBISONICS_CUSTOM_TARGET_INDEX, Some(&geometry_7_1_4_json()));
    let mut attempt = AmbisonicsCustomRestoreAttempt::new(params.clone());
    crate::params::configuration::create_plugin("AmbisonicsDecoder", 48_000, &params).unwrap();
    attempt.commit();
    assert!(
        params
            .serialize_fields()
            .contains_key(ambisonics_custom_state_field()),
        "committed custom saves carry the geometry field"
    );

    // An actual custom -> named transition through the restore path:
    // a fieldless named state constructs named and saves canonically.
    restore_custom(&params, 2, 1, None);
    let plugin =
        crate::params::configuration::create_plugin("AmbisonicsDecoder", 48_000, &params).unwrap();
    assert_eq!((plugin.input_channels(), plugin.output_channels()), (9, 8));
    assert!(
        !params
            .serialize_fields()
            .contains_key(ambisonics_custom_state_field()),
        "canonical named saves omit stale committed geometry"
    );
}

#[test]
fn fieldless_target_8_restore_fails_despite_committed() {
    let params = ambisonics_params();
    // Staged then committed good geometry with a rendered block behind it.
    restore_custom(&params, 7, AMBISONICS_CUSTOM_TARGET_INDEX, Some(&geometry_7_1_4_json()));
    let mut attempt = AmbisonicsCustomRestoreAttempt::new(params.clone());
    let plugin =
        crate::params::configuration::create_plugin("AmbisonicsDecoder", 48_000, &params).unwrap();
    attempt.commit();
    let mut plugin = plugins_bridge::prepare_standalone_plugin(plugin, 64).unwrap();
    plugin.initialize(48_000).unwrap();
    let accepted = render_deterministic_block(&mut plugin, 64, 12, 48_000, 64, 0xA980_1C4D);
    assert_finite_nonzero(&accepted);

    // A fieldless target-8 restore must fail even though accepted
    // geometry exists; it must not resurrect the committed blob.
    restore_custom(&params, 7, AMBISONICS_CUSTOM_TARGET_INDEX, None);
    let error = params
        .ambisonics_custom_layout_for_construction()
        .unwrap_err();
    assert!(
        error.contains("carries no custom geometry"),
        "unexpected: {error}"
    );
    assert!(
        error.contains("requires staged custom geometry"),
        "unexpected: {error}"
    );
    let Err(error) =
        crate::params::configuration::create_plugin("AmbisonicsDecoder", 48_000, &params)
    else {
        panic!("fieldless restore must fail construction");
    };
    assert!(
        error.contains("carries no custom geometry"),
        "unexpected: {error}"
    );
    // A rejected fieldless restore saves fieldless again: the save
    // can never heal into the old geometry on reload.
    assert!(
        !params
            .serialize_fields()
            .contains_key(ambisonics_custom_state_field()),
        "fieldless restore must save fieldless"
    );

    // The accepted carrier is preserved: stage a fresh candidate,
    // drop its attempt, and the accepted geometry still resolves.
    restore_custom(&params, 2, AMBISONICS_CUSTOM_TARGET_INDEX, Some(&geometry_5_1_json()));
    {
        let _attempt = AmbisonicsCustomRestoreAttempt::new(params.clone());
    }
    assert_eq!(
        params
            .ambisonics_custom_layout_for_construction()
            .unwrap()
            .map(|geometry| geometry.total_channels()),
        Some(12),
        "dropped retry must preserve the accepted geometry"
    );

    // Failed-restore -> valid-custom retry constructs and renders.
    restore_custom(&params, 2, AMBISONICS_CUSTOM_TARGET_INDEX, Some(&geometry_5_1_json()));
    let mut retry = AmbisonicsCustomRestoreAttempt::new(params.clone());
    let plugin =
        crate::params::configuration::create_plugin("AmbisonicsDecoder", 48_000, &params).unwrap();
    retry.commit();
    assert_eq!((plugin.input_channels(), plugin.output_channels()), (9, 6));
    let mut plugin = plugins_bridge::prepare_standalone_plugin(plugin, 48).unwrap();
    plugin.initialize(48_000).unwrap();
    let output = render_deterministic_block(&mut plugin, 9, 6, 48_000, 48, 0x51ED_0001);
    assert_finite_nonzero(&output);
}

#[test]
fn malformed_then_fieldless_named_recovers_cleanly() {
    let params = ambisonics_params();
    restore_custom(&params, 2, AMBISONICS_CUSTOM_TARGET_INDEX, Some("not json"));
    assert!(
        params
            .ambisonics_custom_layout_for_construction()
            .is_err()
    );

    // A valid fieldless named state clears invalid and constructs named.
    restore_custom(&params, 2, 1, None);
    let plugin =
        crate::params::configuration::create_plugin("AmbisonicsDecoder", 48_000, &params).unwrap();
    assert_eq!((plugin.input_channels(), plugin.output_channels()), (9, 8));
    assert!(
        !params
            .serialize_fields()
            .contains_key(ambisonics_custom_state_field()),
        "named saves omit the carrier after malformed history"
    );

    // A fresh custom candidate still stages and constructs afterward.
    restore_custom(&params, 7, AMBISONICS_CUSTOM_TARGET_INDEX, Some(&geometry_7_1_4_json()));
    let mut retry = AmbisonicsCustomRestoreAttempt::new(params.clone());
    let plugin =
        crate::params::configuration::create_plugin("AmbisonicsDecoder", 48_000, &params).unwrap();
    retry.commit();
    assert_eq!((plugin.input_channels(), plugin.output_channels()), (64, 12));
}

#[test]
fn single_band_custom_reaches_exact_zero_tail() {
    let params = ambisonics_params();
    restore_custom(&params, 7, AMBISONICS_CUSTOM_TARGET_INDEX, Some(&geometry_7_1_4_json()));
    let mut attempt = AmbisonicsCustomRestoreAttempt::new(params.clone());
    let plugin =
        crate::params::configuration::create_plugin("AmbisonicsDecoder", 48_000, &params).unwrap();
    attempt.commit();
    assert_eq!(plugin.latency_samples(), 0);
    assert_eq!(plugin.tail_length(), TailLength::Finite(0));
    let mut plugin = plugins_bridge::prepare_standalone_plugin(plugin, 64).unwrap();
    plugin.initialize(48_000).unwrap();
    let stream = render_stream_to_eof(
        &mut plugin,
        &[31, 1, 32],
        &StreamEofSpec {
            inputs: 64,
            outputs: 12,
            sample_rate: 48_000,
            seed: 0xE0F0_0001,
            drain_block: 64,
            drain_cap_blocks: 4,
        },
    );
    assert_eq!(
        stream.drain_blocks, 1,
        "memoryless matrix settles on the first zero block"
    );
    assert_eq!(
        stream.first_drain_peak, 0.0,
        "single-band tail is exactly zero"
    );
    assert_eq!(stream.signal.len(), 64 * 12);
}

#[test]
fn dual_band_custom_drains_within_bound() {
    let params = ambisonics_params();
    set_bool(&params, "dual_band", true);
    restore_custom(&params, 7, AMBISONICS_CUSTOM_TARGET_INDEX, Some(&geometry_7_1_4_json()));
    let mut attempt = AmbisonicsCustomRestoreAttempt::new(params.clone());
    let plugin =
        crate::params::configuration::create_plugin("AmbisonicsDecoder", 48_000, &params).unwrap();
    attempt.commit();
    assert_eq!(
        plugin.latency_samples(),
        0,
        "dual-band reports no fixed host latency"
    );
    assert_eq!(
        plugin.tail_length(),
        TailLength::Unknown,
        "LR4 crossover keeps recursive history"
    );
    let mut plugin = plugins_bridge::prepare_standalone_plugin(plugin, 64).unwrap();
    plugin.initialize(48_000).unwrap();
    let stream = render_stream_to_eof(
        &mut plugin,
        &[17, 47],
        &StreamEofSpec {
            inputs: 64,
            outputs: 12,
            sample_rate: 48_000,
            seed: 0xE0F0_0002,
            drain_block: 64,
            drain_cap_blocks: 32,
        },
    );
    assert!(
        stream.first_drain_peak > DRAIN_QUIET_PEAK,
        "dual-band tail must exist (first drain peak {})",
        stream.first_drain_peak
    );
    assert!(
        stream.drain_blocks <= 32,
        "drain must settle within its bound"
    );
}

fn partitioned_render(dual_band: bool, partitions: &[usize], seed: u32) -> Vec<f32> {
    let params = ambisonics_params();
    set_bool(&params, "dual_band", dual_band);
    restore_custom(&params, 2, AMBISONICS_CUSTOM_TARGET_INDEX, Some(&geometry_5_1_json()));
    let mut attempt = AmbisonicsCustomRestoreAttempt::new(params.clone());
    let plugin =
        crate::params::configuration::create_plugin("AmbisonicsDecoder", 48_000, &params).unwrap();
    attempt.commit();
    let mut plugin = plugins_bridge::prepare_standalone_plugin(plugin, 64).unwrap();
    plugin.initialize(48_000).unwrap();
    render_signal_partitions(&mut plugin, 9, 6, 48_000, partitions, seed)
}

fn lcg_partitions(total: usize, seed: u32) -> Vec<usize> {
    let mut state = seed;
    let mut sizes = Vec::new();
    let mut remaining = total;
    while remaining > 0 {
        state = state
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        let size = (1 + (state as usize % remaining.min(17))).min(remaining);
        sizes.push(size);
        remaining -= size;
    }
    sizes
}

#[test]
fn custom_render_is_partition_invariant() {
    for dual_band in [false, true] {
        let reference = partitioned_render(dual_band, &[64], 0x9A27_0001);
        let odd = partitioned_render(dual_band, &[1, 7, 31, 13, 12], 0x9A27_0001);
        let random = partitioned_render(
            dual_band,
            &lcg_partitions(64, 0x9A27_0002),
            0x9A27_0001,
        );
        let replay = partitioned_render(dual_band, &[1, 7, 31, 13, 12], 0x9A27_0001);
        assert_eq!(
            replay, odd,
            "dual_band={dual_band}: identical partitions replay bit-exact"
        );
        if dual_band {
            for (index, (actual, expected)) in odd.iter().zip(&reference).enumerate() {
                assert!(
                    (actual - expected).abs() <= 1.0e-6,
                    "dual-band odd-partition drift at {index}: {actual} vs {expected}"
                );
            }
            for (index, (actual, expected)) in random.iter().zip(&reference).enumerate() {
                assert!(
                    (actual - expected).abs() <= 1.0e-6,
                    "dual-band random-partition drift at {index}: {actual} vs {expected}"
                );
            }
        } else {
            assert_eq!(odd, reference, "single-band odd partitions render bit-exact");
            assert_eq!(
                random, reference,
                "single-band random partitions render bit-exact"
            );
        }
    }
}

#[test]
fn missing_survives_attempt_drop_and_refuses_until_valid_stage() {
    let params = ambisonics_params();
    // Commit accepted good geometry through the real construction path.
    restore_custom(&params, 7, AMBISONICS_CUSTOM_TARGET_INDEX, Some(&geometry_7_1_4_json()));
    let mut attempt = AmbisonicsCustomRestoreAttempt::new(params.clone());
    crate::params::configuration::create_plugin("AmbisonicsDecoder", 48_000, &params).unwrap();
    attempt.commit();
    // A fieldless target-8 restore records missing-field intent.
    restore_custom(&params, 7, AMBISONICS_CUSTOM_TARGET_INDEX, None);
    let error = params
        .ambisonics_custom_layout_for_construction()
        .unwrap_err();
    assert!(
        error.contains("carries no custom geometry"),
        "unexpected: {error}"
    );
    // Abandon an actual restore attempt while missing=true: discard
    // must preserve the refusal rather than clear it.
    {
        let _attempt = AmbisonicsCustomRestoreAttempt::new(params.clone());
    }
    let error = params
        .ambisonics_custom_layout_for_construction()
        .unwrap_err();
    assert!(
        error.contains("carries no custom geometry"),
        "unexpected: {error}"
    );
    assert!(
        !params
            .serialize_fields()
            .contains_key(ambisonics_custom_state_field()),
        "serialize stays fieldless after the abandoned attempt"
    );
    // Valid-stage retry still constructs with the staged geometry.
    restore_custom(&params, 7, AMBISONICS_CUSTOM_TARGET_INDEX, Some(&geometry_7_1_4_json()));
    let mut retry = AmbisonicsCustomRestoreAttempt::new(params.clone());
    let plugin =
        crate::params::configuration::create_plugin("AmbisonicsDecoder", 48_000, &params).unwrap();
    retry.commit();
    assert_eq!((plugin.input_channels(), plugin.output_channels()), (64, 12));
}
