use super::misc::OS_CHUNK_SIZE;
use super::misc::interleaved_to_planar;
use super::misc::planar_to_interleaved;
use super::oversampled_plugin::OversampledPlugin;
use super::oversampler::Oversampler;
use crate::plugin::InPlacePlugin;

#[test]
fn test_oversampler_2x_passthrough() {
    let channels = 2;
    let mut os = Oversampler::new(2, channels).unwrap();

    // Process silence through a passthrough callback
    let num_frames = 512;
    let mut buffer = vec![0.0f32; num_frames * channels];

    // Process several blocks to fill the pipeline
    for _ in 0..10 {
        os.process(&mut buffer, num_frames, |_planar, _frames| {
            // passthrough: do nothing
        })
        .unwrap();
    }

    // Output should be silence (within float tolerance)
    for (i, &s) in buffer.iter().enumerate() {
        assert!(
            s.abs() < 1e-6,
            "2x passthrough sample {} not silent: {}",
            i,
            s
        );
    }
}

#[test]
fn test_oversampler_4x_passthrough() {
    let channels = 2;
    let mut os = Oversampler::new(4, channels).unwrap();

    let num_frames = 512;
    let mut buffer = vec![0.0f32; num_frames * channels];

    for _ in 0..10 {
        os.process(&mut buffer, num_frames, |_planar, _frames| {
            // passthrough: do nothing
        })
        .unwrap();
    }

    for (i, &s) in buffer.iter().enumerate() {
        assert!(
            s.abs() < 1e-6,
            "4x passthrough sample {} not silent: {}",
            i,
            s
        );
    }
}

#[test]
fn test_oversampler_latency() {
    let os_2x = Oversampler::new(2, 2).unwrap();
    assert!(
        os_2x.latency_samples() > 0,
        "2x oversampler should have nonzero latency"
    );
    // Latency should be reasonable: at least OS_CHUNK_SIZE and less than
    // several thousand samples
    assert!(os_2x.latency_samples() >= OS_CHUNK_SIZE);
    assert!(os_2x.latency_samples() < 4096);

    let os_4x = Oversampler::new(4, 2).unwrap();
    assert!(
        os_4x.latency_samples() > 0,
        "4x oversampler should have nonzero latency"
    );
    assert!(os_4x.latency_samples() >= OS_CHUNK_SIZE);
    assert!(os_4x.latency_samples() < 4096);
}

fn render_partitioned_oversampling(
    os: &mut Oversampler,
    input: &[f32],
    channels: usize,
    partition: &[usize],
) -> Vec<f32> {
    let mut output = input.to_vec();
    let mut position = 0;
    for &frames in partition.iter().cycle() {
        let frames = frames.min(input.len() / channels - position);
        if frames == 0 {
            break;
        }
        let block = &mut output[position * channels..(position + frames) * channels];
        crate::assert_no_allocs("oversampling callback partition", || {
            assert_eq!(os.process(block, frames, |_, _| {}).unwrap(), frames);
        });
        position += frames;
    }
    output
}

#[test]
fn oversampler_delay_and_waveform_are_independent_of_callback_partition() {
    for factor in [2, 4] {
        for channels in [1, 2, 6] {
            let mut input = vec![0.0; 4096 * channels];
            for (channel, sample) in input[..channels].iter_mut().enumerate() {
                *sample = 1.0 / (channel + 1) as f32;
            }
            let mut reference_os = Oversampler::new(factor, channels).unwrap();
            let reference =
                render_partitioned_oversampling(&mut reference_os, &input, channels, &[256]);
            let peak = |samples: &[f32]| {
                samples
                    .chunks_exact(channels)
                    .enumerate()
                    .max_by(|(_, a), (_, b)| a[0].abs().total_cmp(&b[0].abs()))
                    .unwrap()
                    .0
            };
            for partition in [
                &[1][..],
                &[127][..],
                &[256][..],
                &[512][..],
                &[1, 17, 255, 3, 512, 29][..],
            ] {
                let mut os = Oversampler::new(factor, channels).unwrap();
                let actual = render_partitioned_oversampling(&mut os, &input, channels, partition);
                assert_eq!(
                    peak(&actual),
                    peak(&reference),
                    "factor={factor}, channels={channels}, partition={partition:?}"
                );
                for (index, (&sample, &expected)) in actual.iter().zip(&reference).enumerate() {
                    assert_eq!(
                        sample, expected,
                        "factor={factor}, channels={channels}, partition={partition:?}, sample={index}"
                    );
                }
                assert_eq!(
                    peak(&actual),
                    os.latency_samples(),
                    "impulse peak must equal reported PDC latency"
                );
                os.reset();
                assert_eq!(
                    render_partitioned_oversampling(&mut os, &input, channels, partition),
                    actual
                );
            }
        }
    }
}

#[test]
fn test_oversampler_preserves_signal() {
    // Process a known sine wave through a passthrough callback and verify
    // the output has the same frequency content. After the pipeline fills,
    // a passthrough should reproduce the input with only resampler delay.
    let channels = 1;
    let mut os = Oversampler::new(2, channels).unwrap();

    let num_frames = 512;
    let freq = 1000.0f32;
    let sample_rate = 48000.0f32;

    // Warm up the pipeline with the sine
    for block in 0..20 {
        let mut buffer: Vec<f32> = (0..num_frames)
            .map(|i| {
                let t = (block * num_frames + i) as f32 / sample_rate;
                (2.0 * std::f32::consts::PI * freq * t).sin() * 0.5
            })
            .collect();

        os.process(&mut buffer, num_frames, |_planar, _frames| {
            // passthrough
        })
        .unwrap();
    }

    // Now capture one more block
    let block = 20;
    let mut output: Vec<f32> = (0..num_frames)
        .map(|i| {
            let t = (block * num_frames + i) as f32 / sample_rate;
            (2.0 * std::f32::consts::PI * freq * t).sin() * 0.5
        })
        .collect();

    os.process(&mut output, num_frames, |_planar, _frames| {
        // passthrough
    })
    .unwrap();

    // The output should be a sine wave with similar amplitude (within
    // resampler attenuation tolerance). Check that peak is > 0.3
    // (input peak is 0.5, some attenuation from the anti-aliasing filter
    // is expected).
    let peak = output.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    assert!(
        peak > 0.3,
        "Output peak {} is too low, signal was not preserved",
        peak
    );

    // All samples should be finite
    for (i, &s) in output.iter().enumerate() {
        assert!(s.is_finite(), "sample {} not finite: {}", i, s);
    }
}

#[test]
fn test_oversampler_reset() {
    let channels = 2;
    let mut os = Oversampler::new(2, channels).unwrap();

    // Process some audio
    let num_frames = 512;
    let mut buffer = vec![0.5f32; num_frames * channels];
    os.process(&mut buffer, num_frames, |_planar, _frames| {})
        .unwrap();

    // Reset clears buffered audio and restores the fixed latency prefill.
    os.reset();
    assert_eq!(os.residual_frames, 0);
    assert_eq!(os.residual_out_frames, OS_CHUNK_SIZE);
    assert_eq!(os.residual_out_read, 0);
    assert!(
        os.residual_out[..OS_CHUNK_SIZE * channels]
            .iter()
            .all(|sample| *sample == 0.0)
    );
}

#[test]
fn test_oversampler_variable_small_blocks_keep_residual_cursors_valid() {
    let channels = 2;
    let mut os = Oversampler::new(2, channels).unwrap();
    let block_sizes = [17usize, 64, 191, 3, 512, 29, 257, 128];

    for (block_idx, &num_frames) in block_sizes.iter().cycle().take(32).enumerate() {
        let mut buffer: Vec<f32> = (0..num_frames * channels)
            .map(|i| ((block_idx * 31 + i) as f32 * 0.01).sin() * 0.25)
            .collect();

        let processed = os
            .process(&mut buffer, num_frames, |_planar, _frames| {})
            .unwrap();

        assert_eq!(processed, num_frames);
        assert!(buffer.iter().all(|s| s.is_finite()));
        assert!(os.residual_in_read + os.residual_frames <= os.residual_in.len() / channels);
        assert!(os.residual_out_read + os.residual_out_frames <= os.residual_out.len() / channels);
    }
}

#[test]
fn test_oversampled_plugin_latency() {
    use crate::parameters::{Parameter, ParameterId, ParameterValue};
    use crate::plugin::{PluginInfo, ProcessContext};

    /// Trivial passthrough plugin for testing
    struct PassthroughPlugin;
    impl InPlacePlugin for PassthroughPlugin {
        fn info(&self) -> PluginInfo {
            PluginInfo::new("Test", "1.0", "Test")
        }
        fn channels(&self) -> usize {
            2
        }
        fn parameters(&self) -> Vec<Parameter> {
            vec![]
        }
        fn set_parameter(
            &mut self,
            _: ParameterId,
            _: ParameterValue,
        ) -> crate::plugin::PluginResult<()> {
            Ok(())
        }
        fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
            None
        }
        fn process_in_place(
            &mut self,
            _buffer: &mut [f32],
            context: &ProcessContext,
        ) -> crate::plugin::PluginResult<usize> {
            Ok(context.num_frames)
        }
    }

    let os = OversampledPlugin::new(PassthroughPlugin, 2, 2).unwrap();
    // Should have non-zero latency from the oversampler
    assert!(
        os.latency_samples() > 0,
        "Oversampled plugin should have latency"
    );
}

#[test]
fn test_oversampled_plugin_processes_audio() {
    use crate::parameters::{Parameter, ParameterId, ParameterValue};
    use crate::plugin::{PluginInfo, ProcessContext};

    /// Plugin that doubles all samples (to verify processing happens at OS rate)
    struct DoublerPlugin;
    impl InPlacePlugin for DoublerPlugin {
        fn info(&self) -> PluginInfo {
            PluginInfo::new("Doubler", "1.0", "Test")
        }
        fn channels(&self) -> usize {
            1
        }
        fn parameters(&self) -> Vec<Parameter> {
            vec![]
        }
        fn set_parameter(
            &mut self,
            _: ParameterId,
            _: ParameterValue,
        ) -> crate::plugin::PluginResult<()> {
            Ok(())
        }
        fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
            None
        }
        fn process_in_place(
            &mut self,
            buffer: &mut [f32],
            context: &ProcessContext,
        ) -> crate::plugin::PluginResult<usize> {
            for s in buffer[..context.num_frames].iter_mut() {
                *s *= 2.0;
            }
            Ok(context.num_frames)
        }
    }

    let mut os = OversampledPlugin::new(DoublerPlugin, 2, 1).unwrap();
    os.initialize(48000).unwrap();

    // Pass the declared delay and one chunk of FIR settling before measuring gain.
    let ctx = ProcessContext::new(48000, 256);
    let mut buf = vec![0.5f32; 256];
    for _ in 0..os.latency_samples().div_ceil(256) + 1 {
        buf.fill(0.5);
        os.process_in_place(&mut buf, &ctx).unwrap();
    }

    // After pipeline is primed, output should be ~doubled (accounting for resampler delay)
    let mut buf2 = vec![0.5f32; 256];
    os.process_in_place(&mut buf2, &ctx).unwrap();
    for (index, sample) in buf2.iter().enumerate() {
        assert!(
            (sample - 1.0).abs() < 1e-4,
            "Doubler steady output sample {index}: {sample}"
        );
    }
}

#[test]
fn test_oversampled_plugin_propagates_inner_process_error() {
    use crate::parameters::{Parameter, ParameterId, ParameterValue};
    use crate::plugin::{PluginInfo, ProcessContext};

    struct ErrorPlugin;
    impl InPlacePlugin for ErrorPlugin {
        fn info(&self) -> PluginInfo {
            PluginInfo::new("Error", "1.0", "Test")
        }
        fn channels(&self) -> usize {
            1
        }
        fn parameters(&self) -> Vec<Parameter> {
            vec![]
        }
        fn set_parameter(
            &mut self,
            _: ParameterId,
            _: ParameterValue,
        ) -> crate::plugin::PluginResult<()> {
            Ok(())
        }
        fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
            None
        }
        fn process_in_place(
            &mut self,
            _buffer: &mut [f32],
            _context: &ProcessContext,
        ) -> crate::plugin::PluginResult<usize> {
            Err("inner failed".to_string())
        }
    }

    let mut os = OversampledPlugin::new(ErrorPlugin, 2, 1).unwrap();
    os.initialize(48000).unwrap();

    let ctx = ProcessContext::new(48000, 256);
    let mut buf = vec![0.0f32; 256];
    let err = os.process_in_place(&mut buf, &ctx).unwrap_err();

    assert!(err.contains("inner failed"));
}

#[test]
fn test_oversampler_invalid_factor() {
    assert!(Oversampler::new(1, 2).is_err());
    assert!(Oversampler::new(3, 2).is_err());
    assert!(Oversampler::new(0, 2).is_err());
    assert!(Oversampler::new(8, 2).is_err());
}

#[test]
fn test_oversampler_invalid_channels() {
    assert!(Oversampler::new(2, 0).is_err());
    assert!(Oversampler::new(2, 33).is_err());
}

#[test]
fn native_f64_inner_with_oversampling_uses_preallocated_host_conversion() {
    use crate::host::DawHost;
    use crate::parameters::{Parameter, ParameterId, ParameterValue};
    use crate::plugin::{Plugin, PluginInfo, ProcessContext};

    struct DualPrecisionGain {
        channels: usize,
        factor: u32,
    }
    impl Plugin for DualPrecisionGain {
        fn info(&self) -> PluginInfo {
            PluginInfo::new("DualPrecisionGain", "1", "test")
        }
        fn input_channels(&self) -> usize {
            self.channels
        }
        fn output_channels(&self) -> usize {
            self.channels
        }
        fn parameters(&self) -> Vec<Parameter> {
            Vec::new()
        }
        fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
            Ok(())
        }
        fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
            None
        }
        fn preferred_oversampling(&self) -> Option<u32> {
            Some(self.factor)
        }
        fn supports_f64(&self) -> bool {
            true
        }
        fn process(
            &mut self,
            input: &[f32],
            output: &mut [f32],
            context: &ProcessContext,
        ) -> Result<usize, String> {
            for (out, sample) in output.iter_mut().zip(input) {
                *out = *sample * 0.75;
            }
            Ok(context.num_frames)
        }
        fn process_f64(
            &mut self,
            input: &[f64],
            output: &mut [f64],
            context: &ProcessContext,
        ) -> Result<usize, String> {
            for (out, sample) in output.iter_mut().zip(input) {
                *out = *sample * 0.75;
            }
            Ok(context.num_frames)
        }
    }
    for factor in [2, 4] {
        for channels in [1, 2, 6] {
            let make_host = || {
                let mut host = DawHost::new(channels, 48_000);
                host.add_plugin(Box::new(DualPrecisionGain { channels, factor }))
                    .unwrap();
                host.build().unwrap();
                host
            };
            let mut host = make_host();
            let mut reference_host = make_host();
            let input: Vec<f64> = (0..8192 * channels)
                .map(|sample| ((sample as f64 * 0.013).sin() * 0.3) as f32 as f64)
                .collect();
            let reference_input: Vec<f32> = input.iter().map(|sample| *sample as f32).collect();
            let mut output = vec![0.0; input.len()];
            let mut reference = vec![0.0; input.len()];
            std::thread::spawn(move || {
                for frames in [1, 127, 256, 8192, 17, 4096] {
                    let count = frames * channels;
                    crate::assert_no_allocs("oversampled f64 host callback", || {
                        assert_eq!(
                            host.process_f64(&input[..count], &mut output[..count])
                                .unwrap(),
                            frames
                        );
                    });
                    assert_eq!(
                        reference_host
                            .process(&reference_input[..count], &mut reference[..count])
                            .unwrap(),
                        frames
                    );
                    for (&actual, &expected) in output[..count].iter().zip(&reference[..count]) {
                        assert_eq!(actual, f64::from(expected));
                    }
                }
            })
            .join()
            .unwrap();
        }
    }
}

#[test]
fn test_interleaved_to_planar_roundtrip() {
    let channels = 3;
    let frames = 4;
    let interleaved: Vec<f32> = (0..channels * frames).map(|i| i as f32).collect();

    let mut planar = vec![vec![0.0f32; frames]; channels];
    interleaved_to_planar(&interleaved, &mut planar, frames, channels);

    // Verify: planar[ch][frame] == interleaved[frame * channels + ch]
    for ch in 0..channels {
        for frame in 0..frames {
            assert_eq!(planar[ch][frame], interleaved[frame * channels + ch]);
        }
    }

    // Roundtrip back
    let mut result = vec![0.0f32; channels * frames];
    planar_to_interleaved(&planar, &mut result, frames, channels);
    assert_eq!(result, interleaved);
}

#[test]
fn wrappers_preserve_transport_in_oversampled_chunk_clock() {
    use crate::parameters::{Parameter, ParameterId, ParameterValue};
    use crate::plugin::{
        InPlacePluginAdapter, LoopRange, Plugin, PluginInfo, ProcessContext, TransportInfo,
    };
    use std::sync::{Arc, Mutex};

    type ContextLog = Arc<Mutex<Vec<(u32, usize, TransportInfo)>>>;
    struct ContextProbe(ContextLog);
    impl InPlacePlugin for ContextProbe {
        fn info(&self) -> PluginInfo {
            PluginInfo::new("Transport probe", "1.0", "Test")
        }
        fn channels(&self) -> usize {
            1
        }
        fn parameters(&self) -> Vec<Parameter> {
            vec![]
        }
        fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
            Ok(())
        }
        fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
            None
        }
        fn process_in_place(
            &mut self,
            _: &mut [f32],
            context: &ProcessContext,
        ) -> Result<usize, String> {
            self.0.lock().unwrap().push((
                context.sample_rate,
                context.num_frames,
                context.transport,
            ));
            Ok(context.num_frames)
        }
    }

    for factor in [2, 4] {
        for dynamic in [false, true] {
            for partition in [&[1][..], &[127][..], &[512][..], &[17, 255, 513, 3][..]] {
                let log: ContextLog = Arc::new(Mutex::new(Vec::with_capacity(16)));
                // Prepare platform mutex resources before measuring callbacks.
                log.lock().unwrap().clear();
                let probe = ContextProbe(Arc::clone(&log));
                let mut wrapper: Box<dyn Plugin> = if dynamic {
                    Box::new(
                        super::AutoOversampledPlugin::new(
                            Box::new(InPlacePluginAdapter::new(probe)),
                            factor,
                        )
                        .unwrap(),
                    )
                } else {
                    Box::new(InPlacePluginAdapter::new(
                        OversampledPlugin::new(probe, factor, 1).unwrap(),
                    ))
                };
                wrapper.initialize(48_000).unwrap();
                let input = [0.0; 513];
                let mut output = [0.0; 513];
                // Reset at a nonzero origin, then seek at a chunk boundary
                // without resetting the filter history.
                for (pass, origin) in [12_480, 40_123, 9_000].into_iter().enumerate() {
                    if pass == 1 {
                        wrapper.reset();
                    }
                    log.lock().unwrap().clear();
                    let mut position = 0;
                    let mut block = 0;
                    while position < 2048 {
                        let frames = partition[block % partition.len()].min(2048 - position);
                        let mut transport =
                            TransportInfo::at_sample(origin + position as u64, 48_000)
                                .with_tempo(93.0, 48_000)
                                .with_time_signature(7, 8)
                                .with_loop_range(LoopRange::new(8000, 50_000));
                        transport.playing = false;
                        transport.recording = true;
                        // The musical origin need not coincide with sample zero.
                        transport.ppq_position = 17.25 + position as f64 / 48_000.0 * 93.0 / 60.0;
                        let context = ProcessContext::new(48_000, frames).with_transport(transport);
                        crate::assert_no_allocs("oversampled transport", || {
                            wrapper
                                .process(&input[..frames], &mut output[..frames], &context)
                                .unwrap();
                        });
                        position += frames;
                        block += 1;
                    }
                    let entries = log.lock().unwrap();
                    assert_eq!(entries.len(), 8);
                    for (chunk, &(rate, frames, transport)) in entries.iter().enumerate() {
                        assert_eq!(rate, 48_000 * factor);
                        assert_eq!(frames, OS_CHUNK_SIZE * factor as usize);
                        assert_eq!(
                            transport.sample_position,
                            (origin + (chunk * OS_CHUNK_SIZE) as u64) * u64::from(factor)
                        );
                        let expected_ppq =
                            17.25 + (chunk * OS_CHUNK_SIZE) as f64 / 48_000.0 * 93.0 / 60.0;
                        assert!((transport.ppq_position - expected_ppq).abs() < 1e-12);
                        assert_eq!(transport.bpm, 93.0);
                        assert_eq!(transport.time_signature.numerator, 7);
                        assert_eq!(transport.time_signature.denominator, 8);
                        assert!(!transport.playing);
                        assert!(transport.recording && transport.looping);
                        assert_eq!(
                            transport.loop_range,
                            LoopRange::new(8000 * u64::from(factor), 50_000 * u64::from(factor))
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn fractional_inner_latency_is_reported_conservatively() {
    use crate::parameters::{Parameter, ParameterId, ParameterValue};
    use crate::plugin::{InPlacePluginAdapter, Plugin, PluginInfo, ProcessContext};

    struct SampleDelay {
        ring: Vec<f32>,
        position: usize,
    }
    impl InPlacePlugin for SampleDelay {
        fn info(&self) -> PluginInfo {
            PluginInfo::new("Sample delay", "1.0", "Test")
        }
        fn channels(&self) -> usize {
            1
        }
        fn parameters(&self) -> Vec<Parameter> {
            vec![]
        }
        fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
            Ok(())
        }
        fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
            None
        }
        fn latency_samples(&self) -> usize {
            self.ring.len()
        }
        fn process_in_place(
            &mut self,
            buffer: &mut [f32],
            context: &ProcessContext,
        ) -> Result<usize, String> {
            if !self.ring.is_empty() {
                for sample in buffer {
                    std::mem::swap(sample, &mut self.ring[self.position]);
                    self.position = (self.position + 1) % self.ring.len();
                }
            }
            Ok(context.num_frames)
        }
    }

    for factor in [2, 4] {
        for dynamic in [false, true] {
            let mut base_moment = 0.0;
            for delay in 0..8 {
                let inner = SampleDelay {
                    ring: vec![0.0; delay],
                    position: 0,
                };
                let mut wrapper: Box<dyn Plugin> = if dynamic {
                    Box::new(
                        super::AutoOversampledPlugin::new(
                            Box::new(InPlacePluginAdapter::new(inner)),
                            factor,
                        )
                        .unwrap(),
                    )
                } else {
                    Box::new(InPlacePluginAdapter::new(
                        OversampledPlugin::new(inner, factor, 1).unwrap(),
                    ))
                };
                wrapper.initialize(48_000).unwrap();
                let mut impulse = vec![0.0; 2048];
                impulse[0] = 1.0;
                let mut response = vec![0.0; impulse.len()];
                for (block, (src, dst)) in impulse
                    .chunks(127)
                    .zip(response.chunks_mut(127))
                    .enumerate()
                {
                    wrapper
                        .process(
                            src,
                            dst,
                            &ProcessContext::new(48_000, src.len())
                                .with_sample_position((block * 127) as u64),
                        )
                        .unwrap();
                }
                // For an FIR with nonzero DC response, its normalized first
                // impulse moment gives DC group delay, including subframes.
                let area: f64 = response.iter().map(|&x| f64::from(x)).sum();
                let moment: f64 = response
                    .iter()
                    .enumerate()
                    .map(|(i, &x)| i as f64 * f64::from(x))
                    .sum::<f64>()
                    / area;
                if delay == 0 {
                    base_moment = moment;
                }
                assert!((moment - base_moment - delay as f64 / f64::from(factor)).abs() < 0.002);
                let reported = wrapper.latency_samples() as f64;
                assert!(
                    reported >= moment - 0.002 && reported < moment + 1.002,
                    "factor={factor}, dynamic={dynamic}, inner={delay}: reported={reported}, measured={moment}"
                );
            }
        }
    }
}

#[test]
fn auto_oversampled_envelopes_dominate_live_declarations_in_every_state() {
    use super::AutoOversampledPlugin;
    use crate::parameters::{Parameter, ParameterId, ParameterValue};
    use crate::plugin::{Plugin, PluginInfo, ProcessContext};

    /// Frame-exact passthrough inner: copies input to output, returns
    /// the context frame count.
    struct PassthroughInner;
    impl Plugin for PassthroughInner {
        fn info(&self) -> PluginInfo {
            PluginInfo::new("PassthroughInner", "1.0", "test")
        }
        fn input_channels(&self) -> usize {
            2
        }
        fn output_channels(&self) -> usize {
            2
        }
        fn parameters(&self) -> Vec<Parameter> {
            Vec::new()
        }
        fn set_parameter(&mut self, id: ParameterId, _value: ParameterValue) -> Result<(), String> {
            Err(format!("unknown parameter: {id}"))
        }
        fn get_parameter(&self, _id: &ParameterId) -> Option<ParameterValue> {
            None
        }
        fn process(
            &mut self,
            input: &[f32],
            output: &mut [f32],
            context: &ProcessContext,
        ) -> Result<usize, String> {
            let samples = context.num_frames * 2;
            output[..samples].copy_from_slice(&input[..samples]);
            Ok(context.num_frames)
        }
    }

    // F2: the envelope contract requires envelope(n) >= live(n) in every
    // stream state. The wrapper publishes process Some(n) over live n and
    // drain Some(OS_CHUNK_SIZE) over live OS_CHUNK_SIZE; pin both across
    // fresh, fed, draining, and reset states, and prove the frame-exact
    // production the process envelope's proof relies on. (Content passes
    // through rate converters, so only the count is pinned.)
    let check = |wrapper: &AutoOversampledPlugin, state: &str| {
        for &frames in &[0usize, 1, 64, 8192] {
            assert_eq!(
                wrapper.output_frames_for_input(frames),
                frames,
                "{state}: live process declaration must stay identity"
            );
            assert_eq!(
                wrapper.output_frames_envelope(frames),
                Some(frames),
                "{state}: process envelope must stay identity"
            );
        }
        assert_eq!(
            wrapper.drain_output_frames_max(),
            OS_CHUNK_SIZE,
            "{state}: live drain bound must stay one chunk"
        );
        assert_eq!(
            wrapper.drain_frames_envelope(),
            Some(OS_CHUNK_SIZE),
            "{state}: drain envelope must stay one chunk"
        );
    };
    let mut wrapper = AutoOversampledPlugin::new(Box::new(PassthroughInner), 2).unwrap();
    wrapper.initialize(48_000).unwrap();
    check(&wrapper, "fresh");
    let input = vec![0.25f32; 256 * 2];
    let mut output = vec![0.0f32; 256 * 2];
    let produced = wrapper
        .process(&input, &mut output, &ProcessContext::new(48_000, 256))
        .unwrap();
    assert_eq!(produced, 256, "oversampled wrapper must stay frame-exact");
    check(&wrapper, "fed");
    wrapper
        .begin_drain(&ProcessContext::new(48_000, 0))
        .unwrap();
    check(&wrapper, "draining");
    let mut drain_out = vec![0.0f32; OS_CHUNK_SIZE * 2];
    let step = wrapper
        .drain(&mut drain_out, &ProcessContext::new(48_000, 0))
        .unwrap();
    assert!(
        step.frames <= OS_CHUNK_SIZE,
        "drain emitted {} past its {OS_CHUNK_SIZE}-frame bound",
        step.frames
    );
    check(&wrapper, "draining");
    wrapper.reset();
    check(&wrapper, "reset");
}
