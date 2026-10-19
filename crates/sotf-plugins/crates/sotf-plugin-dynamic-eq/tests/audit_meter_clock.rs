//! Meter publication follows accepted sample time, independently of callbacks.
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_dynamic_eq::{
    DynEqBandParams, DynamicEqData, DynamicEqPlugin, DynamicEqPluginParams,
};

fn meter(plugin: &DynamicEqPlugin) -> f32 {
    plugin
        .get_data()
        .unwrap()
        .downcast_ref::<DynamicEqData>()
        .unwrap()
        .gain_reduction_db[0]
}

#[test]
fn diagnostic_publication_occurs_at_exact_sample_boundaries() {
    for rate in [44_100_u32, 48_000, 96_000] {
        let interval = rate as usize / 10;
        let mut published = Vec::new();
        for pattern in [&[1][..], &[1, 7, 31, 257][..], &[8192][..]] {
            let params = DynamicEqPluginParams {
                num_bands: 1,
                threshold: -60.0,
                ratio: 10.0,
                attack_ms: 1.0,
                bands: vec![DynEqBandParams {
                    frequency: 1000.0,
                    gain: -12.0,
                    ..Default::default()
                }],
                ..Default::default()
            };
            let mut plugin = DynamicEqPlugin::from_params(2, params);
            plugin.initialize(f64::from(rate)).unwrap();
            let mut position = 0;
            let mut block = 0;
            while position < interval - 1 {
                let frames = pattern[block % pattern.len()].min(interval - 1 - position);
                let mut input: Vec<f32> = (position..position + frames)
                    .flat_map(|frame| {
                        let sample = (std::f64::consts::TAU * 1000.0 * frame as f64
                            / f64::from(rate))
                        .sin() as f32
                            * 0.5;
                        [sample, sample]
                    })
                    .collect();
                plugin
                    .process_in_place(&mut input, &ProcessContext::new(rate, frames))
                    .unwrap();
                position += frames;
                block += 1;
            }
            assert_eq!(
                meter(&plugin),
                0.0,
                "rate={rate}, pattern={pattern:?}: publication before 100 ms"
            );
            let sample = (std::f64::consts::TAU * 1000.0 * position as f64 / f64::from(rate)).sin()
                as f32
                * 0.5;
            plugin
                .process_in_place(&mut [sample; 2], &ProcessContext::new(rate, 1))
                .unwrap();
            let value = meter(&plugin);
            assert!(
                value.abs() > 0.01,
                "the detector must exercise nonzero GR at 100 ms"
            );
            published.push(value);
        }
        assert!(
            published.iter().all(|value| *value == published[0]),
            "rate={rate}: partitioned meter values differ: {published:?}"
        );
    }
}

#[global_allocator]
static ALLOCATOR: sotf_host::test_utils::CountingAlloc = sotf_host::test_utils::CountingAlloc;

#[test]
fn first_meter_publication_and_contended_readers_do_not_allocate_or_free() {
    let mut plugin = DynamicEqPlugin::new(2);
    plugin.initialize(48_000.0).unwrap();
    let initial = plugin.get_data().unwrap();
    let mut buffer = [0.25; 512 * 2];
    let context = ProcessContext::new(48_000, 512);
    let counts = sotf_host::test_utils::measure_heap_activity(|| {
        for _ in 0..10 {
            plugin.process_in_place(&mut buffer, &context).unwrap();
        }
    });
    assert_eq!(counts, (0, 0), "first publication must use prepared slots");
    let published = plugin.get_data().unwrap();
    assert!(!std::sync::Arc::ptr_eq(&initial, &published));
    let counts = sotf_host::test_utils::measure_heap_activity(|| {
        for _ in 0..30 {
            plugin.process_in_place(&mut buffer, &context).unwrap();
        }
    });
    assert_eq!(
        counts,
        (0, 0),
        "retained readers must only defer publication"
    );
    drop(initial);
    drop(published);
    let counts = sotf_host::test_utils::measure_heap_activity(|| {
        for _ in 0..10 {
            plugin.process_in_place(&mut buffer, &context).unwrap();
        }
    });
    assert_eq!(counts, (0, 0), "publication must recover without heap work");
}

#[test]
fn audio_after_a_meter_boundary_cannot_change_that_boundarys_snapshot() {
    for rate in [44_100_u32, 48_000, 96_000, 192_000] {
        let interval = rate as usize / 10;
        let frames = interval + rate as usize / 100;
        let params = DynamicEqPluginParams {
            num_bands: 1,
            threshold: -60.0,
            ratio: 10.0,
            attack_ms: 1.0,
            bands: vec![DynEqBandParams {
                frequency: 1000.0,
                gain: -12.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut whole = DynamicEqPlugin::from_params(2, params.clone());
        let mut split = DynamicEqPlugin::from_params(2, params);
        for plugin in [&mut whole, &mut split] {
            plugin.prepare_max_block_frames(frames).unwrap();
            plugin.initialize(f64::from(rate)).unwrap();
        }
        // Silence until exactly 100 ms, then a loud detector stimulus. The
        // 100 ms snapshot must stay zero after the longer callback completes.
        let mut a: Vec<f32> = (0..frames)
            .flat_map(|n| {
                let sample = if n < interval {
                    0.0
                } else {
                    0.5 * (std::f64::consts::TAU * 1000.0 * (n - interval) as f64 / f64::from(rate))
                        .cos() as f32
                };
                [sample, sample]
            })
            .collect();
        let mut b = a.clone();
        whole
            .process_in_place(&mut a, &ProcessContext::new(rate, frames))
            .unwrap();
        split
            .process_in_place(&mut b[..interval * 2], &ProcessContext::new(rate, interval))
            .unwrap();
        split
            .process_in_place(
                &mut b[interval * 2..],
                &ProcessContext::new(rate, frames - interval),
            )
            .unwrap();
        assert!(
            a == b,
            "audio must not depend on the meter callback boundary"
        );
        assert_eq!(meter(&split), 0.0);
        assert_eq!(
            meter(&whole),
            meter(&split),
            "{rate}: the completed snapshot included future detector samples"
        );
    }
}

#[test]
fn settling_to_dry_publishes_zero_at_the_same_sample_for_all_callbacks() {
    use sotf_host::{ParameterId, ParameterValue};
    for rate in [44_100_u32, 48_000, 96_000, 192_000] {
        let interval = rate as usize / 10;
        let mut results = Vec::new();
        for block in [interval, 31] {
            let params = DynamicEqPluginParams {
                num_bands: 1,
                threshold: -60.0,
                ratio: 10.0,
                attack_ms: 1.0,
                bands: vec![DynEqBandParams {
                    frequency: 1000.0,
                    gain: -12.0,
                    ..Default::default()
                }],
                ..Default::default()
            };
            let mut plugin = DynamicEqPlugin::from_params(2, params);
            plugin.prepare_max_block_frames(interval).unwrap();
            plugin.initialize(f64::from(rate)).unwrap();
            plugin
                .set_parameter(ParameterId::from("mix"), ParameterValue::Float(0.0))
                .unwrap();
            let mut input: Vec<f32> = (0..interval)
                .flat_map(|n| {
                    let x = 0.5
                        * (std::f64::consts::TAU * 1000.0 * n as f64 / f64::from(rate)).sin()
                            as f32;
                    [x, x]
                })
                .collect();
            for audio in input.chunks_mut(block * 2) {
                let frames = audio.len() / 2;
                plugin
                    .process_in_place(audio, &ProcessContext::new(rate, frames))
                    .unwrap();
            }
            results.push(meter(&plugin));
        }
        assert_eq!(
            results[0], results[1],
            "{rate}: dry settling meter depends on callback size"
        );
        assert_eq!(results[0], 0.0, "settled dry meter must publish zero");
    }
}

#[test]
fn tiny_nonzero_mix_is_active_and_bulk_retarget_does_not_reset_it() {
    use sotf_host::parametric_plugin::ParameterSet;
    use sotf_host::{ParameterId, ParameterValue};
    use sotf_plugin_dynamic_eq::DynEqShape;
    let mix = 1e-7;
    let params = DynamicEqPluginParams {
        num_bands: 1,
        threshold: -60.0,
        ratio: 10.0,
        attack_ms: 1.0,
        mix,
        bands: vec![DynEqBandParams {
            shape: DynEqShape::LowShelf,
            frequency: 1000.0,
            gain: 24.0,
            band_threshold: -60.0,
            band_ratio: 10.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut scalar =
        DynamicEqPlugin::try_from_params_at_sample_rate(2, params.clone(), 48_000).unwrap();
    let mut bulk =
        DynamicEqPlugin::try_from_params_at_sample_rate(2, params.clone(), 48_000).unwrap();
    let mut wet = DynamicEqPlugin::try_from_params_at_sample_rate(
        2,
        DynamicEqPluginParams { mix: 1.0, ..params },
        48_000,
    )
    .unwrap();
    let mut a = vec![0.2; 8192 * 2];
    let mut b = a.clone();
    let mut w = a.clone();
    let context = ProcessContext::new(48_000, 8192);
    scalar.process_in_place(&mut a, &context).unwrap();
    bulk.process_in_place(&mut b, &context).unwrap();
    wet.process_in_place(&mut w, &context).unwrap();
    assert!(
        a[a.len() - 2..] != [0.2, 0.2],
        "nonzero mix must not be reclassified as dry"
    );
    let expected = 0.2 * (1.0 - mix) + w[w.len() - 1] * mix;
    assert_eq!(a[a.len() - 1], expected);
    scalar
        .set_parameter(ParameterId::from("mix"), ParameterValue::Float(mix * 2.0))
        .unwrap();
    let mut values = ParameterSet::new();
    values.insert(ParameterId::from("mix"), ParameterValue::Float(mix * 2.0));
    bulk.apply_values(values).unwrap();
    let mut a = vec![0.2; 257 * 2];
    let mut b = a.clone();
    scalar
        .process_in_place(&mut a, &ProcessContext::new(48_000, 257))
        .unwrap();
    bulk.process_in_place(&mut b, &ProcessContext::new(48_000, 257))
        .unwrap();
    assert!(
        a == b,
        "bulk and scalar nonzero mix retargets must preserve the same detector/filter state"
    );
}
