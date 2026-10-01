#![cfg(all(feature = "external-plugin-clap", feature = "external-plugin-vst3"))]

//! Loaded native Crossover routing through the consuming ExternalPlugin host.
//!
//! Expected vectors come from a separately constructed public Crossover DSP
//! configured from this test's declared case values. They validate routing,
//! state composition and channel order; the independent response oracle remains
//! the coefficient-level DSP reference.

use serde_json::{Value, json};
use sotf_host::external_plugin::{
    ExternalPlugin, NativeCrossoverInputLayout as InputLayout, NativeCrossoverMode as Mode,
    NativeCrossoverOutputLayout as OutputLayout, NativeCrossoverTopology as Topology,
    NativePluginAudioSetup, PluginDescriptor, PluginFormat, PluginScanStatus,
};
use sotf_host::parameters::ParameterValue;
use sotf_host::plugin::{Plugin, ProcessContext};
use std::path::PathBuf;

const SAMPLE_RATE: u32 = 48_000;
const FRAMES: usize = 257;
const BLOCKS: [usize; 4] = [1, 63, 127, 66];
const GUARD: usize = 11;
const CANARY: f32 = f32::from_bits(0x4f12_3456);
const CLAP_ID: &str = "org.spinorama.sotf.crossover";
const VST3_CLASS_ID: &str = "536F746643726F73736F766572303031";
const GLOBAL_CUTOFFS: [f32; 3] = [840.0, 2_100.0, 5_300.0];

#[derive(Clone, Copy)]
struct RouteCase {
    name: &'static str,
    topology: Topology,
    mode: Mode,
    bands: u8,
}

const CASES: [RouteCase; 6] = [
    RouteCase {
        name: "lowpass",
        topology: Topology::Bands,
        mode: Mode::Lowpass,
        bands: 2,
    },
    RouteCase {
        name: "highpass",
        topology: Topology::Bands,
        mode: Mode::Highpass,
        bands: 2,
    },
    RouteCase {
        name: "both-2",
        topology: Topology::Bands,
        mode: Mode::Both,
        bands: 2,
    },
    RouteCase {
        name: "both-3",
        topology: Topology::Bands,
        mode: Mode::Both,
        bands: 3,
    },
    RouteCase {
        name: "both-4",
        topology: Topology::Bands,
        mode: Mode::Both,
        bands: 4,
    },
    RouteCase {
        name: "per-channel",
        topology: Topology::PerChannel,
        mode: Mode::Lowpass,
        bands: 2,
    },
];

#[test]
#[ignore = "requires fresh Crossover artifacts in SOTF_TEST_CROSSOVER_CLAP_PLUGIN and SOTF_TEST_CROSSOVER_VST3_PLUGIN"]
fn loaded_crossover_named_multichannel_routes_match_public_dsp_vectors() {
    let layouts = [
        (InputLayout::Mono, "mono"),
        (InputLayout::Stereo, "stereo"),
        (InputLayout::Quad, "quad"),
        (InputLayout::FiveOne, "5.1"),
        (InputLayout::SevenOne, "7.1"),
        (InputLayout::FiveOneTwo, "5.1.2"),
        (InputLayout::FiveOneFour, "5.1.4"),
        (InputLayout::SevenOneTwo, "7.1.2"),
        (InputLayout::SevenOneFour, "7.1.4"),
        (InputLayout::NineOneFour, "9.1.4"),
        (InputLayout::NineOneSixWide, "9.1.6-wide"),
    ];
    let formats = [
        (
            PluginFormat::Clap,
            "clap",
            CLAP_ID,
            "SOTF_TEST_CROSSOVER_CLAP_PLUGIN",
            OutputLayout::ClapPacked,
        ),
        (
            PluginFormat::Vst3,
            "vst3",
            VST3_CLASS_ID,
            "SOTF_TEST_CROSSOVER_VST3_PLUGIN",
            OutputLayout::Vst3Buses,
        ),
    ];
    let capture_root =
        std::env::var_os("SOTF_CROSSOVER_MULTICHANNEL_CAPTURE_DIR").map(PathBuf::from);
    let mut observed_cases = 0_usize;

    for (format, format_name, plugin_id, library_env, output_layout) in formats {
        let descriptor = descriptor(format, plugin_id, library_env);
        for (layout, layout_name) in layouts {
            let width = layout.channel_count();
            // ExternalPlugin's public buffers are already in canonical SOTF order.
            // The backend applies this native VST3 bus permutation internally; the
            // direct public-DSP reference therefore receives the same caller input.
            let native_bus_to_sotf = if format == PluginFormat::Clap {
                (0..width).collect::<Vec<_>>()
            } else {
                layout.vst3_bus_to_sotf_permutation().to_vec()
            };
            let sotf_input = distinct_input(width);

            for case in CASES {
                let audio_setup = NativePluginAudioSetup::Crossover {
                    input_layout: layout,
                    num_bands: case.bands,
                    topology: case.topology,
                    mode: case.mode,
                    output_layout,
                };
                let mut loaded =
                    ExternalPlugin::new_with_audio_setup(&descriptor, audio_setup, SAMPLE_RATE)
                        .unwrap_or_else(|error| {
                            panic!(
                                "construct {format_name} {layout_name} {} route: {error}",
                                case.name
                            )
                        });

                let mut saved_state = loaded
                    .save_opaque_state()
                    .unwrap_or_else(|error| panic!("save {} state: {error}", case.name));
                set_test_state(&mut saved_state, format, case, width);
                loaded
                    .load_opaque_state(&saved_state)
                    .unwrap_or_else(|error| {
                        panic!(
                            "restore {format_name} {layout_name} {} route state: {error}",
                            case.name
                        )
                    });

                let expected_width = if case.topology == Topology::Bands && case.mode == Mode::Both
                {
                    width * usize::from(case.bands)
                } else {
                    width
                };
                assert_eq!(
                    loaded.input_channels(),
                    width,
                    "{format_name} {layout_name} input"
                );
                assert_eq!(
                    loaded.output_channels(),
                    expected_width,
                    "{format_name} {layout_name} {} output width",
                    case.name
                );

                let expected = render_public_crossover(&sotf_input, width, case);
                let actual = render_loaded(&mut loaded, &sotf_input, width, expected_width);
                assert_waveform_matches(
                    &actual,
                    &expected,
                    &format!("{format_name} {layout_name} {} full vector", case.name),
                );
                assert!(
                    expected.iter().any(|sample| sample.abs() > 1.0e-6),
                    "{format_name} {layout_name} {} reference is nonzero",
                    case.name
                );
                if case.topology == Topology::Bands && case.mode == Mode::Both {
                    for band in 0..usize::from(case.bands) {
                        assert!(
                            (0..FRAMES).any(|frame| {
                                (0..width).any(|channel| {
                                    actual[frame * expected_width + band * width + channel].abs()
                                        > 1.0e-6
                                })
                            }),
                            "{format_name} {layout_name} {} band {band} is nonzero",
                            case.name
                        );
                    }
                }
                if case.topology == Topology::PerChannel {
                    for channel in 0..width {
                        let channel_output = (0..FRAMES)
                            .map(|frame| actual[frame * width + channel])
                            .collect::<Vec<_>>();
                        match channel_mode(channel) {
                            2 => assert!(
                                channel_output.iter().all(|sample| sample.abs() <= 2.0e-5),
                                "{format_name} {layout_name} per-channel mute {channel}"
                            ),
                            3 => {
                                let input_channel = (0..FRAMES)
                                    .map(|frame| sotf_input[frame * width + channel])
                                    .collect::<Vec<_>>();
                                assert_waveform_matches(
                                    &channel_output,
                                    &input_channel,
                                    &format!(
                                        "{format_name} {layout_name} per-channel pass {channel}"
                                    ),
                                );
                            }
                            0 | 1 => assert!(
                                channel_output.iter().any(|sample| sample.abs() > 1.0e-6),
                                "{format_name} {layout_name} filtered channel {channel} is nonzero"
                            ),
                            _ => unreachable!(),
                        }
                    }
                }

                if let Some(root) = &capture_root {
                    capture_case(
                        root,
                        format_name,
                        layout_name,
                        case,
                        &native_bus_to_sotf,
                        &loaded
                            .save_opaque_state()
                            .unwrap_or_else(|error| panic!("capture {} state: {error}", case.name)),
                        &sotf_input,
                        &expected,
                        &actual,
                    );
                }
                observed_cases += 1;
            }
        }
    }

    assert_eq!(
        observed_cases, 132,
        "all format/layout/route cases executed"
    );
    eprintln!("loaded Crossover cases observed: {observed_cases}");
}

#[test]
#[ignore = "requires fresh Crossover artifacts in SOTF_TEST_CROSSOVER_CLAP_PLUGIN and SOTF_TEST_CROSSOVER_VST3_PLUGIN"]
fn loaded_crossover_reconfiguration_and_refusal_preserve_instance_contracts() {
    let formats = [
        (
            PluginFormat::Clap,
            "clap",
            CLAP_ID,
            "SOTF_TEST_CROSSOVER_CLAP_PLUGIN",
            OutputLayout::ClapPacked,
        ),
        (
            PluginFormat::Vst3,
            "vst3",
            VST3_CLASS_ID,
            "SOTF_TEST_CROSSOVER_VST3_PLUGIN",
            OutputLayout::Vst3Buses,
        ),
    ];

    for (format, format_name, plugin_id, library_env, output_layout) in formats {
        let descriptor = descriptor(format, plugin_id, library_env);
        let initial_case = CASES[0];
        let initial_setup = crossover_setup(InputLayout::SevenOne, initial_case, output_layout);
        let mut plugin = configured_instance(
            &descriptor,
            initial_setup.clone(),
            initial_case,
            InputLayout::SevenOne.channel_count(),
            0,
        );

        let transitions = [
            (
                InputLayout::SevenOne,
                RouteCase {
                    name: "transition-highpass",
                    topology: Topology::Bands,
                    mode: Mode::Highpass,
                    bands: 2,
                },
            ),
            (
                InputLayout::SevenOne,
                RouteCase {
                    name: "transition-both-3",
                    topology: Topology::Bands,
                    mode: Mode::Both,
                    bands: 3,
                },
            ),
            (
                InputLayout::SevenOne,
                RouteCase {
                    name: "transition-both-4",
                    topology: Topology::Bands,
                    mode: Mode::Both,
                    bands: 4,
                },
            ),
            (
                InputLayout::SevenOne,
                RouteCase {
                    name: "transition-per-channel",
                    topology: Topology::PerChannel,
                    mode: Mode::Lowpass,
                    bands: 2,
                },
            ),
            (
                // SevenOne and FiveOneTwo have the same width but different
                // speaker identity and VST3 arrangement.
                InputLayout::FiveOneTwo,
                RouteCase {
                    name: "transition-same-width-layout",
                    topology: Topology::PerChannel,
                    mode: Mode::Lowpass,
                    bands: 2,
                },
            ),
        ];

        for (layout, case) in transitions {
            let setup = crossover_setup(layout, case, output_layout);
            plugin
                .reconfigure_audio_setup(setup.clone())
                .unwrap_or_else(|error| {
                    panic!(
                        "{format_name} commit {} on {:?}: {error}",
                        case.name, layout
                    )
                });
            assert_eq!(plugin.audio_setup(), Some(&setup));
            let width = layout.channel_count();
            let expected_width = output_channels(width, case);
            assert_eq!(
                plugin.input_channels(),
                width,
                "{format_name} {} input",
                case.name
            );
            assert_eq!(
                plugin.output_channels(),
                expected_width,
                "{format_name} {} output",
                case.name
            );

            let input = distinct_input(width);
            let expected = render_public_crossover(&input, width, case);
            let actual = render_loaded(&mut plugin, &input, width, expected_width);
            assert_waveform_matches(
                &actual,
                &expected,
                &format!("{format_name} {} committed full vector", case.name),
            );
        }

        assert_refused_native_state_is_transactional(
            &descriptor,
            format,
            format_name,
            output_layout,
        );
        assert_fir_per_channel_reconfiguration_is_transactional(
            &descriptor,
            format,
            format_name,
            output_layout,
        );
        assert_same_setup_rejects_native_structural_mismatch(
            &descriptor,
            format,
            format_name,
            output_layout,
            1,
        );
    }
}

#[test]
#[ignore = "requires fresh Crossover artifacts in SOTF_TEST_CROSSOVER_CLAP_PLUGIN and SOTF_TEST_CROSSOVER_VST3_PLUGIN"]
fn loaded_crossover_reconciles_width_changing_both_drift() {
    let formats = [
        (
            PluginFormat::Clap,
            "clap",
            CLAP_ID,
            "SOTF_TEST_CROSSOVER_CLAP_PLUGIN",
            OutputLayout::ClapPacked,
        ),
        (
            PluginFormat::Vst3,
            "vst3",
            VST3_CLASS_ID,
            "SOTF_TEST_CROSSOVER_VST3_PLUGIN",
            OutputLayout::Vst3Buses,
        ),
    ];
    for (format, format_name, plugin_id, library_env, output_layout) in formats {
        let descriptor = descriptor(format, plugin_id, library_env);
        assert_same_setup_rejects_native_structural_mismatch(
            &descriptor,
            format,
            format_name,
            output_layout,
            2,
        );
    }
}

#[test]
#[ignore = "requires fresh Crossover artifacts in SOTF_TEST_CROSSOVER_CLAP_PLUGIN and SOTF_TEST_CROSSOVER_VST3_PLUGIN"]
fn loaded_crossover_recovers_invalid_fir_per_channel_drift() {
    let formats = [
        (
            PluginFormat::Clap,
            "clap",
            CLAP_ID,
            "SOTF_TEST_CROSSOVER_CLAP_PLUGIN",
            OutputLayout::ClapPacked,
        ),
        (
            PluginFormat::Vst3,
            "vst3",
            VST3_CLASS_ID,
            "SOTF_TEST_CROSSOVER_VST3_PLUGIN",
            OutputLayout::Vst3Buses,
        ),
    ];
    for (format, format_name, plugin_id, library_env, output_layout) in formats {
        let descriptor = descriptor(format, plugin_id, library_env);
        assert_invalid_native_fir_per_channel_recovers_to_typed_setup(
            &descriptor,
            output_layout,
            format_name,
        );
    }
}

fn crossover_setup(
    layout: InputLayout,
    case: RouteCase,
    output_layout: OutputLayout,
) -> NativePluginAudioSetup {
    NativePluginAudioSetup::Crossover {
        input_layout: layout,
        num_bands: case.bands,
        topology: case.topology,
        mode: case.mode,
        output_layout,
    }
}

fn configured_instance(
    descriptor: &PluginDescriptor,
    setup: NativePluginAudioSetup,
    case: RouteCase,
    width: usize,
    family_index: i32,
) -> ExternalPlugin {
    let mut plugin = ExternalPlugin::new_with_audio_setup(descriptor, setup, SAMPLE_RATE)
        .unwrap_or_else(|error| panic!("construct loaded Crossover candidate: {error}"));
    let mut state = plugin
        .save_opaque_state()
        .unwrap_or_else(|error| panic!("save initial Crossover state: {error}"));
    set_test_state(&mut state, descriptor.format, case, width);
    let mut parsed = parse_native_state(&state, descriptor.format);
    set_state_value(&mut parsed, "family", json!({"i32": family_index}));
    // Keep dormant per-channel controls populated too. A later topology
    // transition must reveal persisted values rather than constructor defaults.
    for channel in 0..width {
        set_state_value(
            &mut parsed,
            &format!("channel_frequency_{channel}"),
            json!({"f32": channel_frequency(channel)}),
        );
        set_state_value(
            &mut parsed,
            &format!("channel_mode_{channel}"),
            json!({"i32": channel_mode(channel)}),
        );
    }
    state = encode_native_state(&parsed, descriptor.format);
    plugin
        .load_opaque_state(&state)
        .unwrap_or_else(|error| panic!("load configured Crossover state: {error}"));
    plugin
}

fn assert_refused_native_state_is_transactional(
    descriptor: &PluginDescriptor,
    format: PluginFormat,
    format_name: &str,
    output_layout: OutputLayout,
) {
    let case = RouteCase {
        name: "transactional-state-refusal",
        topology: Topology::PerChannel,
        mode: Mode::Lowpass,
        bands: 2,
    };
    let setup = crossover_setup(InputLayout::SevenOne, case, output_layout);
    let width = InputLayout::SevenOne.channel_count();
    let mut loaded = configured_instance(descriptor, setup.clone(), case, width, 0);
    let mut twin = configured_instance(descriptor, setup.clone(), case, width, 0);

    let warm_input = distinct_input(width);
    let warm_actual = render_loaded(&mut loaded, &warm_input, width, width);
    let warm_twin = render_loaded(&mut twin, &warm_input, width, width);
    assert_waveform_exact(
        &warm_actual,
        &warm_twin,
        &format!("{format_name} populated twin warmup"),
    );

    let old_state = loaded
        .save_opaque_state()
        .unwrap_or_else(|error| panic!("{format_name} save populated state: {error}"));
    let mut conflicting_state = old_state.clone();
    let mut parsed = parse_native_state(&conflicting_state, format);
    set_state_value(&mut parsed, "mode", json!({"i32": 2}));
    conflicting_state = encode_native_state(&parsed, format);
    let error = loaded
        .load_opaque_state(&conflicting_state)
        .expect_err("typed Lowpass setup must reject opaque Both state");
    assert!(
        error.contains("audio setup conflicts with native Crossover structural parameters"),
        "{format_name} reports native candidate mismatch: {error}"
    );
    assert_eq!(loaded.audio_setup(), Some(&setup));
    assert_eq!(loaded.input_channels(), width);
    assert_eq!(loaded.output_channels(), width);
    assert_eq!(
        loaded.save_opaque_state().unwrap(),
        old_state,
        "{format_name} rejected state leaves active persisted state intact"
    );

    let silence = vec![0.0; FRAMES * width];
    let retained = render_loaded(&mut loaded, &silence, width, width);
    let twin_continuation = render_loaded(&mut twin, &silence, width, width);
    assert_waveform_exact(
        &retained,
        &twin_continuation,
        &format!("{format_name} rejected state retains recursive history"),
    );

    let mut cold = configured_instance(descriptor, setup, case, width, 0);
    cold.load_opaque_state(&old_state)
        .unwrap_or_else(|error| panic!("{format_name} rebuild cold reference: {error}"));
    let cold_output = render_loaded(&mut cold, &silence, width, width);
    assert!(
        max_abs_error(&retained, &cold_output) > 1.0e-6,
        "{format_name} retained recursive history differs from a fresh restored instance"
    );
}

fn assert_fir_per_channel_reconfiguration_is_transactional(
    descriptor: &PluginDescriptor,
    _format: PluginFormat,
    format_name: &str,
    output_layout: OutputLayout,
) {
    let starting_case = CASES[0];
    let setup = crossover_setup(InputLayout::SevenOne, starting_case, output_layout);
    let width = InputLayout::SevenOne.channel_count();
    let mut loaded = configured_instance(descriptor, setup.clone(), starting_case, width, 1);
    let mut twin = configured_instance(descriptor, setup.clone(), starting_case, width, 1);
    let warm_input = distinct_input(width);
    render_loaded(&mut loaded, &warm_input, width, width);
    render_loaded(&mut twin, &warm_input, width, width);
    let state_before = loaded.save_opaque_state().unwrap();

    let per_channel = RouteCase {
        name: "linear-phase-per-channel-refusal",
        topology: Topology::PerChannel,
        mode: Mode::Lowpass,
        bands: 2,
    };
    let rejected_setup = crossover_setup(InputLayout::SevenOne, per_channel, output_layout);
    let error = loaded
        .reconfigure_audio_setup(rejected_setup)
        .expect_err("FIR LinearPhase must reject PerChannel topology");
    eprintln!("{format_name} FIR PerChannel candidate rejection: {error}");
    assert_eq!(loaded.audio_setup(), Some(&setup));
    assert_eq!(loaded.input_channels(), width);
    assert_eq!(loaded.output_channels(), width);
    assert_eq!(loaded.save_opaque_state().unwrap(), state_before);

    let silence = vec![0.0; FRAMES * width];
    let after_refusal = render_loaded(&mut loaded, &silence, width, width);
    let twin_after = render_loaded(&mut twin, &silence, width, width);
    assert_waveform_exact(
        &after_refusal,
        &twin_after,
        &format!("{format_name} FIR refusal preserves live instance"),
    );
    let mut cold = configured_instance(descriptor, setup, starting_case, width, 1);
    cold.load_opaque_state(&state_before)
        .unwrap_or_else(|error| panic!("{format_name} rebuild cold FIR reference: {error}"));
    let cold_output = render_loaded(&mut cold, &silence, width, width);
    assert!(
        max_abs_error(&after_refusal, &cold_output) > 1.0e-6,
        "{format_name} FIR refusal retains history beyond a fresh restored instance"
    );
}

fn assert_same_setup_rejects_native_structural_mismatch(
    descriptor: &PluginDescriptor,
    _format: PluginFormat,
    format_name: &str,
    output_layout: OutputLayout,
    target_mode_index: i32,
) {
    let case = CASES[0];
    let setup = crossover_setup(InputLayout::SevenOne, case, output_layout);
    let width = InputLayout::SevenOne.channel_count();
    let mut loaded = configured_instance(descriptor, setup.clone(), case, width, 0);
    let mode_id = loaded
        .parameters()
        .into_iter()
        .map(|parameter| parameter.id)
        .find(|id| id.as_str().ends_with(".3357091"))
        .unwrap_or_else(|| panic!("{format_name} exposes native Crossover mode ID"));
    let original_mode = loaded.get_parameter(&mode_id).unwrap();
    let target_mode = match original_mode {
        ParameterValue::Int(_) => ParameterValue::Int(target_mode_index),
        other => {
            panic!("{format_name} mode parameter must encode Both as an integer, got {other:?}")
        }
    };
    loaded
        .set_parameter(mode_id.clone(), target_mode.clone())
        .unwrap_or_else(|error| {
            panic!("{format_name} set structural mode for mismatch test: {error}")
        });
    assert_eq!(loaded.get_parameter(&mode_id), Some(target_mode.clone()));

    // Flush the queued control through the actual native process callback
    // before testing the stored-setup shortcut. A host-side pending value alone
    // does not prove that the native structural state changed.
    let process_input = vec![0.0; width];
    let mut process_output = vec![0.0; width];
    let parameter_process = loaded.process(
        &process_input,
        &mut process_output,
        &ProcessContext::new(SAMPLE_RATE, 1),
    );
    assert!(
        parameter_process.is_err(),
        "{format_name} native process refuses the unapplied structural change: {parameter_process:?}"
    );
    assert_eq!(loaded.get_parameter(&mode_id), Some(target_mode.clone()));

    loaded
        .reconfigure_audio_setup(setup.clone())
        .unwrap_or_else(|error| {
            panic!("{format_name} stored setup reconciles native structural state: {error}")
        });
    assert_eq!(loaded.audio_setup(), Some(&setup));
    assert_eq!(loaded.input_channels(), width);
    assert_eq!(loaded.output_channels(), width);
    assert_eq!(loaded.get_parameter(&mode_id), Some(original_mode));
    let input = distinct_input(width);
    let expected = render_public_crossover(&input, width, case);
    let output = render_loaded(&mut loaded, &input, width, width);
    assert_waveform_matches(
        &output,
        &expected,
        &format!("{format_name} stored setup restores a complete native process path"),
    );
}

fn assert_invalid_native_fir_per_channel_recovers_to_typed_setup(
    descriptor: &PluginDescriptor,
    output_layout: OutputLayout,
    format_name: &str,
) {
    let case = CASES[0];
    let setup = crossover_setup(InputLayout::SevenOne, case, output_layout);
    let width = InputLayout::SevenOne.channel_count();
    let mut loaded = configured_instance(descriptor, setup.clone(), case, width, 1);
    let mut twin = configured_instance(descriptor, setup.clone(), case, width, 1);
    let warm_input = distinct_input(width);
    let warm_output = render_loaded(&mut loaded, &warm_input, width, width);
    let twin_warm_output = render_loaded(&mut twin, &warm_input, width, width);
    assert_waveform_exact(
        &warm_output,
        &twin_warm_output,
        &format!("{format_name} FIR recovery twin warmup"),
    );
    let original_state = loaded
        .save_opaque_state()
        .unwrap_or_else(|error| panic!("{format_name} save valid FIR state: {error}"));

    let topology_id = loaded
        .parameters()
        .into_iter()
        .map(|parameter| parameter.id)
        .find(|id| id.as_str().ends_with(".1196016239"))
        .unwrap_or_else(|| panic!("{format_name} exposes native Crossover topology ID"));
    let original_topology = loaded.get_parameter(&topology_id).unwrap();
    let invalid_topology = match original_topology {
        ParameterValue::Bool(_) => ParameterValue::Bool(true),
        ParameterValue::Int(_) => ParameterValue::Int(1),
        other => panic!("{format_name} topology parameter is discrete, got {other:?}"),
    };
    loaded
        .set_parameter(topology_id.clone(), invalid_topology.clone())
        .unwrap_or_else(|error| {
            panic!("{format_name} queue FIR PerChannel structural drift: {error}")
        });
    assert_eq!(
        loaded.get_parameter(&topology_id),
        Some(invalid_topology.clone())
    );

    let process_input = vec![0.0; width];
    let mut process_output = vec![0.0; width];
    let parameter_process = loaded.process(
        &process_input,
        &mut process_output,
        &ProcessContext::new(SAMPLE_RATE, 1),
    );
    assert!(
        parameter_process.is_err(),
        "{format_name} native process refuses unsupported FIR PerChannel structure: {parameter_process:?}"
    );
    let invalid_state = loaded
        .save_opaque_state()
        .unwrap_or_else(|error| panic!("{format_name} save invalid native structure: {error}"));
    assert_eq!(loaded.audio_setup(), Some(&setup));
    assert_eq!(loaded.input_channels(), width);
    assert_eq!(loaded.output_channels(), width);

    let recovery = loaded.reconfigure_audio_setup(setup.clone());
    if let Err(error) = recovery {
        assert_eq!(loaded.audio_setup(), Some(&setup));
        assert_eq!(loaded.input_channels(), width);
        assert_eq!(loaded.output_channels(), width);
        assert_eq!(
            loaded.save_opaque_state().unwrap(),
            invalid_state,
            "{format_name} failed recovery leaves the active invalid state untouched"
        );
        panic!("{format_name} valid typed setup must recover from FIR PerChannel drift: {error}");
    }

    assert_eq!(loaded.audio_setup(), Some(&setup));
    assert_eq!(loaded.input_channels(), width);
    assert_eq!(loaded.output_channels(), width);
    assert_eq!(loaded.get_parameter(&topology_id), Some(original_topology));
    assert_eq!(
        loaded.save_opaque_state().unwrap(),
        original_state,
        "{format_name} recovery restores the valid setup and preserves FIR controls"
    );

    let recovery_input = distinct_input(width);
    let recovered_output = render_loaded(&mut loaded, &recovery_input, width, width);
    let mut fresh_reference = configured_instance(descriptor, setup, case, width, 1);
    let reference_output = render_loaded(&mut fresh_reference, &recovery_input, width, width);
    assert_waveform_exact(
        &recovered_output,
        &reference_output,
        &format!("{format_name} FIR recovery full fresh-reference vector"),
    );
}

fn descriptor(format: PluginFormat, id: &str, library_env: &str) -> PluginDescriptor {
    PluginDescriptor {
        id: id.into(),
        name: "SOTF: Crossover".into(),
        vendor: "SOTF".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        format,
        path: PathBuf::from(
            std::env::var_os(library_env).unwrap_or_else(|| panic!("{library_env} must be set")),
        ),
        // Scanned metadata is deliberately left at the exported default. The explicit
        // typed setup chooses this instance's actual named input/output topology.
        audio_inputs: 2,
        audio_outputs: 2,
        is_instrument: false,
        categories: vec!["audio-effect".into()],
        scan_status: PluginScanStatus::Loadable,
    }
}

fn parse_native_state(opaque: &[u8], format: PluginFormat) -> Value {
    let payload = if format == PluginFormat::Clap {
        let prefix: [u8; 8] = opaque
            .get(..8)
            .expect("CLAP state has its byte-length prefix")
            .try_into()
            .unwrap();
        let payload = opaque.get(8..).unwrap();
        assert_eq!(
            payload.len(),
            usize::try_from(u64::from_le_bytes(prefix)).unwrap()
        );
        payload
    } else {
        opaque
    };
    serde_json::from_slice(payload).expect("native Crossover state is JSON")
}

fn encode_native_state(state: &Value, format: PluginFormat) -> Vec<u8> {
    let payload = serde_json::to_vec(state).unwrap();
    if format == PluginFormat::Clap {
        let mut bytes = (payload.len() as u64).to_le_bytes().to_vec();
        bytes.extend(payload);
        bytes
    } else {
        payload
    }
}

fn set_state_value(state: &mut Value, id: &str, value: Value) {
    state["params"][id] = value;
}

fn set_test_state(state_bytes: &mut Vec<u8>, format: PluginFormat, case: RouteCase, width: usize) {
    let mut state = parse_native_state(state_bytes, format);
    set_state_value(&mut state, "family", json!({"i32": 0}));
    set_state_value(&mut state, "frequency", json!({"f32": GLOBAL_CUTOFFS[0]}));
    set_state_value(&mut state, "frequency_2", json!({"f32": GLOBAL_CUTOFFS[1]}));
    set_state_value(&mut state, "frequency_3", json!({"f32": GLOBAL_CUTOFFS[2]}));
    set_state_value(
        &mut state,
        "mode",
        json!({"i32": match case.mode { Mode::Lowpass => 0, Mode::Highpass => 1, Mode::Both => 2 }}),
    );
    set_state_value(
        &mut state,
        "topology",
        json!({"i32": match case.topology { Topology::Bands => 0, Topology::PerChannel => 1 }}),
    );
    set_state_value(
        &mut state,
        "band_count",
        json!({"i32": i32::from(case.bands - 2)}),
    );
    if case.topology == Topology::PerChannel {
        for channel in 0..width {
            set_state_value(
                &mut state,
                &format!("channel_frequency_{channel}"),
                json!({"f32": channel_frequency(channel)}),
            );
            set_state_value(
                &mut state,
                &format!("channel_mode_{channel}"),
                json!({"i32": channel_mode(channel)}),
            );
        }
    }
    *state_bytes = encode_native_state(&state, format);
}

fn channel_frequency(channel: usize) -> f32 {
    340.0 + channel as f32 * 287.0
}

fn channel_mode(channel: usize) -> i32 {
    (channel % 4) as i32
}

fn channel_mode_name(channel: usize) -> &'static str {
    match channel_mode(channel) {
        0 => "lowpass",
        1 => "highpass",
        2 => "mute",
        3 => "passthrough",
        _ => unreachable!(),
    }
}

fn distinct_input(channels: usize) -> Vec<f32> {
    (0..FRAMES)
        .flat_map(|frame| {
            (0..channels).map(move |channel| {
                let time = frame as f32 / SAMPLE_RATE as f32;
                let phase = channel as f32 * 0.173;
                let scale = 0.55 + channel as f32 * 0.021;
                let sample = (2.0 * std::f32::consts::PI * (127.0 + channel as f32 * 13.0) * time
                    + phase)
                    .sin()
                    * 0.11
                    + (2.0 * std::f32::consts::PI * 1_240.0 * time + phase * 0.71).sin() * 0.047
                    + (2.0 * std::f32::consts::PI * 4_100.0 * time - phase * 0.37).cos() * 0.031;
                sample * scale
            })
        })
        .collect()
}

fn render_public_crossover(input: &[f32], channels: usize, case: RouteCase) -> Vec<f32> {
    let extras = match case.bands {
        2 => Vec::new(),
        3 => vec![f64::from(GLOBAL_CUTOFFS[1])],
        4 => vec![f64::from(GLOBAL_CUTOFFS[1]), f64::from(GLOBAL_CUTOFFS[2])],
        bands => panic!("unsupported test band count {bands}"),
    };
    let channel_frequencies = (0..channels).map(channel_frequency).collect::<Vec<_>>();
    let channel_modes = (0..channels)
        .map(channel_mode_name)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let config = json!({
        "type": "LR24",
        "frequency": GLOBAL_CUTOFFS[0],
        "output": match case.mode { Mode::Lowpass => "lowpass", Mode::Highpass => "highpass", Mode::Both => "both" },
        "topology": match case.topology { Topology::Bands => "bands", Topology::PerChannel => "per_channel" },
        "band_count": case.bands,
        "extra_frequencies": extras,
        "channel_frequencies_hz": channel_frequencies,
        "channel_modes": channel_modes,
    });
    let mut reference = sotf_plugins::create_plugin("Crossover", &config, channels, SAMPLE_RATE)
        .unwrap_or_else(|error| panic!("construct public Crossover reference: {error}"));
    reference
        .initialize(SAMPLE_RATE)
        .unwrap_or_else(|error| panic!("initialize public Crossover reference: {error}"));
    render_plugin(
        reference.as_mut(),
        input,
        channels,
        output_channels(channels, case),
    )
}

fn output_channels(channels: usize, case: RouteCase) -> usize {
    if case.topology == Topology::Bands && case.mode == Mode::Both {
        channels * usize::from(case.bands)
    } else {
        channels
    }
}

fn render_loaded(
    plugin: &mut dyn Plugin,
    input: &[f32],
    input_channels: usize,
    output_channels: usize,
) -> Vec<f32> {
    render_plugin(plugin, input, input_channels, output_channels)
}

fn render_plugin(
    plugin: &mut dyn Plugin,
    input: &[f32],
    input_channels: usize,
    output_channels: usize,
) -> Vec<f32> {
    assert_eq!(input.len(), FRAMES * input_channels);
    assert_eq!(BLOCKS.iter().sum::<usize>(), FRAMES);
    let output_len = FRAMES * output_channels;
    let mut guarded = vec![CANARY; output_len + GUARD * 2];
    guarded[GUARD..GUARD + output_len].fill(f32::NAN);

    let mut frame_offset = 0;
    for block_frames in BLOCKS {
        let input_start = frame_offset * input_channels;
        let input_end = input_start + block_frames * input_channels;
        let output_start = GUARD + frame_offset * output_channels;
        let output_end = output_start + block_frames * output_channels;
        assert_eq!(
            plugin
                .process(
                    &input[input_start..input_end],
                    &mut guarded[output_start..output_end],
                    &ProcessContext::new(SAMPLE_RATE, block_frames),
                )
                .unwrap_or_else(|error| panic!("process Crossover route: {error}")),
            block_frames
        );
        frame_offset += block_frames;
    }
    assert_eq!(frame_offset, FRAMES);
    assert!(
        guarded[..GUARD]
            .iter()
            .chain(&guarded[GUARD + output_len..])
            .all(|sample| sample.to_bits() == CANARY.to_bits())
    );
    let output = guarded[GUARD..GUARD + output_len].to_vec();
    assert!(output.iter().all(|sample| sample.is_finite()));
    output
}

fn max_abs_error(left: &[f32], right: &[f32]) -> f32 {
    assert_eq!(left.len(), right.len());
    left.iter()
        .zip(right)
        .map(|(left, right)| (left - right).abs())
        .fold(0.0_f32, f32::max)
}

fn assert_waveform_matches(actual: &[f32], expected: &[f32], label: &str) {
    let peak = max_abs_error(actual, expected);
    assert!(peak <= 2.0e-5, "{label}: max error {peak}");
}

fn assert_waveform_exact(actual: &[f32], expected: &[f32], label: &str) {
    assert_eq!(
        actual.len(),
        expected.len(),
        "{label}: sample count differs"
    );
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(
            actual.to_bits(),
            expected.to_bits(),
            "{label}: sample {index} differs ({actual} vs {expected})"
        );
    }
}

fn capture_case(
    root: &std::path::Path,
    format: &str,
    layout: &str,
    case: RouteCase,
    native_bus_to_sotf: &[usize],
    state: &[u8],
    input: &[f32],
    expected: &[f32],
    actual: &[f32],
) {
    let path = root.join(format).join(layout).join(case.name);
    std::fs::create_dir_all(&path).unwrap();
    write_f32le(&path.join("input-sotf-order.f32le"), input);
    write_f32le(&path.join("expected-public-dsp.f32le"), expected);
    write_f32le(&path.join("actual-loaded-host.f32le"), actual);
    std::fs::write(path.join("native-state.bin"), state).unwrap();
    let metadata = json!({
        "format": format,
        "layout": layout,
        "channels": native_bus_to_sotf.len(),
        "consumer_buffer_order": "canonical SOTF channel order",
        "native_bus_to_sotf": native_bus_to_sotf,
        "case": case.name,
        "topology": match case.topology { Topology::Bands => "bands", Topology::PerChannel => "per_channel" },
        "mode": match case.mode { Mode::Lowpass => "lowpass", Mode::Highpass => "highpass", Mode::Both => "both" },
        "bands": case.bands,
        "sample_rate": SAMPLE_RATE,
        "frames": FRAMES,
        "callback_frames": BLOCKS,
        "oracle_scope": "public Crossover DSP composition; routing/state composition reference, not coefficient-level oracle",
    });
    std::fs::write(
        path.join("case.json"),
        serde_json::to_vec_pretty(&metadata).unwrap(),
    )
    .unwrap();
}

fn write_f32le(path: &std::path::Path, values: &[f32]) {
    let mut bytes = Vec::with_capacity(values.len() * std::mem::size_of::<f32>());
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}
