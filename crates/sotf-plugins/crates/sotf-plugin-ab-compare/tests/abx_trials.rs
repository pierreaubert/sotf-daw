//! Public blinded audition, frozen matching, and realtime switching contracts.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, Plugin, ProcessContext};
use sotf_plugin_ab_compare::{
    ABCompareData, ABComparePlugin, ABComparePluginParams, AbxMonitor, AbxPath, PathConfig,
};

#[global_allocator]
static ALLOCATOR: sotf_host::test_utils::CountingAlloc = sotf_host::test_utils::CountingAlloc;

fn prepared(rate: u32, auto_gain: bool) -> ABComparePlugin {
    let mut plugin = ABComparePlugin::from_params(
        1,
        ABComparePluginParams {
            path_b: PathConfig::Plugin {
                plugin_type: "gain".into(),
                parameters: serde_json::json!({"gain_db": -12.0}),
            },
            auto_gain_enabled: auto_gain,
            mix: -1.0,
            mix_transition_ms: 1.0,
            ..Default::default()
        },
    )
    .unwrap();
    plugin.initialize(f64::from(rate)).unwrap();
    plugin
}

fn render(plugin: &mut ABComparePlugin, input: &[f32], rate: u32, pattern: &[usize]) -> Vec<f32> {
    let mut output = vec![0.0; input.len()];
    let mut position = 0;
    for &size in pattern.iter().cycle() {
        let count = size.min(input.len() - position);
        if count == 0 {
            break;
        }
        plugin
            .process(
                &input[position..position + count],
                &mut output[position..position + count],
                &ProcessContext::new(rate, count),
            )
            .unwrap();
        position += count;
    }
    output
}

#[test]
fn x_matches_revealed_reference_and_has_no_parameter_or_meter_identity() {
    for rate in [44_100, 48_000, 96_000] {
        let mut plugin = prepared(rate, false);
        plugin.begin_abx_trial().unwrap();
        assert_eq!(plugin.abx_monitor(), Some(AbxMonitor::X));
        assert!(plugin.begin_abx_trial().is_err());
        let output = render(
            &mut plugin,
            &vec![0.25; rate as usize / 4],
            rate,
            &[7, 257, 8192],
        );
        let data = plugin.get_data().unwrap();
        let data = data.downcast_ref::<ABCompareData>().unwrap();
        assert_eq!(
            data.current_mix, 0.0,
            "meter must not expose the X crossfade endpoint"
        );
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("mix")),
            Some(ParameterValue::Float(-1.0))
        );
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("selected_path")),
            Some(ParameterValue::Int(0))
        );
        for id in [
            "mix",
            "bypass",
            "difference_mode",
            "auto_gain_enabled",
            "phase_invert_a",
        ] {
            assert!(
                plugin
                    .set_parameter(ParameterId::from(id), ParameterValue::Bool(true))
                    .is_err()
            );
        }
        assert!(plugin.initialize(f64::from(rate) * 2.0).is_err());
        let result = plugin.submit_abx_guess(AbxPath::A).unwrap();
        assert_eq!(result.correct, result.reference == AbxPath::A);
        let expected = 0.25
            * if result.reference == AbxPath::A {
                1.0
            } else {
                10.0_f32.powf(-12.0 / 20.0)
            };
        assert!((output[output.len() - 1] - expected).abs() < 1e-6);
        assert_eq!(plugin.abx_monitor(), None);
        assert!(plugin.submit_abx_guess(AbxPath::B).is_err());
    }
}

#[test]
fn judged_excerpt_freezes_moving_gain_and_resumes_it_after_submission() {
    let rate = 48_000;
    let mut plugin = prepared(rate, true);
    let signal: Vec<f32> = (0..24_000)
        .map(|i| 0.1 * (std::f32::consts::TAU * 997.0 * i as f32 / rate as f32).sin())
        .collect();
    render(&mut plugin, &signal, rate, &[257]);
    plugin.begin_abx_trial().unwrap();
    plugin.select_abx_monitor(AbxMonitor::B).unwrap();
    render(&mut plugin, &signal, rate, &[257]);
    let frozen = plugin
        .get_data()
        .unwrap()
        .downcast_ref::<ABCompareData>()
        .unwrap()
        .auto_gain_db;
    assert!(
        frozen > 0.1 && frozen < 11.9,
        "warmup must leave a moving gain: {frozen}"
    );
    render(&mut plugin, &signal, rate, &[8192]);
    let still_frozen = plugin
        .get_data()
        .unwrap()
        .downcast_ref::<ABCompareData>()
        .unwrap()
        .auto_gain_db;
    assert_eq!(
        frozen, still_frozen,
        "both measurement and gain smoothing must pause"
    );
    plugin.submit_abx_guess(AbxPath::B).unwrap();
    render(&mut plugin, &signal, rate, &[257]);
    let resumed = plugin
        .get_data()
        .unwrap()
        .downcast_ref::<ABCompareData>()
        .unwrap()
        .auto_gain_db;
    assert!(
        resumed > frozen + 0.1,
        "matching must resume: {frozen} -> {resumed}"
    );
}

#[test]
fn reference_switches_are_partition_invariant_and_bounded_per_sample() {
    let run = |pattern: &[usize]| {
        let mut plugin = prepared(48_000, false);
        plugin.begin_abx_trial().unwrap();
        let mut output = Vec::new();
        for selection in [AbxMonitor::A, AbxMonitor::B, AbxMonitor::A, AbxMonitor::B] {
            plugin.select_abx_monitor(selection).unwrap();
            output.extend(render(&mut plugin, &vec![0.25; 4096], 48_000, pattern));
        }
        output
    };
    let reference = run(&[1]);
    for pattern in [&[8192][..], &[7, 31, 257][..]] {
        let actual = run(pattern);
        assert_eq!(actual, reference);
    }
    let largest_step = reference
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        largest_step < 0.005,
        "smoothed 1 ms switch must not step the full path difference: {largest_step}"
    );
}

#[test]
fn trial_switching_processing_and_completion_do_not_allocate() {
    let mut plugin = prepared(48_000, true);
    plugin.begin_abx_trial().unwrap(); // RNG belongs to the control thread.
    let input = vec![0.1; 8192];
    let mut output = vec![0.0; input.len()];
    let context = ProcessContext::new(48_000, input.len());
    sotf_host::test_utils::assert_no_allocs("ABX switching and judged rendering", || {
        for monitor in [AbxMonitor::A, AbxMonitor::B, AbxMonitor::X] {
            plugin.select_abx_monitor(monitor).unwrap();
            plugin.process(&input, &mut output, &context).unwrap();
        }
        plugin.submit_abx_guess(AbxPath::A).unwrap();
    });
    plugin.begin_abx_trial().unwrap();
    plugin.reset();
    assert_eq!(plugin.abx_monitor(), None);
    plugin
        .set_parameter(ParameterId::from("mix"), ParameterValue::Float(0.0))
        .unwrap();
    plugin.begin_abx_trial().unwrap();
    plugin.cancel_abx_trial();
    assert_eq!(plugin.abx_monitor(), None);
}
