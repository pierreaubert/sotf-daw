//! Public-API state and realtime regressions at the duplicate/decorrelator boundary.

use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_plugin_mono_to_stereo::{MonoToStereoPlugin, MonoToStereoPluginParams};

#[global_allocator]
static ALLOCATOR: sotf_host::test_utils::CountingAlloc = sotf_host::test_utils::CountingAlloc;

fn duplicate_plugin() -> MonoToStereoPlugin {
    let mut plugin = MonoToStereoPlugin::try_from_params(
        1,
        MonoToStereoPluginParams {
            stereo_width: 0.0,
            haas_delay_ms: 0.0,
            ..Default::default()
        },
    )
    .unwrap();
    plugin.initialize(48_000).unwrap();
    plugin
}

fn widen(plugin: &mut MonoToStereoPlugin) {
    plugin
        .set_parameter(
            ParameterId::from("stereo_width"),
            ParameterValue::Float(1.0),
        )
        .unwrap();
}

#[test]
fn hostile_sample_then_width_transition_is_partition_invariant() {
    for hostile in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut results = Vec::new();
        for partitions in [&[4096][..], &[1, 17, 7, 257, 31][..]] {
            let mut plugin = duplicate_plugin();
            let mut initial = [0.2; 17];
            initial[16] = hostile;
            let mut prefix = [0.0; 34];
            plugin
                .process(&initial, &mut prefix, &ProcessContext::new(48_000, 17))
                .unwrap();
            assert_eq!(&prefix[32..], &[0.0, 0.0]);
            widen(&mut plugin);
            let input: Vec<f32> = (0..4096)
                .map(|frame| (frame as f32 * 0.097).sin() * 0.2)
                .collect();
            let mut output = vec![0.0; input.len() * 2];
            let mut frame = 0;
            let mut block = 0;
            while frame < input.len() {
                let count = partitions[block % partitions.len()].min(input.len() - frame);
                plugin
                    .process(
                        &input[frame..frame + count],
                        &mut output[frame * 2..(frame + count) * 2],
                        &ProcessContext::new(48_000, count),
                    )
                    .unwrap();
                frame += count;
                block += 1;
            }
            assert!(output.iter().all(|sample| sample.is_finite()));
            results.push(output);
        }
        assert_eq!(results[0], results[1]);
    }
}

#[test]
fn cold_duplicate_transition_and_reset_do_not_allocate() {
    let mut plugin = duplicate_plugin();
    let mut input = [0.1; 257];
    input[256] = f32::NAN;
    let mut output = [0.0; 514];
    let context = ProcessContext::new(48_000, 257);
    sotf_host::test_utils::assert_no_allocs("cold duplicate path", || {
        plugin.process(&input, &mut output, &context).unwrap();
    });
    widen(&mut plugin);
    input[256] = 0.1;
    sotf_host::test_utils::assert_no_allocs("first decorated block and reset", || {
        plugin.process(&input, &mut output, &context).unwrap();
        plugin.reset();
        plugin.process(&input, &mut output, &context).unwrap();
    });
    assert!(output.iter().all(|sample| sample.is_finite()));
}
