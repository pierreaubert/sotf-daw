//! Explicit recursive render cutoffs preserve mathematical infinite support.

// Rust guideline compliant 2026-02-21
use sotf_host::plugin::PluginDrainResult;
use sotf_host::{
    ParametricInPlacePlugin, ParametricInPlacePluginAdapter, Plugin, ProcessContext, TailLength,
};
use sotf_plugin_multiband_compressor::{
    MultibandCompressorPlugin, MultibandCompressorPluginParams,
};

#[global_allocator]
static ALLOCATOR: sotf_host::CountingAlloc = sotf_host::CountingAlloc;

fn make(
    rate: u32,
    channels: usize,
    bands: usize,
    lookahead: f32,
    mix: f32,
    limit: Option<usize>,
) -> MultibandCompressorPlugin {
    let mut plugin = MultibandCompressorPlugin::from_params(
        channels,
        MultibandCompressorPluginParams {
            num_bands: bands,
            per_band_lookahead_ms: lookahead,
            mix,
            ratio: 1.0,
            ..Default::default()
        },
    );
    plugin.set_recursive_tail_render_frames(limit).unwrap();
    assert_eq!(plugin.tail_length(), TailLength::Unknown);
    assert_eq!(plugin.drain_output_frames_max(), 0);
    plugin.initialize(f64::from(rate)).unwrap();
    plugin
}
fn feed(plugin: &mut MultibandCompressorPlugin, rate: u32, input: &[f32]) -> Vec<f32> {
    let channels = ParametricInPlacePlugin::channels(plugin);
    let mut output = input.to_vec();
    for block in output.chunks_mut(97 * channels) {
        let frames = block.len() / channels;
        let counts = sotf_host::test_utils::measure_heap_activity(|| {
            assert_eq!(
                plugin
                    .process_in_place(block, &ProcessContext::new(rate, frames))
                    .unwrap(),
                frames
            );
        });
        assert_eq!(counts, (0, 0));
    }
    output
}
fn drain(plugin: &mut MultibandCompressorPlugin, rate: u32) -> Vec<f32> {
    let channels = ParametricInPlacePlugin::channels(plugin);
    let mut output = Vec::new();
    for call in 0..4000 {
        let capacity = [1, 17, 257, 1024][call % 4];
        let mut buffer = vec![37.0; capacity * channels + 3];
        let mut result = Ok(PluginDrainResult::COMPLETE);
        let counts = sotf_host::test_utils::measure_heap_activity(|| {
            result = plugin.drain(
                &mut buffer[..capacity * channels],
                &ProcessContext::new(rate, 0),
            );
        });
        assert_eq!(counts, (0, 0));
        let result = result.unwrap();
        assert!(result.frames <= capacity && result.frames <= plugin.drain_output_frames_max());
        assert!(
            buffer[result.frames * channels..]
                .iter()
                .all(|&sample| sample == 37.0)
        );
        output.extend_from_slice(&buffer[..result.frames * channels]);
        if result.complete {
            return output;
        }
        assert!(result.frames > 0);
    }
    panic!("chosen recursive render did not complete");
}

#[test]
fn chosen_recursive_extent_matches_uninterrupted_zero_continuation() {
    for rate in [44_100, 96_000, 192_000] {
        for channels in [1, 2, 6] {
            for bands in [2, 3, 5] {
                for lookahead in [0.0, 5.0] {
                    for limit in [0, 1, 17, 1025] {
                        let mut actual = make(rate, channels, bands, lookahead, 1.0, Some(limit));
                        let mut reference = make(rate, channels, bands, lookahead, 1.0, None);
                        let mut input = vec![0.0; 131 * channels];
                        for ch in 0..channels {
                            input[ch] = 0.125;
                            input[130 * channels + ch] = -0.25;
                        }
                        assert_eq!(
                            feed(&mut actual, rate, &input),
                            feed(&mut reference, rate, &input)
                        );
                        assert_eq!(actual.tail_length(), TailLength::Infinite);
                        assert_eq!(
                            actual.drain_call_bound().unwrap().get(),
                            limit.div_ceil(256).max(1) as u64
                        );
                        let expected = feed(&mut reference, rate, &vec![0.0; limit * channels]);
                        assert_eq!(
                            drain(&mut actual, rate),
                            expected,
                            "rate={rate},channels={channels},bands={bands},lookahead={lookahead},limit={limit}"
                        );
                        assert_eq!(actual.tail_length(), TailLength::Infinite);
                        if limit <= 17 {
                            let future_frames = reference.latency_samples() + 4096;
                            let future =
                                feed(&mut reference, rate, &vec![0.0; future_frames * channels]);
                            assert!(
                                future.iter().any(|sample| sample.abs() > 1e-9),
                                "cutoff must explicitly discard a real remaining response"
                            );
                        }
                        actual
                            .set_recursive_tail_render_frames(Some(limit))
                            .unwrap();
                        assert!(actual.set_recursive_tail_render_frames(None).is_err());
                        actual.reset();
                        assert_eq!(actual.recursive_tail_render_frames(), Some(limit));
                        actual.set_recursive_tail_render_frames(None).unwrap();
                    }
                }
            }
        }
    }
}

#[test]
fn exact_finite_paths_take_precedence_over_recursive_render_policy() {
    for (bands, mix) in [(1, 1.0), (3, 0.0)] {
        let mut actual = make(48_000, 2, bands, 5.0, mix, Some(4096));
        let mut reference = make(48_000, 2, bands, 5.0, mix, None);
        let input = vec![0.25; 117 * 2];
        assert_eq!(
            feed(&mut actual, 48_000, &input),
            feed(&mut reference, 48_000, &input)
        );
        let delay = actual.latency_samples();
        assert_eq!(actual.tail_length(), TailLength::Finite(delay as u64));
        assert_eq!(actual.drain_output_frames_max(), delay.min(256));
        assert_eq!(
            drain(&mut actual, 48_000),
            feed(&mut reference, 48_000, &vec![0.0; delay * 2])
        );
    }
}

#[test]
fn chosen_extent_stays_frozen_when_mix_settles_dry_during_render() {
    let mut actual = make(48_000, 2, 3, 0.0, 1.0, Some(32000));
    feed(&mut actual, 48_000, &[0.25; 258]);
    actual
        .set_parameter(
            sotf_host::ParameterId::from("mix"),
            sotf_host::ParameterValue::Float(0.0),
        )
        .unwrap();
    let mut frames = 0;
    let mut became_dry = false;
    let mut complete = false;
    for _ in 0..128 {
        let mut output = [37.0; 512];
        let result = actual
            .drain(&mut output, &ProcessContext::new(48_000, 0))
            .unwrap();
        frames += result.frames;
        became_dry |= actual.tail_length() == TailLength::Finite(0);
        assert_eq!(
            actual.drain_output_frames_max(),
            256,
            "frozen render capacity changed during wet-to-dry settling"
        );
        if result.complete {
            complete = true;
            break;
        }
    }
    assert!(complete);
    assert!(became_dry);
    assert_eq!(frames, 32000);
}

#[test]
fn rejected_drain_does_not_start_eos_or_freeze_policy() {
    let mut plugin = make(48_000, 2, 3, 0.0, 1.0, Some(64));
    feed(&mut plugin, 48_000, &[0.25; 34]);
    let mut odd = [37.0; 3];
    assert!(
        plugin
            .drain(&mut odd, &ProcessContext::new(48_000, 0))
            .is_err()
    );
    assert_eq!(odd, [37.0; 3]);
    plugin.set_recursive_tail_render_frames(Some(65)).unwrap();
    assert!(
        plugin
            .drain(&mut [], &ProcessContext::new(48_000, 0))
            .is_err()
    );
    plugin.set_recursive_tail_render_frames(Some(66)).unwrap();
    let mut output = [37.0; 2];
    assert!(
        plugin
            .drain(&mut output, &ProcessContext::new(44_100, 0))
            .is_err()
    );
    assert_eq!(output, [37.0; 2]);
    assert_eq!(
        plugin
            .drain(&mut output, &ProcessContext::new(48_000, 0))
            .unwrap()
            .frames,
        1
    );
    assert!(plugin.set_recursive_tail_render_frames(Some(67)).is_err());
    assert_eq!(plugin.recursive_tail_render_frames(), Some(66));
    let mut input = [9.0; 2];
    assert!(
        plugin
            .process_in_place(&mut input, &ProcessContext::new(48_000, 1))
            .is_err()
    );
    assert_eq!(input, [9.0; 2]);
    assert_eq!(drain(&mut plugin, 48_000).len(), 65 * 2);
    plugin.initialize(96_000.0).unwrap();
    assert_eq!(plugin.recursive_tail_render_frames(), Some(66));
    assert_eq!(plugin.tail_length(), TailLength::Infinite);
}

#[test]
fn adapter_forwards_bounded_render_while_mathematical_tail_is_infinite() {
    let native = make(48_000, 2, 3, 0.0, 1.0, Some(513));
    let mut plugin = ParametricInPlacePluginAdapter::new(native);
    Plugin::initialize(&mut plugin, 48_000.0).unwrap();
    plugin
        .process(&[0.25; 2], &mut [0.0; 2], &ProcessContext::new(48_000, 1))
        .unwrap();
    assert_eq!(Plugin::tail_length(&plugin), TailLength::Infinite);
    assert_eq!(plugin.drain_call_bound().unwrap().get(), 3);
    let mut frames = 0;
    let mut complete = false;
    for _ in 0..10 {
        let result = plugin
            .drain(&mut [0.0; 512], &ProcessContext::new(48_000, 0))
            .unwrap();
        frames += result.frames;
        if result.complete {
            complete = true;
            break;
        }
    }
    assert!(complete);
    assert_eq!(frames, 513);
}

#[test]
fn daw_host_renders_explicit_cutoff_without_claiming_finite_iir_support() {
    let native = make(48_000, 2, 3, 0.0, 1.0, Some(513));
    let mut host = sotf_host::DawHost::new(2, 48_000);
    host.add_plugin(Box::new(ParametricInPlacePluginAdapter::new(native)))
        .unwrap();
    host.build().unwrap();
    let mut reference = make(48_000, 2, 3, 0.0, 1.0, None);
    let mut output = [0.0; 2];
    host.process(&[0.25; 2], &mut output).unwrap();
    assert_eq!(output.to_vec(), feed(&mut reference, 48_000, &[0.25; 2]));
    assert_eq!(
        host.get_plugin(0).unwrap().tail_length(),
        TailLength::Infinite
    );
    let mut tail = Vec::new();
    let mut output = vec![0.0; host.drain_output_frames_max() * 2];
    let mut complete = false;
    for _ in 0..10 {
        let step = host.drain(&mut output).unwrap();
        tail.extend_from_slice(&output[..step.frames * 2]);
        if step.complete {
            complete = true;
            break;
        }
    }
    assert!(complete);
    assert_eq!(tail.len(), 513 * 2);
    assert_eq!(tail, feed(&mut reference, 48_000, &vec![0.0; 513 * 2]));
}
