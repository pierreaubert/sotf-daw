//! Rejected rate changes must preserve the recursive decoder state.
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_ambisonics::{AmbisonicsDecoderConfig, AmbisonicsDecoderPlugin};

#[test]
fn dual_band_rate_mismatch_leaves_samples_and_history_unchanged() {
    for algorithm in ["allrad", "mode_matching"] {
        let make = || {
            let mut plugin = AmbisonicsDecoderPlugin::new(&AmbisonicsDecoderConfig {
                order: 1,
                target_layout: "5.1".into(),
                max_re_weighting: true,
                dual_band: true,
                algorithm: algorithm.into(),
            })
            .unwrap();
            plugin.initialize(48_000.0).unwrap();
            plugin
        };
        let mut tested = make();
        let mut reference = make();
        let input = vec![0.125; 17 * tested.input_channels()];
        let mut actual = vec![0.0; 17 * tested.output_channels()];
        let mut expected = actual.clone();
        tested
            .process(&input, &mut actual, &ProcessContext::new(48_000, 17))
            .unwrap();
        reference
            .process(&input, &mut expected, &ProcessContext::new(48_000, 17))
            .unwrap();
        actual.fill(42.0);
        assert!(
            tested
                .process(&input, &mut actual, &ProcessContext::new(96_000, 17))
                .is_err()
        );
        assert!(actual.iter().all(|sample| *sample == 42.0));
        tested
            .process(&input, &mut actual, &ProcessContext::new(48_000, 17))
            .unwrap();
        reference
            .process(&input, &mut expected, &ProcessContext::new(48_000, 17))
            .unwrap();
        assert_eq!(actual, expected);
    }
}
