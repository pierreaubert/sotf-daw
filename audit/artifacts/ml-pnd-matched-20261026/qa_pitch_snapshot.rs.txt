//! Read-only QA decisions preserve native state and expose reset boundaries.
#![cfg(feature = "qa")]

// Rust guideline compliant 2026-02-21
use sotf_host::{CountingAlloc, Plugin, ProcessContext};
use sotf_plugin_pnd::{PndPlugin, PndPluginParams};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

#[test]
fn snapshots_are_heap_free_read_only_and_clear_on_reset() {
    let params = PndPluginParams {
        reference_frequency_hz: 440.0,
        ..PndPluginParams::default()
    };
    let mut observed = PndPlugin::from_params(1, params.clone()).unwrap();
    let mut control = PndPlugin::from_params(1, params).unwrap();
    observed.initialize(48000.0).unwrap();
    control.initialize(48000.0).unwrap();
    let mut a = [0.0_f32; 128];
    let mut b = a;
    for block in 0..64 {
        let input = std::array::from_fn::<_, 128, _>(|i| {
            (std::f32::consts::TAU * 444.0 * (block * 128 + i) as f32 / 48000.0).sin() * 0.2
        });
        let context = ProcessContext::new(48000, 128);
        observed.process(&input, &mut a, &context).unwrap();
        control.process(&input, &mut b, &context).unwrap();
        assert_eq!(a, b);
        sotf_host::test_utils::assert_no_allocs_or_deallocs("QA pitch snapshot", || {
            let first = observed.pitch_decision_snapshot();
            assert_eq!(first, observed.pitch_decision_snapshot());
            assert!(!first.neural_pitch_used);
            assert!(first.drift_ratio.is_finite() && first.correction_ratio.is_finite());
        });
    }
    assert!(observed.pitch_decision_snapshot().analysis_generation > 0);
    observed.reset();
    let state = observed.pitch_decision_snapshot();
    assert_eq!(state.analysis_generation, 0);
    assert_eq!(state.drift_ratio, 1.0);
    assert_eq!(state.correction_ratio, 1.0);
    assert!(!state.neural_pitch_used);
}
