//! IIR and FIR callbacks must use the prepared coefficient sample rate.
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_crossover::CrossoverPlugin;

#[test]
fn all_filter_families_reject_wrong_rate_without_changing_history() {
    for kind in [
        "LR24", "LR12", "LR48", "BW6", "BW12", "BW18", "BW24", "BW30", "BW36", "BW42", "BW48",
        "Bessel12", "FIR",
    ] {
        for multi in [false, true] {
            let make = || {
                let mut plugin = CrossoverPlugin::new_multiway(
                    2,
                    kind,
                    500.0,
                    "both",
                    if multi { &[3000.0] } else { &[] },
                )
                .unwrap();
                plugin.initialize(48_000.0).unwrap();
                plugin
            };
            let mut tested = make();
            let mut reference = make();
            let input = [0.125; 34];
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
                    .is_err(),
                "{kind} multi={multi}"
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
}
