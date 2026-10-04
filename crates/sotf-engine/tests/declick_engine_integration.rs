//! Declick engine integration: typed settings to audible EOF.
//!
//! Proves the six appended declick controls survive the whole engine route:
//! legacy/new typed-settings serde, save/reload round trip, the registered
//! converter, factory construction, and audible processing with complete
//! finite EOF through a factory plugin, a [`DawHost`] chain, and an
//! [`EmbeddedAudioEngine`]. Cleaned and residual outputs are checked against
//! an independently shifted clean reference (manual prepend/truncate, never
//! the implementation's own bypass run), with exact length, latency, and
//! channel geometry. Rejected automation preserves populated history
//! exactly, structural controls require chain rebuilds, and legacy
//! three-key settings render bit-identically to explicit neutral settings.
//!
//! Bounds reuse the frozen declick contract: 5%-of-amplitude repair error
//! at click frames, 0.05 absolute damage outside the widened repair
//! footprint, recall and precision at least 0.95 through the residual tap,
//! exact leading silence, 1e-5 residual regrouping, and click-free controls
//! (legacy bit-exact, multiband within 1e-5). Worst-case measurements print
//! with the `[declick-engine]` tag for the coordinator record
//! (`cargo test -- --nocapture`); assertions are exact or pre-fixed, never
//! fitted here.

use sotf_audio::{
    EmbeddedAudioEngine, EngineConfig, PluginConfig, PluginSettings, PluginType,
};
use sotf_plugins::{DawHost, ParameterId, ParameterValue, ProcessContext, create_plugin};

const RATE: u32 = 48_000;
const CHANNELS: usize = 2;
const FRAMES: usize = 1024;
const WIDTH: usize = 3;
const LATENCY: usize = 8 + WIDTH;
const LEGACY_LATENCY: usize = 8;
const CLICK_AMP: f32 = 3.0;
const HOT_RESIDUAL: f32 = 0.5;
/// First settled input frame for value assertions, mirroring the 32-frame
/// settled scope of `modes.rs` DECLICK-A1 (startup detector context).
const SETTLED_FROM: usize = 32;

/// Non-default typed settings exercising all six appended controls.
fn nondefault_settings_json(audition: bool) -> serde_json::Value {
    serde_json::json!({
        "Declick": {
            "enabled": true,
            "sensitivity": 2.0,
            "link_channels": true,
            "mode": 1,
            "bands": 2,
            "crossover_hz": 8000.0,
            "frequency_skew": 0.5,
            "repair_width": WIDTH,
            "audition_residual": audition,
        }
    })
}

/// Fixture tone: 0.25-amplitude sine at `freq` Hz.
fn tone(frame: usize, freq: f32) -> f32 {
    (frame as f32 * freq / RATE as f32 * std::f32::consts::TAU).sin() * 0.25
}

/// Corrupted stereo, per-channel clean references, and click flags.
///
/// Left carries a 440 Hz tone with its own click plan; right carries 660 Hz
/// with an offset plan, so channel swaps or cross-talk fail loudly.
fn stereo_fixture() -> (Vec<f32>, Vec<f32>, Vec<Vec<bool>>) {
    let mut corrupted = vec![0.0; FRAMES * CHANNELS];
    let mut clean = vec![0.0; FRAMES * CHANNELS];
    let mut is_click = vec![vec![false; FRAMES]; CHANNELS];
    let plans: [Vec<(usize, usize, f32)>; CHANNELS] = [
        vec![(100, 1, 1.0), (300, 3, -1.0), (700, 1, 1.0)],
        vec![(150, 1, -1.0), (500, 3, 1.0), (900, 1, -1.0)],
    ];
    for ch in 0..CHANNELS {
        let freq = if ch == 0 { 440.0 } else { 660.0 };
        for frame in 0..FRAMES {
            let sample = tone(frame, freq);
            corrupted[frame * CHANNELS + ch] = sample;
            clean[frame * CHANNELS + ch] = sample;
        }
        for &(start, width, sign) in &plans[ch] {
            for offset in 0..width {
                corrupted[(start + offset) * CHANNELS + ch] += sign * CLICK_AMP;
                is_click[ch][start + offset] = true;
            }
        }
    }
    (corrupted, clean, is_click)
}

/// Independent oracle: `signal` delayed by `latency` frames over the full
/// rendered span (input frames plus latency tail), zero-padded past the end.
fn manual_delayed(signal: &[f32], channels: usize, latency: usize) -> Vec<f32> {
    let frames = signal.len() / channels;
    let mut delayed = vec![0.0; (frames + latency) * channels];
    for frame in 0..frames {
        for ch in 0..channels {
            delayed[(frame + latency) * channels + ch] = signal[frame * channels + ch];
        }
    }
    delayed
}

fn report(label: &str, worst: f32) {
    eprintln!("[declick-engine] {label} worst={worst:.6}");
}

/// Check a full cleaned render against the frozen accuracy contract.
///
/// Mirrors `modes.rs` DECLICK-A1 at engine scope: repair error within 5% of
/// click amplitude at click frames, absolute damage below 0.05 on settled
/// frames outside the repair footprint, exact leading silence. Output frame
/// `F` carries input frame `F - latency`; the first `latency` frames are
/// exact zeros from zero-filled delay lines. The guard spans each click
/// frame ± (`WIDTH` + 2): widened emission is structural (± `WIDTH`) and
/// ±2 carries the detection slop from the accepted width-0 suite, so
/// multiband rounding and intentional skirt repairs never count as damage.
/// Reports worst repair error and worst damage for the coordinator record.
fn check_cleaned(
    label: &str,
    output: &[f32],
    clean: &[f32],
    is_click: &[Vec<bool>],
    latency: usize,
) {
    assert_eq!(output.len(), (FRAMES + latency) * CHANNELS, "{label}: length");
    assert!(
        output.iter().all(|sample| sample.is_finite()),
        "{label}: finite"
    );
    for (index, sample) in output.iter().take(latency * CHANNELS).enumerate() {
        assert_eq!(*sample, 0.0, "{label}: leading silence sample={index}");
    }
    let clean_delayed = manual_delayed(clean, CHANNELS, latency);
    let mut worst_repair = 0.0_f32;
    let mut worst_damage = 0.0_f32;
    for frame in latency..FRAMES + latency {
        let input = frame - latency;
        if input < SETTLED_FROM {
            continue;
        }
        for ch in 0..CHANNELS {
            let clicks = &is_click[ch];
            let actual = output[frame * CHANNELS + ch];
            let expected = clean_delayed[frame * CHANNELS + ch];
            if clicks[input] {
                let error = (actual - expected).abs();
                worst_repair = worst_repair.max(error);
                assert!(
                    error < CLICK_AMP * 0.05,
                    "{label}: repair input={input} ch={ch} error={error}"
                );
            } else if !near_click(clicks, input) {
                let damage = (actual - expected).abs();
                worst_damage = worst_damage.max(damage);
                assert!(
                    damage < 0.05,
                    "{label}: damage input={input} ch={ch} damage={damage}"
                );
            }
        }
    }
    report(&format!("{label} repair-error"), worst_repair);
    report(&format!("{label} damage"), worst_damage);
}

/// Repair-footprint guard: click frames ± widened emission (`WIDTH`) plus
/// the ±2 detection slop carried from the accepted width-0 suite.
fn near_click(is_click: &[bool], frame: usize) -> bool {
    const GUARD: usize = WIDTH + 2;
    let start = frame.saturating_sub(GUARD);
    let end = (frame + GUARD + 1).min(is_click.len());
    is_click[start..end].contains(&true)
}

/// Check a residual render: `repaired + residual` must regroup to the
/// independently delayed corrupted input within float regrouping, and the
/// residual must run hot at click frames (recall and precision at least
/// 0.95, mirroring `modes.rs` DECLICK-A1 over settled frames; the widened
/// footprint is excluded from false-hot counting because intentional skirt
/// repairs run hot by design).
fn check_residual(
    label: &str,
    residual: &[f32],
    repaired: &[f32],
    corrupted: &[f32],
    is_click: &[Vec<bool>],
    latency: usize,
) {
    assert_eq!(residual.len(), repaired.len(), "{label}: length");
    assert_eq!(
        residual.len(),
        (FRAMES + latency) * CHANNELS,
        "{label}: length"
    );
    let dry = manual_delayed(corrupted, CHANNELS, latency);
    let mut worst = 0.0_f32;
    for i in 0..dry.len() {
        worst = worst.max((repaired[i] + residual[i] - dry[i]).abs());
    }
    assert!(worst < 1.0e-5, "{label}: regroup drift {worst}");
    report(&format!("{label} regroup"), worst);
    let mut true_hot = 0;
    let mut false_hot = 0;
    let mut missed = 0;
    for frame in SETTLED_FROM..FRAMES {
        for ch in 0..CHANNELS {
            let clicks = &is_click[ch];
            if !clicks[frame] && near_click(clicks, frame) {
                continue;
            }
            let hot = residual[(frame + latency) * CHANNELS + ch].abs() > HOT_RESIDUAL;
            match (clicks[frame], hot) {
                (true, true) => true_hot += 1,
                (false, true) => false_hot += 1,
                (true, false) => missed += 1,
                (false, false) => {}
            }
        }
    }
    let total = true_hot + missed;
    let recall = true_hot as f32 / total as f32;
    let precision = true_hot as f32 / (true_hot + false_hot).max(1) as f32;
    assert!(recall >= 0.95, "{label}: recall={recall} ({true_hot}/{total})");
    assert!(
        precision >= 0.95,
        "{label}: precision={precision} false_hot={false_hot}"
    );
    eprintln!("[declick-engine] {label} recall={recall:.4} precision={precision:.4}");
}

#[test]
fn declick_typed_settings_converter_and_factory_carry_all_nine_controls() {
    // Legacy three-key save shape keeps neutral new-mode defaults.
    let legacy: PluginSettings = serde_json::from_value(serde_json::json!({
        "Declick": { "enabled": true, "sensitivity": 7.0, "link_channels": false }
    }))
    .unwrap();
    match &legacy {
        PluginSettings::Declick {
            enabled,
            sensitivity,
            link_channels,
            mode,
            bands,
            crossover_hz,
            frequency_skew,
            repair_width,
            audition_residual,
        } => {
            assert_eq!((*enabled, *sensitivity, *link_channels), (true, 7.0, false));
            assert_eq!((*mode, *bands, *repair_width), (0, 0, 0));
            assert_eq!(*crossover_hz, 4000.0);
            assert_eq!(*frequency_skew, 0.0);
            assert!(!*audition_residual);
        }
        other => panic!("expected Declick settings, got {other:?}"),
    }

    // Non-default save shape converts key-for-key with exact values.
    let settings: PluginSettings =
        serde_json::from_value(nondefault_settings_json(false)).unwrap();
    let config = settings.to_plugin_config(RATE as f64);
    assert_eq!(config.plugin_type, "declick");
    for (key, expected) in [
        ("enabled", serde_json::json!(true)),
        ("sensitivity", serde_json::json!(2.0)),
        ("link_channels", serde_json::json!(true)),
        ("mode", serde_json::json!(1)),
        ("bands", serde_json::json!(2)),
        ("crossover_hz", serde_json::json!(8000.0)),
        ("frequency_skew", serde_json::json!(0.5)),
        ("repair_width", serde_json::json!(WIDTH)),
        ("audition_residual", serde_json::json!(false)),
    ] {
        assert_eq!(config.parameters[key], expected, "converter key {key}");
    }

    // Save/reload round trip preserves the converter input bit-for-bit.
    let saved = serde_json::to_value(&settings).unwrap();
    let reloaded: PluginSettings = serde_json::from_value(saved).unwrap();
    assert_eq!(
        reloaded.to_plugin_config(RATE as f64).parameters,
        config.parameters
    );

    // The UI accessor path converges to the identical converter JSON.
    let mut via_accessors = PluginSettings::default_for(&PluginType::Declick).unwrap();
    via_accessors.set_param_value(1, 2.0);
    via_accessors.set_param_value(3, 1.0);
    via_accessors.set_param_value(4, 2.0);
    via_accessors.set_param_value(5, 8000.0);
    via_accessors.set_param_value(6, 0.5);
    via_accessors.set_param_value(7, WIDTH as f64);
    assert_eq!(
        via_accessors.to_plugin_config(RATE as f64).parameters,
        config.parameters
    );

    // Factory construction exposes every control with its value and type.
    let plugin = create_plugin(&config.plugin_type, &config.parameters, CHANNELS, RATE).unwrap();
    for (key, expected) in [
        ("enabled", ParameterValue::Bool(true)),
        ("sensitivity", ParameterValue::Float(2.0)),
        ("link_channels", ParameterValue::Bool(true)),
        ("mode", ParameterValue::Int(1)),
        ("bands", ParameterValue::Int(2)),
        ("crossover_hz", ParameterValue::Float(8000.0)),
        ("frequency_skew", ParameterValue::Float(0.5)),
        ("repair_width", ParameterValue::Int(WIDTH as i32)),
        ("audition_residual", ParameterValue::Bool(false)),
    ] {
        assert_eq!(
            plugin.get_parameter(&ParameterId::from(key)),
            Some(expected),
            "factory getter {key}"
        );
    }
    assert_eq!(plugin.latency_samples(), LATENCY);

    // A mistyped structural key fails closed; the accepted config rebuilds.
    let mut bad = config.parameters.clone();
    bad["mode"] = serde_json::json!("Bogus");
    assert!(create_plugin(&config.plugin_type, &bad, CHANNELS, RATE).is_err());
    let rebuilt =
        create_plugin(&config.plugin_type, &config.parameters, CHANNELS, RATE).unwrap();
    assert_eq!(
        rebuilt.get_parameter(&ParameterId::from("mode")),
        Some(ParameterValue::Int(1))
    );
}

/// Render `input` through a factory-built plugin to EOF: chunked process,
/// latency-tail drain, and post-drain exhaustion, all NaN-guarded.
fn render_factory_plugin(
    plugin_type: &str,
    parameters: &serde_json::Value,
    input: &[f32],
    latency: usize,
) -> Vec<f32> {
    let mut plugin = create_plugin(plugin_type, parameters, CHANNELS, RATE).unwrap();
    plugin.initialize(f64::from(RATE)).unwrap();
    assert_eq!(plugin.latency_samples(), latency);
    let mut output = Vec::new();
    for chunk in input.chunks(256 * CHANNELS) {
        let frames = chunk.len() / CHANNELS;
        let mut block = vec![f32::NAN; chunk.len()];
        assert_eq!(
            plugin
                .process(chunk, &mut block, &ProcessContext::new(RATE, frames))
                .unwrap(),
            frames
        );
        output.extend_from_slice(&block);
    }
    assert!(plugin.drain_output_frames_max() >= latency);
    loop {
        let mut tail = vec![f32::NAN; latency * CHANNELS];
        let status = plugin
            .drain(&mut tail, &ProcessContext::new(RATE, latency))
            .unwrap();
        assert!(status.frames <= latency);
        output.extend_from_slice(&tail[..status.frames * CHANNELS]);
        if status.complete {
            break;
        }
    }
    assert!(
        plugin
            .drain(
                &mut vec![0.0; latency * CHANNELS],
                &ProcessContext::new(RATE, latency)
            )
            .unwrap()
            .complete
    );
    output
}

#[test]
fn declick_factory_audio_and_eof_match_manual_oracle() {
    let settings: PluginSettings =
        serde_json::from_value(nondefault_settings_json(false)).unwrap();
    let config = settings.to_plugin_config(RATE as f64);
    let (corrupted, clean, is_click) = stereo_fixture();

    // Each arm derives its expected latency from its known typed controls
    // (8 + repair_width); the helper verifies the instance reports it.
    let render = |parameters: &serde_json::Value, latency: usize| {
        render_factory_plugin(&config.plugin_type, parameters, &corrupted, latency)
    };

    // Non-default new-mode arm: repair_width 3 -> 11.
    let repaired = render(&config.parameters, LATENCY);
    check_cleaned("factory cleaned", &repaired, &clean, &is_click, LATENCY);

    // Residual arm shares the non-default width (audition flag only).
    let residual_settings: PluginSettings =
        serde_json::from_value(nondefault_settings_json(true)).unwrap();
    let residual = render(
        &residual_settings.to_plugin_config(RATE as f64).parameters,
        LATENCY,
    );
    check_residual(
        "factory residual",
        &residual,
        &repaired,
        &corrupted,
        &is_click,
        LATENCY,
    );

    // Legacy three-key settings render bit-identically to explicit neutral
    // settings: the settings path preserves old default bit identity.
    let legacy: PluginSettings = serde_json::from_value(serde_json::json!({
        "Declick": { "enabled": true, "sensitivity": 2.0, "link_channels": true }
    }))
    .unwrap();
    let neutral: PluginSettings = serde_json::from_value(serde_json::json!({
        "Declick": {
            "enabled": true, "sensitivity": 2.0, "link_channels": true,
            "mode": 0, "bands": 0, "crossover_hz": 4000.0,
            "frequency_skew": 0.0, "repair_width": 0, "audition_residual": false,
        }
    }))
    .unwrap();
    // Legacy three-key and explicit neutral arms: repair_width 0 -> 8.
    let legacy_rendered = render(
        &legacy.to_plugin_config(RATE as f64).parameters,
        LEGACY_LATENCY,
    );
    let neutral_rendered = render(
        &neutral.to_plugin_config(RATE as f64).parameters,
        LEGACY_LATENCY,
    );
    assert_eq!(legacy_rendered.len(), (FRAMES + LEGACY_LATENCY) * CHANNELS);
    assert_eq!(legacy_rendered, neutral_rendered);
}

#[test]
fn declick_daw_host_chain_renders_and_drains_with_manual_oracle() {
    let settings: PluginSettings =
        serde_json::from_value(nondefault_settings_json(false)).unwrap();
    let config = settings.to_plugin_config(RATE as f64);
    let (corrupted, clean, is_click) = stereo_fixture();

    let render = |parameters: &serde_json::Value| {
        let mut host = DawHost::new(CHANNELS, RATE);
        host.add_plugin(create_plugin(&config.plugin_type, parameters, CHANNELS, RATE).unwrap())
            .unwrap();
        host.build().unwrap();
        assert_eq!(host.total_latency_samples(), LATENCY);
        let mut output = Vec::new();
        for chunk in corrupted.chunks(300 * CHANNELS) {
            let mut block = vec![f32::NAN; chunk.len()];
            assert_eq!(
                host.process(chunk, &mut block).unwrap(),
                chunk.len() / CHANNELS
            );
            output.extend_from_slice(&block);
        }
        let capacity = host.drain_output_frames_max().max(1);
        for _ in 0..4096 {
            let mut tail = vec![f32::NAN; capacity * CHANNELS];
            let status = host.drain(&mut tail).unwrap();
            assert!(status.frames <= capacity);
            output.extend_from_slice(&tail[..status.frames * CHANNELS]);
            if status.complete {
                assert_eq!(host.drain(&mut tail).unwrap().frames, 0);
                return output;
            }
        }
        panic!("declick host chain did not finish draining");
    };

    let repaired = render(&config.parameters);
    check_cleaned("host cleaned", &repaired, &clean, &is_click, LATENCY);

    let residual_settings: PluginSettings =
        serde_json::from_value(nondefault_settings_json(true)).unwrap();
    let residual = render(&residual_settings.to_plugin_config(RATE as f64).parameters);
    check_residual(
        "host residual",
        &residual,
        &repaired,
        &corrupted,
        &is_click,
        LATENCY,
    );

    // Host reset reproduces the render bit-exactly.
    let mut host = DawHost::new(CHANNELS, RATE);
    host.add_plugin(create_plugin(&config.plugin_type, &config.parameters, CHANNELS, RATE).unwrap())
        .unwrap();
    host.build().unwrap();
    let mut first = Vec::new();
    for chunk in corrupted.chunks(300 * CHANNELS) {
        let mut block = vec![0.0; chunk.len()];
        host.process(chunk, &mut block).unwrap();
        first.extend_from_slice(&block);
    }
    host.reset();
    let mut second = Vec::new();
    for chunk in corrupted.chunks(300 * CHANNELS) {
        let mut block = vec![0.0; chunk.len()];
        host.process(chunk, &mut block).unwrap();
        second.extend_from_slice(&block);
    }
    assert_eq!(first, second);
}

#[test]
fn declick_embedded_engine_renders_tail_and_reports_geometry() {
    let settings: PluginSettings =
        serde_json::from_value(nondefault_settings_json(false)).unwrap();
    let config = settings.to_plugin_config(RATE as f64);
    let engine_config = EngineConfig {
        frame_size: 512,
        input_channels: CHANNELS,
        output_channels: CHANNELS,
        output_sample_rate: RATE,
        plugins: vec![PluginConfig::new("declick", config.parameters.clone())],
        ..EngineConfig::default()
    };
    let (mut engine, diagnostics) = EmbeddedAudioEngine::new(&engine_config).unwrap();
    assert!(diagnostics.is_empty());
    assert_eq!(engine.latency_samples(), LATENCY);
    assert_eq!(engine.output_channels(), CHANNELS);
    assert_eq!(engine.max_block_frames(), 512);

    let (corrupted, clean, is_click) = stereo_fixture();
    // The embedded engine has no drain call: the finite tail renders by
    // feeding exactly `LATENCY` zero frames after the programme.
    let mut stream = corrupted.clone();
    stream.extend(std::iter::repeat_n(0.0, LATENCY * CHANNELS));
    let mut output = Vec::new();
    let mut position = 0_u64;
    for chunk in stream.chunks(512 * CHANNELS) {
        let mut block = vec![f32::NAN; chunk.len()];
        let frames = engine.process_at(position, chunk, &mut block).unwrap();
        assert_eq!(frames, chunk.len() / CHANNELS);
        position += frames as u64;
        output.extend_from_slice(&block);
    }
    check_cleaned("embedded cleaned", &output, &clean, &is_click, LATENCY);
}

#[test]
fn declick_engine_rejections_preserve_history_and_structural_params_rebuild() {
    // Legacy engine with correlated populated history.
    let legacy: PluginSettings = serde_json::from_value(serde_json::json!({
        "Declick": { "enabled": true, "sensitivity": 2.0, "link_channels": true }
    }))
    .unwrap();
    let legacy_config = legacy.to_plugin_config(RATE as f64);
    let engine_config = EngineConfig {
        frame_size: 512,
        input_channels: CHANNELS,
        output_channels: CHANNELS,
        output_sample_rate: RATE,
        plugins: vec![PluginConfig::new(
            "declick",
            legacy_config.parameters.clone(),
        )],
        ..EngineConfig::default()
    };
    let (corrupted, _, _) = stereo_fixture();
    let render_prefix = |engine: &mut EmbeddedAudioEngine, position: &mut u64, frames: usize| {
        let start = *position as usize * CHANNELS;
        let chunk = &corrupted[start..start + frames * CHANNELS];
        let mut block = vec![0.0; chunk.len()];
        assert_eq!(engine.process_at(*position, chunk, &mut block).unwrap(), frames);
        *position += frames as u64;
        block
    };

    let (mut engine, _) = EmbeddedAudioEngine::new(&engine_config).unwrap();
    let (mut reference, _) = EmbeddedAudioEngine::new(&engine_config).unwrap();
    let mut position = 0_u64;
    let mut ref_position = 0_u64;
    // The 600-frame prefix exceeds the 512-frame host block limit, so it
    // renders as two back-to-back blocks on the same stream; the
    // populated-history comparison still spans the whole 600 frames.
    let mut output = render_prefix(&mut engine, &mut position, 512);
    output.extend(render_prefix(&mut engine, &mut position, 88));
    let mut expected = render_prefix(&mut reference, &mut ref_position, 512);
    expected.extend(render_prefix(&mut reference, &mut ref_position, 88));
    assert_eq!(output, expected);

    // Unknown automation fails; every structural control fails closed with
    // the documented rebuild contract instead of mutating the live chain.
    assert!(
        engine
            .set_plugin_parameter_at(0, "bogus_param", ParameterValue::Float(1.0), 0)
            .is_err()
    );
    for (key, value) in [
        ("mode", ParameterValue::Int(1)),
        ("bands", ParameterValue::Int(2)),
        ("crossover_hz", ParameterValue::Float(8000.0)),
        ("repair_width", ParameterValue::Int(3)),
    ] {
        let error = engine
            .set_plugin_parameter_at(0, key, value, 0)
            .unwrap_err();
        assert!(error.contains("requires rebuilding"), "{key}: {error}");
    }
    // The taken-sender path rejects structural controls with the same rebuild
    // contract. It runs on a distinct engine: taking the producer out of
    // `engine` would disable its engine-side queue, which the
    // live-automation check below still exercises.
    let (mut sender_engine, _) = EmbeddedAudioEngine::new(&engine_config).unwrap();
    let mut sender = sender_engine.take_parameter_event_sender().unwrap();
    let error = sender
        .queue_plugin_parameter(0, "bands".into(), ParameterValue::Int(2))
        .unwrap_err();
    assert!(error.contains("requires rebuilding"), "{error}");

    // Populated history is intact: continued rendering matches the
    // uninterrupted reference bit-exactly, then the finite tail completes.
    let mut rest = corrupted[600 * CHANNELS..].to_vec();
    rest.extend(std::iter::repeat_n(0.0, 8 * CHANNELS));
    for chunk in rest.chunks(512 * CHANNELS) {
        let mut block = vec![0.0; chunk.len()];
        let mut ref_block = vec![0.0; chunk.len()];
        assert_eq!(
            engine.process_at(position, chunk, &mut block).unwrap(),
            chunk.len() / CHANNELS
        );
        assert_eq!(
            reference
                .process_at(ref_position, chunk, &mut ref_block)
                .unwrap(),
            chunk.len() / CHANNELS
        );
        position += (chunk.len() / CHANNELS) as u64;
        ref_position += (chunk.len() / CHANNELS) as u64;
        output.extend_from_slice(&block);
        expected.extend_from_slice(&ref_block);
    }
    assert_eq!(output, expected);
    assert_eq!(output.len(), (FRAMES + 8) * CHANNELS);

    // Live controls still automate after the rejections.
    engine
        .set_plugin_parameter_at(0, "sensitivity", ParameterValue::Float(5.0), 0)
        .unwrap();
    let mut probe = vec![0.0; 64 * CHANNELS];
    engine
        .process_at(position, &[0.1; 64 * CHANNELS], &mut probe)
        .unwrap();
    assert!(probe.iter().all(|sample| sample.is_finite()));

    // Structural changes take effect through a documented chain rebuild:
    // fresh latency plus audible, EOF-complete new-mode rendering.
    let rebuilt_settings: PluginSettings =
        serde_json::from_value(nondefault_settings_json(false)).unwrap();
    let rebuilt_config = rebuilt_settings.to_plugin_config(RATE as f64);
    let rebuilt_engine_config = EngineConfig {
        frame_size: 512,
        input_channels: CHANNELS,
        output_channels: CHANNELS,
        output_sample_rate: RATE,
        plugins: vec![PluginConfig::new(
            "declick",
            rebuilt_config.parameters.clone(),
        )],
        ..EngineConfig::default()
    };
    let (mut rebuilt, rebuilt_diagnostics) =
        EmbeddedAudioEngine::new(&rebuilt_engine_config).unwrap();
    assert!(rebuilt_diagnostics.is_empty());
    assert_eq!(rebuilt.latency_samples(), LATENCY);
    let mut rebuilt_out = Vec::new();
    let mut rebuilt_position = 0_u64;
    let mut rebuilt_stream = corrupted.clone();
    rebuilt_stream.extend(std::iter::repeat_n(0.0, LATENCY * CHANNELS));
    for chunk in rebuilt_stream.chunks(512 * CHANNELS) {
        let mut block = vec![0.0; chunk.len()];
        let frames = rebuilt
            .process_at(rebuilt_position, chunk, &mut block)
            .unwrap();
        assert_eq!(frames, chunk.len() / CHANNELS);
        rebuilt_position += frames as u64;
        rebuilt_out.extend_from_slice(&block);
    }
    let (_, clean, is_click) = stereo_fixture();
    check_cleaned(
        "rebuilt cleaned",
        &rebuilt_out,
        &clean,
        &is_click,
        LATENCY,
    );
}

#[test]
fn declick_clean_controls_preserve_transparency_at_engine_scope() {
    // Click-free controls mirror `modes.rs` `a1_clean_controls_are_preserved`
    // through the engine factory path: legacy fullband is bit-exact, owned
    // multiband adds crossover rounding below 1e-5. An 8-frame natural
    // continuation keeps the detector post-context of the last asserted
    // frame in-signal instead of reading flush zeros.
    const SPAN: usize = 600;
    const CONTEXT: usize = 8;
    let mut control = vec![0.0; (SPAN + CONTEXT) * CHANNELS];
    for frame in 0..SPAN + CONTEXT {
        control[frame * CHANNELS] = tone(frame, 440.0);
        control[frame * CHANNELS + 1] = tone(frame, 660.0);
    }

    let legacy: PluginSettings = serde_json::from_value(serde_json::json!({
        "Declick": { "enabled": true, "sensitivity": 2.0, "link_channels": true }
    }))
    .unwrap();
    let legacy_config = legacy.to_plugin_config(RATE as f64);
    let legacy_out = render_factory_plugin(
        &legacy_config.plugin_type,
        &legacy_config.parameters,
        &control,
        LEGACY_LATENCY,
    );
    assert_eq!(legacy_out.len(), (SPAN + CONTEXT + LEGACY_LATENCY) * CHANNELS);
    assert_eq!(
        &legacy_out[LEGACY_LATENCY * CHANNELS..(LEGACY_LATENCY + SPAN) * CHANNELS],
        &control[..SPAN * CHANNELS]
    );

    let settings: PluginSettings =
        serde_json::from_value(nondefault_settings_json(false)).unwrap();
    let config = settings.to_plugin_config(RATE as f64);
    let output =
        render_factory_plugin(&config.plugin_type, &config.parameters, &control, LATENCY);
    assert_eq!(output.len(), (SPAN + CONTEXT + LATENCY) * CHANNELS);
    let mut worst = 0.0_f32;
    for (actual, expected) in output[LATENCY * CHANNELS..(LATENCY + SPAN) * CHANNELS]
        .iter()
        .zip(control.iter())
    {
        worst = worst.max((actual - expected).abs());
    }
    assert!(worst < 1.0e-5, "new-mode control drift {worst}");
    report("controls multiband", worst);
}
