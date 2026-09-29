//! Successful-call bounds include zero-output backend work and ratio ramps.
// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, Plugin, ProcessContext};
use sotf_plugin_resampler::{ResamplerPlugin, ResamplerQuality};

#[test]
fn prepared_work_bound_covers_extreme_rates_small_chunks_and_partially_drained_states() {
    let mut worst = 0.0_f64;
    let mut cases = 0;
    for quality in [
        ResamplerQuality::Fast,
        ResamplerQuality::Medium,
        ResamplerQuality::High,
    ] {
        for (input_rate, output_rate) in [
            (8000, 384000),
            (384000, 8000),
            (44100, 48000),
            (48000, 44100),
        ] {
            for chunk in [1, 7, 256] {
                let mut plugin =
                    ResamplerPlugin::with_quality(1, input_rate, output_rate, chunk, quality)
                        .unwrap();
                plugin.initialize(input_rate).unwrap();
                plugin
                    .set_parameter(
                        ParameterId::from("dynamic_ratio"),
                        ParameterValue::Bool(true),
                    )
                    .unwrap();
                let nominal = f64::from(output_rate) / f64::from(input_rate);
                let mut buffer = vec![
                    0.0;
                    plugin
                        .drain_output_frames_max()
                        .max(plugin.output_frames_for_input(2 * chunk + 1))
                        .max(1)
                ];
                for relative in [0.5, 1.0, 2.0] {
                    for ramp in [false, true] {
                        for prior in [0, 1, 3] {
                            plugin.reset();
                            assert_eq!(plugin.drain_call_bound().unwrap().get(), 1);
                            let input = vec![0.1; chunk];
                            plugin
                                .process(
                                    &input,
                                    &mut buffer,
                                    &ProcessContext::new(input_rate, chunk),
                                )
                                .unwrap();
                            plugin.set_ratio(nominal * relative, ramp).unwrap();
                            // Keep a partial accepted input prefix where the chunk permits.
                            if chunk > 1 {
                                plugin
                                    .process(
                                        &[0.25],
                                        &mut buffer,
                                        &ProcessContext::new(input_rate, 1),
                                    )
                                    .unwrap();
                            }
                            let context = ProcessContext::new(input_rate, 0);
                            for _ in 0..prior {
                                if plugin.drain(&mut buffer, &context).unwrap().complete {
                                    break;
                                }
                            }
                            let bound = plugin.drain_call_bound().unwrap().get();
                            let mut observed = 0;
                            loop {
                                observed += 1;
                                assert!(
                                    observed <= bound,
                                    "quality={quality:?} rate={input_rate}/{output_rate} chunk={chunk} relative={relative} ramp={ramp} prior={prior} bound={bound}"
                                );
                                if plugin.drain(&mut buffer, &context).unwrap().complete {
                                    break;
                                }
                            }
                            worst = worst.max(bound as f64 / observed as f64);
                            cases += 1;
                            assert_eq!(plugin.drain_call_bound().unwrap().get(), 1);
                        }
                    }
                }
            }
        }
    }
    eprintln!("resampler drain quota: {cases} cases, worst bound/observed={worst:.3}");
}
