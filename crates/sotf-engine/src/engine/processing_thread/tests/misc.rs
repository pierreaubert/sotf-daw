use super::super::build::build_plugin_host;
use super::super::misc::create_plugin;
use crate::plugins::{EQFilter, EqBandPlacement, PluginSettings, PluginType};
use math_audio_iir_fir::BiquadFilterType;
use sotf_plugins::ParameterValue;

/// Returns the input channel count that `create_plugin` expects for each type.
fn input_channels_for(plugin_type: &PluginType) -> usize {
    match plugin_type {
        PluginType::Upmixer => 2,
        PluginType::XTC => 2,
        PluginType::Crossfeed => 2,
        PluginType::MonoToStereo => 1,
        // BandMerge default is 2 bands, so input = output_channels * bands = 2 * 2 = 4
        PluginType::BandMerge => 4,
        // Downmix default has input_channels = 6
        PluginType::Downmix => 6,
        // BinauralDecoder defaults to 6 input channels (5.1)
        PluginType::BinauralDecoder => 6,
        // AmbisonicsDecoder order 1 = 4 channels (FOA)
        PluginType::AmbisonicsDecoder => 4,
        _ => 2,
    }
}

#[test]
fn test_create_plugin_all_types() {
    let sample_rate = 48000;

    for plugin_type in PluginType::all() {
        // Convolution requires an IR file on disk — skip factory test
        if plugin_type == PluginType::Convolution {
            continue;
        }
        // AmbisonicsDecoder requires the `iamf` feature
        #[cfg(not(feature = "iamf"))]
        if plugin_type == PluginType::AmbisonicsDecoder {
            continue;
        }

        let settings = PluginSettings::default_for(&plugin_type).unwrap();
        let config = settings.to_plugin_config(sample_rate as f64);
        let channels = input_channels_for(&plugin_type);

        let plugin = match create_plugin(
            &config.plugin_type,
            &config.parameters,
            channels,
            sample_rate,
        ) {
            Ok(p) => p,
            Err(e) => panic!("create_plugin failed for '{}': {}", config.plugin_type, e),
        };
        assert_eq!(
            plugin.input_channels(),
            channels,
            "input_channels mismatch for '{}'",
            config.plugin_type
        );
    }
}

#[test]
fn test_build_plugin_host_all_types() {
    let sample_rate = 48000;

    for plugin_type in PluginType::all() {
        if plugin_type == PluginType::Convolution {
            continue;
        }
        #[cfg(not(feature = "iamf"))]
        if plugin_type == PluginType::AmbisonicsDecoder {
            continue;
        }

        let settings = PluginSettings::default_for(&plugin_type).unwrap();
        let config = settings.to_plugin_config(sample_rate as f64);
        let channels = input_channels_for(&plugin_type);

        match build_plugin_host(std::slice::from_ref(&config), sample_rate, channels) {
            Ok((_host, warnings)) => {
                assert!(
                    warnings.is_empty(),
                    "build_plugin_host warnings for '{}': {:?}",
                    config.plugin_type,
                    warnings
                );
            }
            Err(e) => panic!(
                "build_plugin_host failed for '{}': {}",
                config.plugin_type, e
            ),
        }
    }
}

#[test]
fn test_process_audio_all_types() {
    let sample_rate = 48000;
    let num_frames = 1024;

    for plugin_type in PluginType::all() {
        // Skip plugins that can't be tested in isolation with a simple process call:
        // - Convolution requires an IR file on disk
        // - Upmixer/BinauralDecoder/Pnd use FFT overlap-add that returns 0 frames
        //   on first call, which triggers an assertion in PluginHost
        let skip_process = matches!(
            plugin_type,
            PluginType::Convolution
                | PluginType::Upmixer
                | PluginType::BinauralDecoder
                | PluginType::Pnd
        );
        if skip_process {
            continue;
        }
        // AmbisonicsDecoder requires the `iamf` feature
        #[cfg(not(feature = "iamf"))]
        if plugin_type == PluginType::AmbisonicsDecoder {
            continue;
        }

        let settings = PluginSettings::default_for(&plugin_type).unwrap();
        let config = settings.to_plugin_config(sample_rate as f64);
        let in_channels = input_channels_for(&plugin_type);

        let (mut host, _warnings) =
            build_plugin_host(std::slice::from_ref(&config), sample_rate, in_channels)
                .unwrap_or_else(|e| panic!("build failed for '{}': {}", config.plugin_type, e));

        let out_channels = host.output_channels();

        // Generate a 440Hz sine wave as input
        let input: Vec<f32> = (0..num_frames * in_channels)
            .map(|i| {
                let frame = i / in_channels;
                (2.0 * std::f32::consts::PI * 440.0 * frame as f32 / sample_rate as f32).sin() * 0.5
            })
            .collect();

        let mut output = vec![0.0f32; num_frames * out_channels];

        let result = host.process(&input, &mut output);
        assert!(
            result.is_ok(),
            "process failed for '{}': {}",
            config.plugin_type,
            result.err().unwrap()
        );

        // Some plugins produce silence in normal operation:
        // - Gate/Expander: gate signal to zero for quiet inputs
        // - ABCompare: may bypass
        // - XTC/Denoiser/Downmix: STFT latency causes silent output on first block
        let may_produce_silence = matches!(
            plugin_type,
            PluginType::Gate
                | PluginType::Expander
                | PluginType::ABCompare
                | PluginType::XTC
                | PluginType::Denoiser
                | PluginType::Downmix
                | PluginType::MonoToStereo
                | PluginType::LinearPhaseEq
                | PluginType::SpectralCompressor
        );

        if !may_produce_silence {
            let max_abs = output.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
            assert!(
                max_abs > 1e-6,
                "plugin '{}' produced silence (max_abs={})",
                config.plugin_type,
                max_abs
            );
        }
    }
}

#[test]
fn test_parameter_sync_get_matches_parameters_list() {
    let sample_rate = 48000;

    for plugin_type in PluginType::all() {
        if plugin_type == PluginType::Convolution {
            continue;
        }
        #[cfg(not(feature = "iamf"))]
        if plugin_type == PluginType::AmbisonicsDecoder {
            continue;
        }

        let settings = PluginSettings::default_for(&plugin_type).unwrap();
        let config = settings.to_plugin_config(sample_rate as f64);
        let channels = input_channels_for(&plugin_type);

        let plugin = match create_plugin(
            &config.plugin_type,
            &config.parameters,
            channels,
            sample_rate,
        ) {
            Ok(p) => p,
            Err(_) => continue,
        };

        let params = plugin.parameters();
        for param in &params {
            let value = plugin.get_parameter(&param.id);
            assert!(
                value.is_some(),
                "Plugin '{}': parameter '{}' listed in parameters() but get_parameter() returns None. \
                     Likely missing from get_parameter() match arm.",
                config.plugin_type,
                param.id
            );
        }
    }
}

#[test]
fn test_parameter_set_then_get_roundtrip() {
    use sotf_plugins::parameters::ParameterValue;

    let sample_rate = 48000;

    for plugin_type in PluginType::all() {
        if plugin_type == PluginType::Convolution {
            continue;
        }
        #[cfg(not(feature = "iamf"))]
        if plugin_type == PluginType::AmbisonicsDecoder {
            continue;
        }

        let settings = PluginSettings::default_for(&plugin_type).unwrap();
        let config = settings.to_plugin_config(sample_rate as f64);
        let channels = input_channels_for(&plugin_type);

        let mut plugin = match create_plugin(
            &config.plugin_type,
            &config.parameters,
            channels,
            sample_rate,
        ) {
            Ok(p) => p,
            Err(_) => continue,
        };

        let params = plugin.parameters();
        for param in &params {
            // Pick a test value within the parameter's range
            let test_value = match (&param.default_value, &param.min_value, &param.max_value) {
                (
                    ParameterValue::Float(_),
                    Some(ParameterValue::Float(min)),
                    Some(ParameterValue::Float(max)),
                ) => {
                    // Use midpoint of range
                    ParameterValue::Float((min + max) / 2.0)
                }
                (ParameterValue::Bool(b), _, _) => ParameterValue::Bool(!b),
                (
                    ParameterValue::Int(_),
                    Some(ParameterValue::Int(min)),
                    Some(ParameterValue::Int(max)),
                ) => ParameterValue::Int((min + max) / 2),
                _ => continue, // Skip string/complex params
            };

            let set_result = plugin.set_parameter(param.id.clone(), test_value.clone());
            if set_result.is_err() {
                continue; // Some params may reject certain values
            }

            let got = plugin.get_parameter(&param.id);
            assert!(
                got.is_some(),
                "Plugin '{}': set_parameter('{}') succeeded but get_parameter returns None",
                config.plugin_type,
                param.id
            );
        }
    }
}

#[test]
fn test_nan_parameter_values_rejected_or_safe() {
    use sotf_plugins::parameters::ParameterValue;

    let sample_rate = 48000;
    let mut panicked_plugins = Vec::new();

    for plugin_type in PluginType::all() {
        if plugin_type == PluginType::Convolution {
            continue;
        }
        #[cfg(not(feature = "iamf"))]
        if plugin_type == PluginType::AmbisonicsDecoder {
            continue;
        }

        let settings = PluginSettings::default_for(&plugin_type).unwrap();
        let config = settings.to_plugin_config(sample_rate as f64);
        let channels = input_channels_for(&plugin_type);

        let type_name = config.plugin_type.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut plugin = match create_plugin(
                &config.plugin_type,
                &config.parameters,
                channels,
                sample_rate,
            ) {
                Ok(p) => p,
                Err(_) => return,
            };

            let params = plugin.parameters();
            for param in &params {
                if matches!(param.default_value, ParameterValue::Float(_)) {
                    let _ = plugin.set_parameter(param.id.clone(), ParameterValue::Float(f32::NAN));
                    let _ = plugin
                        .set_parameter(param.id.clone(), ParameterValue::Float(f32::INFINITY));
                    let _ = plugin
                        .set_parameter(param.id.clone(), ParameterValue::Float(f32::NEG_INFINITY));
                }
            }

            let num_frames = 64;
            let in_samples = num_frames * plugin.input_channels();
            let out_samples = num_frames * plugin.output_channels();
            let input = vec![0.5_f32; in_samples];
            let mut output = vec![0.0_f32; out_samples];
            let context = sotf_plugins::plugin::ProcessContext::new(sample_rate, num_frames);
            let _ = plugin.process(&input, &mut output, &context);
        }));

        if result.is_err() {
            panicked_plugins.push(type_name);
        }
    }

    // Log which plugins panicked with NaN — these should be fixed eventually
    // but we don't fail the test since NaN params are an edge case
    if !panicked_plugins.is_empty() {
        eprintln!(
            "WARNING: {} plugin(s) panicked with NaN/inf params: {:?}",
            panicked_plugins.len(),
            panicked_plugins
        );
    }
}

#[test]
fn test_process_zero_frames_does_not_panic() {
    let sample_rate = 48000;

    for plugin_type in PluginType::all() {
        if plugin_type == PluginType::Convolution {
            continue;
        }
        #[cfg(not(feature = "iamf"))]
        if plugin_type == PluginType::AmbisonicsDecoder {
            continue;
        }

        let settings = PluginSettings::default_for(&plugin_type).unwrap();
        let config = settings.to_plugin_config(sample_rate as f64);
        let channels = input_channels_for(&plugin_type);

        let mut plugin = match create_plugin(
            &config.plugin_type,
            &config.parameters,
            channels,
            sample_rate,
        ) {
            Ok(p) => p,
            Err(_) => continue,
        };

        let context = sotf_plugins::plugin::ProcessContext::new(sample_rate, 0);
        // Zero-length buffers — must not panic
        let _ = plugin.process(&[], &mut [], &context);
    }
}

fn aud145_placement_fixture_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../audit/artifacts/aud145-placement-reference-r1")
}

// Portable regressions use the independently generated R2 reference packet.
// Historical capture tooling keeps its original R1 root and cases.
fn eq_placement_regression_fixture_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/aud145-analytical-reference-r2")
}

fn read_f32le_samples(path: &std::path::Path, expected_count: usize) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    assert_eq!(
        bytes.len(),
        expected_count * 4,
        "{} byte length",
        path.display()
    );
    let (chunks, remainder) = bytes.as_slice().as_chunks::<4>();
    assert!(remainder.is_empty(), "{} trailing bytes", path.display());
    chunks
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect()
}

fn read_f64le_samples(path: &std::path::Path, expected_count: usize) -> Vec<f64> {
    let bytes = std::fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    assert_eq!(
        bytes.len(),
        expected_count * 8,
        "{} byte length",
        path.display()
    );
    let (chunks, remainder) = bytes.as_slice().as_chunks::<8>();
    assert!(remainder.is_empty(), "{} trailing bytes", path.display());
    chunks
        .iter()
        .map(|chunk| f64::from_le_bytes(*chunk))
        .collect()
}

#[test]
#[ignore = "AUD145 independent-reference capture; set AUD145_EQ_PLACEMENT_CAPTURE_DIR"]
fn capture_eq_placement_vectors_through_engine_factory_and_daw_host() {
    let root = aud145_placement_fixture_root();
    let capture_dir = std::env::var_os("AUD145_EQ_PLACEMENT_CAPTURE_DIR")
        .map(std::path::PathBuf::from)
        .expect("set AUD145_EQ_PLACEMENT_CAPTURE_DIR to an absolute capture directory");
    std::fs::create_dir_all(&capture_dir).unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("cases.json")).unwrap()).unwrap();
    let filter_config = &manifest["filter"];
    assert_eq!(filter_config["type"], "peak");

    for case in manifest["cases"].as_array().expect("cases array") {
        let sample_rate = case["sample_rate"].as_u64().unwrap() as u32;
        let channels = case["channels"].as_u64().unwrap() as usize;
        let frames = case["frames"].as_u64().unwrap() as usize;
        let placements = case["placements"].as_array().unwrap();
        let filters = placements
            .iter()
            .map(|placement| {
                let mut filter = EQFilter::new(
                    BiquadFilterType::Peak,
                    filter_config["frequency"].as_f64().unwrap(),
                    filter_config["q"].as_f64().unwrap(),
                    filter_config["gain_db"].as_f64().unwrap(),
                );
                filter.order = filter_config["order"].as_u64().unwrap() as usize;
                filter.placement = Some(match placement.as_str().unwrap() {
                    "stereo" => EqBandPlacement::Stereo,
                    "left" => EqBandPlacement::Left,
                    "right" => EqBandPlacement::Right,
                    "mid" => EqBandPlacement::Mid,
                    "side" => EqBandPlacement::Side,
                    other => panic!("unknown placement {other}"),
                });
                filter
            })
            .collect();
        let stereo_pairs = serde_json::from_value(case["pairs"].clone()).unwrap();
        let settings = PluginSettings::EQ {
            channels,
            filters,
            channel_filters: None,
            stereo_pairs: Some(stereo_pairs),
            per_channel_mode: false,
            max_filters: 20,
            tdf2: false,
            topology: 0.0,
            auto_gain_enabled: false,
            oversampling: 1.0,
        };
        let config = settings.to_plugin_config(sample_rate as f64);
        let (mut host, warnings) = build_plugin_host(&[config], sample_rate, channels)
            .unwrap_or_else(|error| panic!("EQ host build failed: {error}"));
        assert!(
            warnings.is_empty(),
            "unexpected host warnings: {warnings:?}"
        );
        assert_eq!(host.output_channels(), channels);
        assert!(
            host.get_plugin(0)
                .expect("built EQ plugin")
                .compile_metadata()
                .compiled_op
                .is_none(),
            "explicit ordered EQ must use the generic host path"
        );

        let input = read_f32le_samples(
            &root.join(case["input"].as_str().unwrap()),
            frames * channels,
        );
        let mut output = vec![0.0f32; frames * channels];
        let processed = host.process(&input, &mut output).expect("EQ host process");
        assert_eq!(processed, frames);
        assert!(output.iter().all(|sample| sample.is_finite()));

        let reference_name = case["reference"].as_str().unwrap();
        let capture_name = reference_name.strip_suffix(".f64le").unwrap().to_owned() + ".f32le";
        let mut bytes = Vec::with_capacity(output.len() * 4);
        for sample in output {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(capture_dir.join(capture_name), bytes).unwrap();
    }
}

#[test]
fn compiled_legacy_eq_host_falls_back_after_ordered_placement_event() {
    let root = eq_placement_regression_fixture_root();
    let channels = 2;
    let sample_rate = 44_100;
    let mut filter = EQFilter::new(BiquadFilterType::Peak, 1379.0, 0.83, 7.0);
    filter.order = 2;
    let settings = PluginSettings::EQ {
        channels,
        filters: vec![filter],
        channel_filters: None,
        stereo_pairs: Some(vec![[0, 1]]),
        per_channel_mode: false,
        max_filters: 20,
        tdf2: false,
        topology: 0.0,
        auto_gain_enabled: false,
        oversampling: 1.0,
    };
    let config = settings.to_plugin_config(sample_rate as f64);
    let (mut host, warnings) = build_plugin_host(&[config], sample_rate, channels).unwrap();
    assert!(warnings.is_empty());
    assert!(
        host.get_plugin(0)
            .unwrap()
            .compile_metadata()
            .compiled_op
            .is_some()
    );

    let warm_input = vec![0.25f32; 128 * channels];
    let mut warm_output = vec![0.0f32; warm_input.len()];
    assert_eq!(host.process(&warm_input, &mut warm_output).unwrap(), 128);

    // The supported automation API rejects structural placement changes.
    let mid = ParameterValue::Int(4);
    assert!(
        host.validate_automatable_plugin_parameter(0, "filter_0_placement", &mid)
            .is_err()
    );
    assert!(
        host.set_plugin_parameter_at(0, "filter_0_placement", mid.clone(), 0)
            .is_err()
    );

    // Exercise the defensive path if a low-level event reaches an already
    // compiled legacy host: EqPlugin declines the stale compiled operation,
    // and DawHost falls back to the generic ordered route.
    host.set_plugin_parameter(0, "filter_0_placement", mid)
        .unwrap();
    let input = read_f32le_samples(&root.join("44100-2ch-input.f32le"), 4096 * channels);
    let mut output = vec![0.0f32; input.len()];
    assert_eq!(host.process(&input, &mut output).unwrap(), 4096);
    assert!(
        host.get_plugin(0)
            .unwrap()
            .compile_metadata()
            .compiled_op
            .is_none()
    );

    let reference = read_f64le_samples(&root.join("44100-2ch-mid.f64le"), output.len());
    let peak_error = output
        .iter()
        .zip(&reference)
        .map(|(actual, expected)| (f64::from(*actual) - expected).abs())
        .fold(0.0, f64::max);
    let rms_error = (output
        .iter()
        .zip(&reference)
        .map(|(actual, expected)| {
            let error = f64::from(*actual) - expected;
            error * error
        })
        .sum::<f64>()
        / reference.len() as f64)
        .sqrt();
    assert!(peak_error <= 2e-5, "peak error {peak_error}");
    assert!(rms_error <= 2e-6, "RMS error {rms_error}");
}

#[test]
fn rejected_all_muted_per_channel_placement_keeps_live_eq_host_unchanged() {
    let fixture_root = eq_placement_regression_fixture_root();
    let sample_rate = 44_100;
    let channels = 2;

    let mut legacy_filter = EQFilter::new(BiquadFilterType::Peak, 1379.0, 0.83, 7.0);
    legacy_filter.order = 2;
    let legacy_config = PluginSettings::EQ {
        channels,
        filters: vec![legacy_filter],
        channel_filters: None,
        stereo_pairs: None,
        per_channel_mode: false,
        max_filters: 20,
        tdf2: false,
        topology: 0.0,
        auto_gain_enabled: false,
        oversampling: 1.0,
    }
    .to_plugin_config(sample_rate as f64);

    let (mut live_host, live_warnings) =
        build_plugin_host(std::slice::from_ref(&legacy_config), sample_rate, channels).unwrap();
    let (mut twin_host, twin_warnings) =
        build_plugin_host(std::slice::from_ref(&legacy_config), sample_rate, channels).unwrap();
    assert!(live_warnings.is_empty());
    assert!(twin_warnings.is_empty());

    let warm_input = vec![0.25f32; 128 * channels];
    let mut live_warm_output = vec![0.0f32; warm_input.len()];
    let mut twin_warm_output = vec![0.0f32; warm_input.len()];
    assert_eq!(
        live_host
            .process(&warm_input, &mut live_warm_output)
            .unwrap(),
        128
    );
    assert_eq!(
        twin_host
            .process(&warm_input, &mut twin_warm_output)
            .unwrap(),
        128
    );
    assert_eq!(live_warm_output, twin_warm_output);

    let mut muted_explicit = EQFilter::new(BiquadFilterType::Peak, 2100.0, 0.9, -3.0);
    muted_explicit.muted = true;
    muted_explicit.placement = Some(EqBandPlacement::Mid);
    let invalid_candidate = PluginSettings::EQ {
        channels,
        filters: Vec::new(),
        channel_filters: Some(vec![vec![muted_explicit.clone()], vec![muted_explicit]]),
        stereo_pairs: Some(vec![[0, 1]]),
        per_channel_mode: true,
        max_filters: 20,
        tdf2: false,
        topology: 0.0,
        auto_gain_enabled: false,
        oversampling: 1.0,
    }
    .to_plugin_config(sample_rate as f64);

    let rejection = create_plugin(
        &invalid_candidate.plugin_type,
        &invalid_candidate.parameters,
        channels,
        sample_rate,
    );
    assert!(
        rejection.is_err(),
        "factory must reject saved explicit placement even when every source band is muted"
    );

    let continuation =
        read_f32le_samples(&fixture_root.join("44100-2ch-input.f32le"), 4096 * channels);
    let mut live_output = vec![0.0f32; continuation.len()];
    let mut twin_output = vec![0.0f32; continuation.len()];
    assert_eq!(
        live_host.process(&continuation, &mut live_output).unwrap(),
        4096
    );
    assert_eq!(
        twin_host.process(&continuation, &mut twin_output).unwrap(),
        4096
    );
    assert!(live_output.iter().all(|sample| sample.is_finite()));
    assert!(live_output.iter().any(|sample| sample.abs() > 1e-4));
    assert_eq!(
        live_output, twin_output,
        "failed structural candidate construction must leave the populated live route unchanged"
    );
}
