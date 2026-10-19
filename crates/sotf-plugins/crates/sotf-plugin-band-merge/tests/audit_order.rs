//! Declared band-order validation must precede state and output mutation.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, Plugin, ProcessContext};
use sotf_plugin_band_merge::BandMergePlugin;

#[global_allocator]
static ALLOCATOR: sotf_host::CountingAlloc = sotf_host::CountingAlloc;

#[test]
fn malformed_or_permuted_order_preserves_output_controls_and_ramp_history() {
    for rate in [32_000, 48_000, 192_000] {
        for channels in [1, 2, 6] {
            for bands in 2..=8 {
                let mut checked = BandMergePlugin::new(channels, bands).unwrap();
                let mut raw = BandMergePlugin::new(channels, bands).unwrap();
                for plugin in [&mut checked, &mut raw] {
                    plugin.initialize(f64::from(rate)).unwrap();
                    plugin
                        .set_parameter(
                            ParameterId::from("band_0_gain_db"),
                            ParameterValue::Float(-12.0),
                        )
                        .unwrap();
                }
                let frames = 257;
                let input: Vec<f32> = (0..frames * bands * channels)
                    .map(|i| ((i % 53) as f32 - 26.0) * 0.001)
                    .collect();
                let mut output = vec![0.125; frames * channels];
                let mut expected = output.clone();
                let context = ProcessContext::new(rate, frames);
                let canonical: Vec<usize> = (0..bands).collect();
                checked
                    .process_with_band_order(&input, &mut output, &context, &canonical)
                    .unwrap();
                raw.process(&input, &mut expected, &context).unwrap();
                assert!(output == expected);
                let mut reversed = canonical.clone();
                reversed.reverse();
                let mut duplicate = canonical.clone();
                duplicate[1] = 0;
                let mut out_of_range = canonical.clone();
                out_of_range[0] = bands;
                for invalid in [
                    &canonical[..bands - 1],
                    &reversed,
                    &duplicate,
                    &out_of_range,
                    &[],
                ] {
                    output.fill(0.125);
                    let controls = checked.parameters();
                    assert!(
                        checked
                            .process_with_band_order(&input, &mut output, &context, invalid)
                            .is_err()
                    );
                    assert!(
                        output.iter().all(|&x| x == 0.125),
                        "rejected routing touched output"
                    );
                    assert!(
                        checked
                            .parameters()
                            .iter()
                            .zip(&controls)
                            .all(|(a, b)| a.id == b.id && a.default_value == b.default_value)
                    );
                }
                sotf_host::assert_no_allocs("checked band merge", || {
                    checked
                        .process_with_band_order(&input, &mut output, &context, &canonical)
                        .unwrap();
                    raw.process(&input, &mut expected, &context).unwrap();
                });
                assert!(output == expected, "order rejection advanced the gain ramp");
            }
        }
    }
}
