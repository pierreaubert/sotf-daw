//! Hiss momentary controls through the real host immediate route.
//!
//! Exercises `DawHost::set_plugin_parameter_immediate` exactly as the engine
//! `SetParameter` command does: start and cancel `learn_noise`, fire
//! `clear_profile`, observe live progress and generation snapshots, preserve
//! prior accepted profiles, refuse unrelated structural ids, reject automation
//! replay, rebuild from a live-captured typed carrier with bit-exact audio
//! and finite EOF equivalence, fail closed on panicking probes, and preserve
//! admission under forced oversampling.

// Rust guideline compliant 2026-02-21
use sotf_host::oversampling::OS_CHUNK_SIZE;
use sotf_plugins::param_specs::UpdateMode;
use sotf_plugins::plugin_hiss_reducer::snapshot::ProfileSnapshot;
use sotf_plugins::{
    DawHost, Host, Parameter, ParameterId, ParameterValue, Plugin, PluginInfo, ProcessContext,
    create_plugin,
};
use std::sync::Arc;

const RATE: u32 = 48_000;

fn lcg_noise(frames: usize, amplitude: f32, seed: u32) -> Vec<f32> {
    let mut state = seed;
    (0..frames)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            amplitude * ((state as f32 / u32::MAX as f32) * 2.0 - 1.0)
        })
        .collect()
}

fn first_difference_hiss(frames: usize, amplitude: f32, seed: u32) -> Vec<f32> {
    let white = lcg_noise(frames, 1.0, seed);
    let mut previous = 0.0f32;
    white
        .iter()
        .map(|&sample| {
            let high_pass = amplitude * (sample - previous);
            previous = sample;
            high_pass
        })
        .collect()
}

fn sine_tone(frames: usize, amplitude: f32, freq_hz: f64) -> Vec<f32> {
    (0..frames)
        .map(|i| {
            (f64::from(amplitude)
                * (2.0 * std::f64::consts::PI * freq_hz * i as f64 / f64::from(RATE)).sin())
                as f32
        })
        .collect()
}

fn hiss_host(params: &serde_json::Value) -> DawHost {
    let mut host = DawHost::new(1, RATE);
    let plugin = create_plugin("hiss_reducer", params, 1, RATE).unwrap();
    host.add_plugin(plugin).unwrap();
    host.build().unwrap();
    host
}

fn hiss_snapshot(host: &DawHost) -> Arc<ProfileSnapshot> {
    host.get_plugin_data(0)
        .expect("host must transport Hiss snapshot")
        .downcast::<ProfileSnapshot>()
        .expect("snapshot must downcast")
}

fn process_all(host: &mut DawHost, input: &[f32], block: usize) -> Vec<f32> {
    let mut output = Vec::with_capacity(input.len());
    for chunk in input.chunks(block) {
        let mut block_out = vec![0.0f32; chunk.len()];
        let frames = host.process(chunk, &mut block_out).unwrap();
        assert_eq!(frames, chunk.len());
        output.extend_from_slice(&block_out);
    }
    output
}

fn drain_all(host: &mut DawHost) -> Vec<f32> {
    let mut output = Vec::new();
    let capacity = host.drain_output_frames_max().max(1);
    for _ in 0..4096 {
        let mut block = vec![0.0f32; capacity];
        let status = host.drain(&mut block).unwrap();
        output.extend_from_slice(&block[..status.frames]);
        if status.complete {
            return output;
        }
    }
    panic!("host drain did not complete");
}

fn immediate(host: &mut DawHost, id: &str, value: bool) -> Result<(), String> {
    host.set_plugin_parameter_immediate(0, id, ParameterValue::Bool(value))
}

#[test]
fn immediate_admits_learn_and_clear_with_live_snapshot() {
    let mut host = hiss_host(&serde_json::json!({}));
    let snapshot = hiss_snapshot(&host);
    let gen_empty = snapshot.try_status().unwrap().generation;
    assert!(snapshot.try_export().unwrap().is_none());
    assert_eq!(snapshot.capture_state(), (false, 0.0));

    // Start through the real immediate route, not the DSP setter.
    immediate(&mut host, "learn_noise", true).unwrap();
    assert_eq!(snapshot.capture_state(), (true, 0.0));
    assert_eq!(
        host.get_plugin(0)
            .unwrap()
            .get_parameter(&ParameterId::from("learn_noise")),
        Some(ParameterValue::Bool(true))
    );

    // Partial progress advances; export stays absent; generation holds.
    let colored = first_difference_hiss(RATE as usize, 0.035, 0xe940c);
    process_all(&mut host, &colored[..12288], 4096);
    let (active, progress) = snapshot.capture_state();
    assert!(active);
    assert_eq!(progress, 12288f32 / 48000f32);
    assert!(snapshot.try_export().unwrap().is_none());
    assert_eq!(snapshot.try_status().unwrap().generation, gen_empty);

    // Completion publishes a v2 profile with a fresh generation.
    process_all(&mut host, &colored[12288..], 4096);
    assert_eq!(snapshot.capture_state(), (false, 0.0));
    let done = snapshot.try_export().unwrap().expect("capture must export");
    assert_eq!(done.profile.format_version, 2);
    assert_eq!(done.profile.frames_analyzed, u64::from(RATE));
    assert!(done.generation > gen_empty);

    // Idle cancel preserves the accepted profile and generation.
    immediate(&mut host, "learn_noise", false).unwrap();
    let kept = snapshot.try_export().unwrap().expect("cancel keeps profile");
    assert_eq!(kept.generation, done.generation);
    assert_eq!(kept.profile, done.profile);

    // Deliberate clear drops the profile with a fresh generation.
    immediate(&mut host, "clear_profile", true).unwrap();
    assert!(snapshot.try_export().unwrap().is_none());
    assert!(snapshot.try_status().unwrap().generation > done.generation);
    assert_eq!(snapshot.capture_state(), (false, 0.0));

    // Clear-off is an accepted no-op.
    immediate(&mut host, "clear_profile", false).unwrap();
    assert!(snapshot.try_export().unwrap().is_none());
}

#[test]
fn cancel_preserves_prior_accepted_profile_and_audio() {
    let mut host = hiss_host(&serde_json::json!({}));
    let colored = first_difference_hiss(RATE as usize, 0.035, 0xca97);
    immediate(&mut host, "learn_noise", true).unwrap();
    process_all(&mut host, &colored, 4096);
    let snapshot = hiss_snapshot(&host);
    let accepted = snapshot.try_export().unwrap().expect("first capture");
    immediate(&mut host, "use_captured_profile", true).unwrap();
    let gen_accepted = snapshot.try_status().unwrap().generation;
    host.reset();
    let hiss = lcg_noise(8192, 0.04, 0x77aa);
    let tone = sine_tone(8192, 0.06, 9984.375);
    let mix: Vec<f32> = hiss.iter().zip(tone.iter()).map(|(h, t)| h + t).collect();
    let reference = process_all(&mut host, &mix, 997);

    // Restart, accumulate partial, then cancel through the immediate route.
    immediate(&mut host, "learn_noise", true).unwrap();
    process_all(&mut host, &colored[..8192], 4096);
    let (active, progress) = snapshot.capture_state();
    assert!(active && progress > 0.0);
    // The accepted export stays readable while the new capture runs.
    let during = snapshot.try_export().unwrap().expect("prior stays");
    assert_eq!(during.profile, accepted.profile);
    immediate(&mut host, "learn_noise", false).unwrap();
    assert_eq!(snapshot.capture_state(), (false, 0.0));
    let kept = snapshot.try_export().unwrap().expect("cancel keeps prior");
    assert_eq!(kept.profile, accepted.profile);
    assert_eq!(
        snapshot.try_status().unwrap().generation,
        gen_accepted,
        "cancel must preserve the accepted generation"
    );

    // Same mix after reset renders bit-exactly: no history was lost.
    host.reset();
    let after = process_all(&mut host, &mix, 997);
    assert_eq!(reference, after);
}

#[test]
fn structural_refusals_and_automation_rejections_hold() {
    let mut host = hiss_host(&serde_json::json!({}));
    // Spectral mode stays rebuild-only through the immediate gate.
    for value in [true, false] {
        let error = immediate(&mut host, "spectral_mode", value).unwrap_err();
        assert!(
            error.contains("requires rebuilding"),
            "spectral_mode must refuse: {error}"
        );
    }
    // Non-momentary realtime controls still pass through immediate.
    immediate(&mut host, "use_captured_profile", true).unwrap();

    // Automation never replays momentary controls on Hiss.
    for id in ["learn_noise", "clear_profile"] {
        let value = ParameterValue::Bool(true);
        let error = host
            .validate_automatable_plugin_parameter(0, id, &value)
            .unwrap_err();
        assert!(error.contains("requires rebuilding"), "{id}: {error}");
        let error = host
            .set_plugin_parameter_at(0, id, value, 0)
            .unwrap_err();
        assert!(error.contains("requires rebuilding"), "{id}: {error}");
    }
    // A realtime Hiss flag remains automatable as a control case.
    host.set_plugin_parameter_at(
        0,
        "use_captured_profile",
        ParameterValue::Bool(false),
        0,
    )
    .unwrap();

    // The detached automation sender also rejects momentary ids.
    let mut sender_host = hiss_host(&serde_json::json!({}));
    let mut sender = sender_host
        .take_parameter_event_sender()
        .expect("sender must detach");
    for id in ["learn_noise", "clear_profile"] {
        let error = sender
            .queue_plugin_parameter(0, ParameterId::from(id), ParameterValue::Bool(true))
            .unwrap_err();
        assert!(error.contains("requires rebuilding"), "{id}: {error}");
    }

    // No global id bypass: the denoiser shares trigger ids but opts out.
    let mut denoiser = DawHost::new(1, RATE);
    let plugin = create_plugin("denoiser", &serde_json::json!({}), 1, RATE).unwrap();
    denoiser.add_plugin(plugin).unwrap();
    denoiser.build().unwrap();
    for id in ["learn_noise", "clear_profile", "low_latency", "multi_resolution"] {
        let error = denoiser
            .set_plugin_parameter_immediate(0, id, ParameterValue::Bool(true))
            .unwrap_err();
        assert!(
            error.contains("requires rebuilding"),
            "denoiser {id} must refuse: {error}"
        );
    }
}

#[test]
fn live_capture_typed_carrier_rebuild_matches_audio_and_eof() {
    // Spectral host so EOF exercises the finite tail and drain path.
    let mut host = hiss_host(&serde_json::json!({
        "spectral_mode": true,
        "strength": 0.85,
    }));
    let colored = first_difference_hiss(RATE as usize, 0.035, 0xe940c);
    immediate(&mut host, "learn_noise", true).unwrap();
    process_all(&mut host, &colored, 4096);
    let snapshot = hiss_snapshot(&host);
    let live = snapshot.try_export().unwrap().expect("live capture");
    assert_eq!(live.profile.format_version, 2);
    immediate(&mut host, "use_captured_profile", true).unwrap();
    host.reset();

    let hiss = lcg_noise(16384, 0.04, 0xe9f1);
    let tone = sine_tone(16384, 0.06, 9984.375);
    let mix: Vec<f32> = hiss.iter().zip(tone.iter()).map(|(h, t)| h + t).collect();
    let mut original = process_all(&mut host, &mix, 4096);
    original.extend(drain_all(&mut host));
    assert!(
        original.iter().all(|sample| sample.is_finite()),
        "original full output must be finite"
    );
    assert_ne!(
        original[1024..16384],
        mix[0..15360],
        "engaged capture must process audio"
    );

    // Typed carrier round-trip from the live export; never handcrafted.
    let mut settings =
        sotf_audio::PluginSettings::default_for(&sotf_audio::PluginType::HissReducer).unwrap();
    if let sotf_audio::PluginSettings::HissReducer {
        captured_profile,
        use_captured_profile,
        spectral_mode,
        strength,
        ..
    } = &mut settings
    {
        *captured_profile = Some(live.profile.clone());
        *use_captured_profile = true;
        *spectral_mode = true;
        *strength = 0.85;
    } else {
        panic!("expected HissReducer settings");
    }
    let json = serde_json::to_value(&settings).unwrap();
    assert!(json["HissReducer"].get("captured_profile").is_some());
    let restored: sotf_audio::PluginSettings = serde_json::from_value(json).unwrap();
    let config = restored.to_plugin_config(f64::from(RATE));
    assert!(config.parameters.get("captured_profile").is_some());
    assert!(config.parameters.get("learn_noise").is_none());
    assert!(config.parameters.get("clear_profile").is_none());

    // Fresh factory rebuild from the converted carrier.
    let mut rebuilt_host = DawHost::new(1, RATE);
    let plugin =
        create_plugin(&config.plugin_type, &config.parameters, 1, RATE).unwrap();
    rebuilt_host.add_plugin(plugin).unwrap();
    rebuilt_host.build().unwrap();
    let rebuilt_snapshot = hiss_snapshot(&rebuilt_host);
    let rebuilt_export = rebuilt_snapshot
        .try_export()
        .unwrap()
        .expect("rebuilt carrier");
    assert_eq!(rebuilt_export.profile, live.profile);

    let mut rebuilt = process_all(&mut rebuilt_host, &mix, 4096);
    rebuilt.extend(drain_all(&mut rebuilt_host));
    assert!(
        rebuilt.iter().all(|sample| sample.is_finite()),
        "rebuilt full output must be finite"
    );
    assert_eq!(
        original, rebuilt,
        "typed-carrier rebuild must match live audio and EOF bit-exactly"
    );
}

/// Pass-through plugin whose momentary hook panics on every probe.
struct PanicHookPlugin;

impl Plugin for PanicHookPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Panic Hook", "1", "Test")
    }

    fn input_channels(&self) -> usize {
        1
    }

    fn output_channels(&self) -> usize {
        1
    }

    fn parameters(&self) -> Vec<Parameter> {
        vec![Parameter::new_bool("trigger", "Trigger", false)
            .with_update_mode(UpdateMode::Structural)]
    }

    fn set_parameter(
        &mut self,
        _id: ParameterId,
        _value: ParameterValue,
    ) -> Result<(), String> {
        Ok(())
    }

    fn get_parameter(&self, _id: &ParameterId) -> Option<ParameterValue> {
        Some(ParameterValue::Bool(false))
    }

    fn supports_immediate_momentary_control(&self, _id: &ParameterId) -> bool {
        panic!("injected hook panic")
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        _context: &ProcessContext,
    ) -> Result<usize, String> {
        output.copy_from_slice(input);
        Ok(input.len())
    }
}

/// Pass-through plugin whose parameter metadata panics on every probe.
struct PanicMetadataPlugin;

impl Plugin for PanicMetadataPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Panic Metadata", "1", "Test")
    }

    fn input_channels(&self) -> usize {
        1
    }

    fn output_channels(&self) -> usize {
        1
    }

    fn parameters(&self) -> Vec<Parameter> {
        panic!("injected metadata panic")
    }

    fn set_parameter(
        &mut self,
        _id: ParameterId,
        _value: ParameterValue,
    ) -> Result<(), String> {
        Ok(())
    }

    fn get_parameter(&self, _id: &ParameterId) -> Option<ParameterValue> {
        Some(ParameterValue::Bool(false))
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        _context: &ProcessContext,
    ) -> Result<usize, String> {
        output.copy_from_slice(input);
        Ok(input.len())
    }
}

#[test]
fn panicking_probe_fails_closed_and_preserves_host() {
    let mut host = DawHost::new(1, RATE);
    host.add_plugin(create_plugin("hiss_reducer", &serde_json::json!({}), 1, RATE).unwrap())
        .unwrap();
    host.add_plugin(Box::new(PanicHookPlugin)).unwrap();
    host.add_plugin(Box::new(PanicMetadataPlugin)).unwrap();
    host.build().unwrap();

    // Accepted Hiss history exists before any panicking probe runs.
    let colored = first_difference_hiss(RATE as usize, 0.035, 0xe940c);
    immediate(&mut host, "learn_noise", true).unwrap();
    process_all(&mut host, &colored, 4096);
    let snapshot = hiss_snapshot(&host);
    let accepted = snapshot.try_export().unwrap().expect("capture first");
    let gen_accepted = snapshot.try_status().unwrap().generation;

    // Both probes fail closed as errors instead of unwinding.
    let error = host
        .set_plugin_parameter_immediate(1, "trigger", ParameterValue::Bool(true))
        .unwrap_err();
    assert!(
        error.contains("panicked"),
        "hook panic must fail closed: {error}"
    );
    let error = host
        .set_plugin_parameter_immediate(2, "trigger", ParameterValue::Bool(true))
        .unwrap_err();
    assert!(
        error.contains("panicked"),
        "metadata panic must fail closed: {error}"
    );

    // Accepted Hiss state is untouched by the failed probes.
    let kept = snapshot.try_export().unwrap().expect("profile survives");
    assert_eq!(kept.profile, accepted.profile);
    assert_eq!(snapshot.try_status().unwrap().generation, gen_accepted);

    // The host stays usable: a later command applies and renders finite.
    immediate(&mut host, "use_captured_profile", true).unwrap();
    let rendered = process_all(&mut host, &colored[..8192], 4096);
    assert!(rendered.iter().all(|sample| sample.is_finite()));
    assert!(snapshot.try_export().unwrap().is_some());
}

#[test]
fn forced_oversampling_preserves_momentary_admission_and_refusals() {
    for factor in [2u32, 4] {
        let mut host = DawHost::new(1, RATE);
        host.set_forced_oversampling_factor(Some(factor)).unwrap();
        host.add_plugin(create_plugin("hiss_reducer", &serde_json::json!({}), 1, RATE).unwrap())
            .unwrap();
        host.build().unwrap();
        assert!(
            host.get_plugin(0)
                .unwrap()
                .info()
                .name
                .contains(&format!("({factor}x)")),
            "factor {factor} must wrap Hiss"
        );

        // Chunk-contract accounting (derived, not padded): the oversampler
        // forwards input to the inner plugin only in full OS_CHUNK_SIZE
        // host-frame groups, stranding any remainder in residual_in, so one
        // host second delivers RATE - RATE % OS_CHUNK_SIZE inner-rate
        // frames per factor unit (48000 = 187 * 256 + 128). Filter group
        // delay shifts output timing (reported PDC latency), never the
        // inner delivery count: every chunk yields exactly
        // OS_CHUNK_SIZE * factor inner frames. Capture therefore needs
        // the next chunk multiple of host input, and completes at that
        // chunk boundary with analyzed frames equal to produced frames.
        let stranded = RATE as usize % OS_CHUNK_SIZE;
        let required_host_frames = RATE as usize + (OS_CHUNK_SIZE - stranded) % OS_CHUNK_SIZE;
        assert_eq!((stranded, required_host_frames), (128, 48128));

        // Momentary admission flows through the oversampling wrapper.
        immediate(&mut host, "learn_noise", true).unwrap();
        let snapshot = hiss_snapshot(&host);
        let gen_before = snapshot.try_status().unwrap().generation;
        assert!(snapshot.try_export().unwrap().is_none());
        assert_eq!(snapshot.capture_state(), (true, 0.0));

        // Structural and automation refusals hold through the wrapper.
        let error = immediate(&mut host, "spectral_mode", true).unwrap_err();
        assert!(
            error.contains("requires rebuilding"),
            "factor {factor}: {error}"
        );
        let error = host
            .set_plugin_parameter_at(0, "learn_noise", ParameterValue::Bool(true), 0)
            .unwrap_err();
        assert!(
            error.contains("requires rebuilding"),
            "factor {factor}: {error}"
        );

        // After exactly one host second the stranded remainder is still
        // buffered: capture open at the derived partial progress
        // (47872 / 48000 = measured 0.99733335), export absent,
        // generation held.
        let colored = first_difference_hiss(required_host_frames, 0.035, 0xe940c);
        process_all(&mut host, &colored[..RATE as usize], 4096);
        let (active, progress) = snapshot.capture_state();
        assert!(active, "factor {factor} must still capture");
        assert_eq!(
            progress,
            (RATE as usize - stranded) as f32 / RATE as f32,
            "factor {factor} partial progress must match chunked delivery"
        );
        assert!(snapshot.try_export().unwrap().is_none());
        assert_eq!(snapshot.try_status().unwrap().generation, gen_before);

        // The completing chunk multiple finishes the 1 s inner target at
        // the chunk boundary with a fresh generation.
        process_all(&mut host, &colored[RATE as usize..], 4096);
        assert_eq!(snapshot.capture_state(), (false, 0.0));
        let done = snapshot
            .try_export()
            .unwrap()
            .expect("wrapped capture must export");
        assert_eq!(done.profile.format_version, 2);
        assert_eq!(done.profile.sample_rate, f64::from(RATE * factor));
        assert_eq!(
            done.profile.frames_analyzed,
            (required_host_frames * factor as usize) as u64,
            "factor {factor} must analyze exactly the produced inner frames"
        );
        assert!(done.generation > gen_before);

        immediate(&mut host, "clear_profile", true).unwrap();
        assert!(snapshot.try_export().unwrap().is_none());
    }
}
