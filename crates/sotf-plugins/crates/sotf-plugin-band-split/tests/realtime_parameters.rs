use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, assert_no_allocs};
use sotf_plugin_band_split::{BandSplitPlugin, BandSplitRecombinationMode};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

#[test]
fn completed_cutoff_ramps_adopt_exact_coefficients_without_heap_activity() {
    for slope in ["LR24", "LR48"] {
        for mode in [
            BandSplitRecombinationMode::LegacyCascade,
            BandSplitRecombinationMode::PhaseCompensated,
        ] {
            let mut plugin =
                BandSplitPlugin::new_multiband_with_mode(2, &[300.0, 1500.0, 6000.0], slope, mode)
                    .unwrap();
            plugin.initialize(96_000.0).unwrap();
            let frequency = ParameterId::from("frequency");
            let input = vec![0.1; 8192 * 2];
            let mut output = vec![0.0; 8192 * 8];
            let context = ProcessContext::new(96_000, 8192);
            for target in [150.0, 150.0005, 150.05, 600.0] {
                assert_no_allocs_or_deallocs("exact cutoff adoption", || {
                    plugin
                        .set_parameter(frequency.clone(), ParameterValue::Float(target))
                        .unwrap();
                    plugin.process(&input, &mut output, &context).unwrap();
                });
            }
        }
    }
}

#[test]
fn live_frequency_and_gain_updates_do_not_allocate() {
    let mut plugin = BandSplitPlugin::new_multiband(2, &[500.0, 2_000.0], "LR48").unwrap();
    plugin.initialize(48_000.0).unwrap();
    let frequency = ParameterId::from("frequency_2");
    let gain = ParameterId::from("band_1_gain_db");

    assert_no_allocs("Band Split live frequency update", || {
        plugin
            .set_parameter(frequency.clone(), ParameterValue::Float(3_000.0))
            .unwrap();
    });
    assert_no_allocs("Band Split live gain update", || {
        plugin
            .set_parameter(gain.clone(), ParameterValue::Float(-6.0))
            .unwrap();
    });
}

#[test]
fn structural_type_change_is_rejected_without_rebuilding_dsp() {
    let mut plugin = BandSplitPlugin::new(2, 1_000.0, "LR24").unwrap();
    plugin.initialize(48_000.0).unwrap();
    let type_id = ParameterId::from("crossover_type");
    let before = plugin.get_parameter(&type_id);

    let error = plugin
        .set_parameter(type_id.clone(), ParameterValue::Int(1))
        .unwrap_err();
    assert!(error.contains("structural"));
    assert_eq!(plugin.get_parameter(&type_id), before);
}
