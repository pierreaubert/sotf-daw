//! Real encoder followed by native passive sum/difference decoder buses.

// Rust guideline compliant 2026-02-21
use sotf_plugins::{DawHost, MatrixPlugin, create_plugin};

#[test]
fn factory_ltrt_survives_native_passive_decoder_buses() {
    const FRAMES: usize = 24576;
    const ANALYSIS: usize = 8192;
    for rate in [44_100, 48_000, 96_000] {
        for (layout, channels, surrounds) in [("5.1", 6, vec![4, 5]), ("7.1", 8, vec![4, 5, 6, 7])]
        {
            let mut host = DawHost::new(channels, rate);
            host.add_plugin(
                create_plugin(
                    "downmix",
                    &serde_json::json!({
                        "input_layout":layout,"phase_coherence":false,"matrix_ltrt":true
                    }),
                    channels,
                    rate,
                )
                .unwrap(),
            )
            .unwrap();
            let gain = std::f32::consts::FRAC_1_SQRT_2;
            host.add_plugin(Box::new(
                MatrixPlugin::with_matrix(2, 2, vec![gain, gain, gain, -gain]).unwrap(),
            ))
            .unwrap();
            host.build().unwrap();
            let mut input = vec![0.0; FRAMES * channels];
            for frame in 0..FRAMES {
                let phase = std::f64::consts::TAU * frame as f64 / ANALYSIS as f64;
                input[frame * channels + 2] = (0.1 * (phase * 359.0).cos()) as f32;
                for &surround in &surrounds {
                    input[frame * channels + surround] = (0.1 * (phase * 173.0).cos()) as f32;
                }
            }
            let mut output = vec![0.0; FRAMES * 2];
            // Irregular callbacks include partial analysis hops.
            let mut position = 0;
            for n in [1, 31, 257, 4093].into_iter().cycle() {
                let n = n.min(FRAMES - position);
                if n == 0 {
                    break;
                }
                host.process(
                    &input[position * channels..(position + n) * channels],
                    &mut output[position * 2..(position + n) * 2],
                )
                .unwrap();
                position += n;
            }
            for (cycles, wanted_bus, amplitude, phase_offset) in [
                (359, 0, 0.1, 0.0),
                (
                    173,
                    1,
                    0.1 * surrounds.len() as f64,
                    -std::f64::consts::FRAC_PI_2,
                ),
            ] {
                let mut measured = [(0.0_f64, 0.0_f64); 2];
                for frame in 6144..6144 + ANALYSIS {
                    let phase = -std::f64::consts::TAU * cycles as f64 * (frame - 2048) as f64
                        / ANALYSIS as f64;
                    for bus in 0..2 {
                        let value = f64::from(output[frame * 2 + bus]) * 2.0 / ANALYSIS as f64;
                        measured[bus].0 += value * phase.cos();
                        measured[bus].1 += value * phase.sin();
                    }
                }
                let actual = measured[wanted_bus];
                let expected = (
                    amplitude * phase_offset.cos(),
                    amplitude * phase_offset.sin(),
                );
                assert!(
                    (actual.0 - expected.0).hypot(actual.1 - expected.1) < amplitude * 0.04,
                    "{layout} fs={rate} decoder bus {wanted_bus}: {actual:?} != {expected:?}"
                );
                let rejected = measured[1 - wanted_bus];
                assert!(rejected.0.hypot(rejected.1) < amplitude * 0.02);
            }
        }
    }
}
