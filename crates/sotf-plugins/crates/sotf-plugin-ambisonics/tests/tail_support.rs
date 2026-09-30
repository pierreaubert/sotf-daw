//! Matrix-only audio support is distinct from the recursive dual-band crossover.

// Rust guideline compliant 2026-02-21
use sotf_host::{Plugin, ProcessContext, TailLength};
use sotf_plugin_ambisonics::{AmbisonicsDecoderConfig, AmbisonicsDecoderPlugin};

fn plugin(
    order: usize,
    layout: &str,
    algorithm: &str,
    max_re: bool,
    dual: bool,
) -> AmbisonicsDecoderPlugin {
    AmbisonicsDecoderPlugin::new(&AmbisonicsDecoderConfig {
        order,
        target_layout: layout.to_owned(),
        max_re_weighting: max_re,
        dual_band: dual,
        algorithm: algorithm.to_owned(),
    })
    .unwrap()
}

#[test]
fn single_band_outputs_exact_zero_immediately_after_program_for_every_matrix_family() {
    let mut cases = 0;
    for algorithm in ["mode_matching", "allrad"] {
        for order in 1..=7 {
            for layout in ["5.1", "7.1.4", "9.1.6"] {
                for weighting in [false, true] {
                    let mut p = plugin(order, layout, algorithm, weighting, false);
                    assert_eq!(p.tail_length(), TailLength::Finite(0));
                    p.initialize(48000).unwrap();
                    let input: Vec<_> = (0..137 * p.input_channels())
                        .map(|i| ((i * 37 % 257) as f32 - 128.) / 1024.)
                        .collect();
                    let mut output = vec![1234.; 137 * p.output_channels()];
                    assert_eq!(
                        p.process(&input, &mut output, &ProcessContext::new(48000, 137))
                            .unwrap(),
                        137
                    );
                    assert!(output.iter().any(|x| x.abs() > 1e-5));
                    for frames in [1, 17, 137, 4097] {
                        let zeros = vec![0.; frames * p.input_channels()];
                        output.resize(frames * p.output_channels(), 1234.);
                        output.fill(1234.);
                        assert_eq!(
                            p.process(&zeros, &mut output, &ProcessContext::new(48000, frames))
                                .unwrap(),
                            frames
                        );
                        assert!(
                            output.iter().all(|&x| x == 0.),
                            "algorithm={algorithm} order={order} layout={layout}"
                        );
                    }
                    let mut sentinel = [1234., -5678.];
                    for _ in 0..2 {
                        assert_eq!(p.drain_call_bound().unwrap().get(), 1);
                        let result = p
                            .drain(&mut sentinel, &ProcessContext::new(48000, 0))
                            .unwrap();
                        assert_eq!(result.frames, 0);
                        assert!(result.complete);
                        assert_eq!(sentinel, [1234., -5678.]);
                        p.reset();
                        assert_eq!(p.tail_length(), TailLength::Finite(0));
                    }
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 84);
}

#[test]
fn dual_band_retains_actual_filter_response_and_unknown_audio_support() {
    for algorithm in ["mode_matching", "allrad"] {
        for order in 1..=7 {
            let mut p = plugin(order, "7.1.4", algorithm, true, true);
            assert_eq!(p.tail_length(), TailLength::Unknown);
            p.initialize(48000).unwrap();
            let mut impulse = vec![0.; p.input_channels()];
            impulse[0] = 0.25;
            let mut output = vec![0.; p.output_channels()];
            p.process(&impulse, &mut output, &ProcessContext::new(48000, 1))
                .unwrap();
            let zeros = vec![0.; 2048 * p.input_channels()];
            output.resize(2048 * p.output_channels(), 0.);
            p.process(&zeros, &mut output, &ProcessContext::new(48000, 2048))
                .unwrap();
            assert!(
                output.iter().any(|x| x.abs() > 1e-6),
                "recursive response was not exercised"
            );
            assert_eq!(p.tail_length(), TailLength::Unknown);
            // Work metadata describes the existing immediate drain, not audio support.
            assert_eq!(p.drain_call_bound().unwrap().get(), 1);
            assert!(
                p.drain(&mut [], &ProcessContext::new(48000, 0))
                    .unwrap()
                    .complete
            );
        }
    }
}
