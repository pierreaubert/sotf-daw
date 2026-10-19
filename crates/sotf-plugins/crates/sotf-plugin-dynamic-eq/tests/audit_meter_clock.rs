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
