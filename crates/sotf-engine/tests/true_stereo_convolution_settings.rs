//! Persisted engine controls reconstruct and drive the true-stereo IR route.

use sotf_audio::{PluginSettings, PluginType};
use sotf_plugins::{ParameterId, ParameterValue, ProcessContext, create_plugin};

#[test]
fn structural_true_stereo_setting_survives_preset_and_reaches_audible_factory_path() {
    let temp_dir = tempfile::tempdir().expect("create temporary IR directory");
    let ir_path = temp_dir.path().join("true-stereo.wav");
    let spec = hound::WavSpec {
        channels: 4,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&ir_path, spec).expect("create four-path IR");
    let paths = [
        [8192_i16, 0, 4096, 0, 0],
        [-4096_i16, 0, 0, 2048, 0],
        [6144_i16, 0, 0, 0, -2048],
        [8192_i16, 2048, 0, 0, 0],
    ];
    for frame in 0..paths[0].len() {
        for path in &paths {
            writer
                .write_sample(path[frame])
                .expect("write IR path sample");
        }
    }
    writer.finalize().expect("finalize four-path IR");

    let mut legacy =
        PluginSettings::default_for(&PluginType::Convolution).expect("convolution defaults exist");
    let true_stereo_index =
        sotf_plugins::param_specs::index_of(legacy.param_specs(), "true_stereo");
    let spec = &legacy.param_specs()[true_stereo_index];
    assert_eq!(
        spec.update_mode,
        sotf_plugins::param_specs::UpdateMode::Structural
    );
    assert_eq!(legacy.param_value(true_stereo_index), Some(0.0));

    // Old presets omit this newly introduced structural setting and retain the
    // legacy channel-0/1 mapping.
    let mut old_preset = serde_json::to_value(&legacy).expect("serialize legacy settings");
    old_preset
        .as_object_mut()
        .expect("settings use an object wire format")
        .get_mut("Convolution")
        .and_then(serde_json::Value::as_object_mut)
        .expect("convolution variant is serialized by name")
        .remove("true_stereo");
    let restored_old: PluginSettings =
        serde_json::from_value(old_preset).expect("old preset remains loadable");
    assert_eq!(restored_old.param_value(true_stereo_index), Some(0.0));

    // This is the same generic indexed setter used by engine Advanced controls.
    legacy.set_param_value(true_stereo_index, 1.0);
    assert_eq!(legacy.param_value(true_stereo_index), Some(1.0));
    assert_eq!(
        legacy.engine_param_at(true_stereo_index),
        None,
        "a structural route change rebuilds the plugin instead of emitting a live DSP update"
    );
    let PluginSettings::Convolution {
        ir_file: setting_path,
        mix,
        ..
    } = &mut legacy
    else {
        panic!("convolution defaults must use the Convolution settings variant");
    };
    *setting_path = ir_path.to_string_lossy().into_owned();
    *mix = 1.0;

    let saved = serde_json::to_vec(&legacy).expect("serialize opted-in settings");
    let restored: PluginSettings = serde_json::from_slice(&saved).expect("restore saved settings");
    assert_eq!(restored.param_value(true_stereo_index), Some(1.0));
    let config = restored.to_plugin_config(48_000.0);
    assert_eq!(config.plugin_type, "convolution");
    assert_eq!(config.parameters["true_stereo"], true);

    let mut plugin = create_plugin(&config.plugin_type, &config.parameters, 2, 48_000)
        .expect("factory rebuilds the opted-in route from the saved engine preset");
    plugin
        .initialize(48_000)
        .expect("initialize matrix convolution");
    plugin.reset();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("true_stereo")),
        Some(ParameterValue::Bool(true))
    );
    assert_eq!(plugin.latency_samples(), 1024);

    let input_frames = 1100;
    let mut input = vec![0.0_f32; input_frames * 2];
    input[0] = 0.5; // left impulse: LL -> left and LR -> right
    input[20 * 2 + 1] = 0.25; // right impulse: RL -> left and RR -> right
    let mut output = vec![f32::NAN; input.len()];
    assert_eq!(
        plugin
            .process(
                &input,
                &mut output,
                &ProcessContext::new(48_000, input_frames),
            )
            .expect("process routed impulses"),
        input_frames
    );

    let frame = |n: usize| [output[n * 2], output[n * 2 + 1]];
    let left_input_ll = frame(1024);
    assert!((left_input_ll[0] - 0.5 * (8192.0 / 32768.0)).abs() < 2e-5);
    assert!((left_input_ll[1] - 0.5 * (-4096.0 / 32768.0)).abs() < 2e-5);

    let right_input_rl = frame(1044);
    assert!((right_input_rl[0] - 0.25 * (6144.0 / 32768.0)).abs() < 2e-5);
    assert!((right_input_rl[1] - 0.25 * (8192.0 / 32768.0)).abs() < 2e-5);
}
