//! Verify host PDC uses automatic FIR support in actual parallel-branch audio.

// Rust guideline compliant 2026-02-21
use plugins_bridge::create_plugin;
use sotf_host::{DawHost, GraphEdge, Plugin, ProcessContext};
use sotf_plugin_linear_phase_eq::LinearPhaseEqDynamicPlugin;

const RATE: u32 = 96_000;
const CHANNELS: usize = 2;
const CONFIG: &str = r#"{
    "num_filters": 3,
    "fir_length_index": 0,
    "accuracy_mode": true,
    "phase_mode_index": 0,
    "auto_gain": false,
    "mix": 1.0,
    "filters": [
        {"filter_type":"Peak","frequency":500.0,"q":2.0,"gain_db":9.0},
        {"filter_type":"Peak","frequency":2000.0,"q":1.5,"gain_db":-6.0},
        {"filter_type":"Lowshelf","frequency":150.0,"gain_db":3.0}
    ]
}"#;

fn unity() -> Box<dyn Plugin> {
    create_plugin("Gain", CHANNELS, f64::from(RATE), r#"{"gain_db":0.0}"#).unwrap()
}

#[test]
fn parallel_dry_branch_aligns_with_resolved_automatic_fir_latency() {
    let mut wet = create_plugin("LinearPhaseEQ", CHANNELS, f64::from(RATE), CONFIG).unwrap();
    let report = wet
        .as_any()
        .unwrap()
        .downcast_ref::<LinearPhaseEqDynamicPlugin>()
        .unwrap()
        .inner()
        .accuracy_report()
        .unwrap();
    let manual_delay = 1024 / 2 + 32;
    let actual_delay = report.selected_taps / 2 + 32;
    assert_eq!(report.manual_latency_samples, manual_delay);
    assert!(report.selected_taps > 1024);
    assert!(actual_delay > manual_delay);
    assert_eq!(report.latency_samples, actual_delay);
    wet.initialize(f64::from(RATE)).unwrap();

    let mut host = DawHost::new(CHANNELS, RATE);
    let input_node = host.add_node("input".into(), unity()).unwrap();
    let wet_node = host
        .add_node(
            "automatic EQ".into(),
            create_plugin("LinearPhaseEQ", CHANNELS, f64::from(RATE), CONFIG).unwrap(),
        )
        .unwrap();
    let dry_node = host.add_node("dry".into(), unity()).unwrap();
    let merge_node = host.add_node("sum".into(), unity()).unwrap();
    for (from, to) in [
        (input_node, wet_node),
        (input_node, dry_node),
        (wet_node, merge_node),
        (dry_node, merge_node),
    ] {
        host.add_edge(GraphEdge::new(from, to)).unwrap();
    }
    host.build().unwrap();
    assert_eq!(host.total_latency_samples(), actual_delay);

    let source_frames = 317;
    // Two channel-distinct markers check both initial alignment and the final
    // source sample. Zero continuation covers the entire causal FIR support.
    let total_frames = source_frames + report.selected_taps + 32 + 29;
    let mut source = vec![0.0_f32; total_frames * CHANNELS];
    source[0] = 0.25;
    source[1] = -0.125;
    source[(source_frames - 1) * CHANNELS] = -0.0625;
    source[(source_frames - 1) * CHANNELS + 1] = 0.1875;

    let mut reference_wet = vec![f32::NAN; source.len()];
    let mut cursor = 0;
    for frames in [127, 1, 509, 31, 1024, 7].into_iter().cycle() {
        if cursor == total_frames {
            break;
        }
        let end = (cursor + frames).min(total_frames);
        let range = cursor * CHANNELS..end * CHANNELS;
        assert_eq!(
            wet.process(
                &source[range.clone()],
                &mut reference_wet[range],
                &ProcessContext::new(RATE, end - cursor)
            )
            .unwrap(),
            end - cursor,
        );
        cursor = end;
    }
    let mut actual = vec![f32::NAN; source.len()];
    cursor = 0;
    // Deliberately differ from the standalone partitioning so a common callback
    // boundary mistake cannot hide behind an identical reference schedule.
    for frames in [1, 257, 13, 1024, 31, 7, 509].into_iter().cycle() {
        if cursor == total_frames {
            break;
        }
        let end = (cursor + frames).min(total_frames);
        let range = cursor * CHANNELS..end * CHANNELS;
        assert_eq!(
            host.process(&source[range.clone()], &mut actual[range])
                .unwrap(),
            end - cursor
        );
        cursor = end;
    }

    let mut maximum_error = 0.0_f64;
    let mut wrong_manual_error = 0.0_f64;
    for frame in 0..total_frames {
        for channel in 0..CHANNELS {
            let index = frame * CHANNELS + channel;
            let dry = if frame >= actual_delay {
                source[(frame - actual_delay) * CHANNELS + channel]
            } else {
                0.0
            };
            let manual_dry = if frame >= manual_delay {
                source[(frame - manual_delay) * CHANNELS + channel]
            } else {
                0.0
            };
            let expected = f64::from(reference_wet[index]) + f64::from(dry);
            let observed = f64::from(actual[index]);
            assert!(observed.is_finite() && expected.is_finite());
            maximum_error = maximum_error.max((observed - expected).abs());
            wrong_manual_error = wrong_manual_error
                .max((observed - f64::from(reference_wet[index]) - f64::from(manual_dry)).abs());
        }
    }
    // The allowance covers independent f32 convolution/mixing roundoff, while
    // the quarter-scale dry marker makes an incorrect compensation observable.
    assert!(maximum_error < 1e-5, "PDC waveform error {maximum_error}");
    assert!(
        wrong_manual_error > 0.1,
        "fixture did not distinguish manual support"
    );
    println!(
        "automatic_host_pdc rate={RATE} selected_taps={} actual_delay={actual_delay} manual_delay={manual_delay} frames={total_frames} maximum_error={maximum_error:e} wrong_manual_error={wrong_manual_error:e}",
        report.selected_taps
    );
}
