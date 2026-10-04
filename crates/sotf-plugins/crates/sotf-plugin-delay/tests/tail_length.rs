use sotf_host::{
    CountingAlloc, ParameterId, ParameterValue, ParametricInPlacePlugin,
    ParametricInPlacePluginAdapter, Plugin, ProcessContext, TailLength, assert_no_allocs,
};
use sotf_plugin_delay::DelayPlugin;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

#[test]
fn finite_delay_bound_retains_integer_impulse_and_covers_fractional_interpolation() {
    for rate in [44_100, 48_000, 96_000] {
        for delay_frames in [0.0, 1.0, 1.25, 37.0, 37.75] {
            let delay_ms = delay_frames * 1000.0 / rate as f32;
            let delay = DelayPlugin::try_new_with_max_delay(2, delay_ms, 0.0, 1.0, 100.0).unwrap();
            let mut plugin = ParametricInPlacePluginAdapter::new(delay);
            assert_eq!(Plugin::tail_length(&plugin), TailLength::Unknown);
            Plugin::initialize(&mut plugin, f64::from(rate)).unwrap();
            let TailLength::Finite(bound) = Plugin::tail_length(&plugin) else {
                panic!("nonrecursive delay is finite");
            };
            let total = bound as usize + 128 + 257;
            let mut stream = vec![0.0_f32; total * 2];
            let mut position = 0;
            for size in [1, 17, 127, 256].into_iter().cycle() {
                if position == total {
                    break;
                }
                let frames = size.min(total - position);
                let mut input = vec![0.0; frames * 2];
                for (index, amplitude) in [(0, 0.5), (127, -0.25)] {
                    if (position..position + frames).contains(&index) {
                        input[(index - position) * 2] = amplitude;
                        input[(index - position) * 2 + 1] = -amplitude;
                    }
                }
                let output = &mut stream[position * 2..(position + frames) * 2];
                let mut context = ProcessContext::new(rate, frames);
                context.transport.sample_position = position as u64;
                assert_eq!(
                    Plugin::process(&mut plugin, &input, output, &context).unwrap(),
                    frames
                );
                assert_eq!(Plugin::tail_length(&plugin), TailLength::Finite(bound));
                position += frames;
            }
            assert!(stream.iter().all(|sample| sample.is_finite()));
            assert!(stream.iter().any(|sample| sample.abs() > 0.1));
            assert!(
                stream[(128 + bound as usize) * 2..]
                    .iter()
                    .all(|sample| *sample == 0.0)
            );
            if delay_frames.fract() == 0.0 {
                for (frame, pair) in stream.as_chunks::<2>().0.iter().enumerate() {
                    let expected = if frame == delay_frames as usize {
                        0.5
                    } else if frame == 127 + delay_frames as usize {
                        -0.25
                    } else {
                        0.0
                    };
                    assert!(
                        (pair[0] - expected).abs() < 2e-5,
                        "rate{rate}, delay{delay_frames}, frame{frame}"
                    );
                    assert!((pair[0] + pair[1]).abs() < 1e-6);
                }
            }
        }
    }
}

#[test]
fn feedback_history_keeps_infinite_classification_until_reset() {
    for feedback in [-0.5, 0.5] {
        let mut plugin = DelayPlugin::try_new_with_max_delay(1, 1.0, 0.0, 1.0, 20.0).unwrap();
        plugin.initialize(48_000.0).unwrap();
        let finite = plugin.tail_length();
        assert!(matches!(finite, TailLength::Finite(_)));
        plugin
            .parametric_set_parameter(
                ParameterId::from("feedback"),
                ParameterValue::Float(feedback),
            )
            .unwrap();
        assert_eq!(plugin.tail_length(), TailLength::Infinite);
        let mut input = [0.0; 257];
        input[0] = 0.5;
        plugin
            .process_in_place(&mut input, &ProcessContext::new(48_000, 257))
            .unwrap();
        plugin
            .parametric_set_parameter(ParameterId::from("feedback"), ParameterValue::Float(0.0))
            .unwrap();
        assert_eq!(plugin.tail_length(), TailLength::Infinite);
        std::thread::spawn(move || {
            assert_no_allocs("Delay tail query and reset", || {
                assert_eq!(plugin.tail_length(), TailLength::Infinite);
                plugin.reset();
                assert_eq!(plugin.tail_length(), finite);
            });
        })
        .join()
        .unwrap();
    }
}
