//! Independent moment checks for nonsubtractive, unshaped TPDF quantization.
//!
//! Lipshitz, Wannamaker and Vanderkooy, JAES 40(5), 1992, section 5.3:
//! two-LSB peak-to-peak triangular dither gives E[e] = 0 and E[e²] = Δ²/4
//! for every input away from clipping. These checks measure final PCM output,
//! including the precision of adding dither to a large input signal.

use sotf_host::{ParametricInPlacePlugin, plugin::ProcessContext};
use sotf_plugin_dither::{DitherPlugin, DitherPluginParams};

#[test]
fn tpdf_output_error_moments_do_not_depend_on_signal_level() {
    const FRAMES: usize = 262_144;
    const CHANNELS: usize = 8;
    const BLOCK: usize = 8191;
    let mut failures = Vec::new();
    for (bit_depth, bits) in [16, 20, 24].into_iter().enumerate() {
        let scale = 2.0_f64.powi(bits - 1);
        let lsb = 1.0 / scale;
        let levels = [
            0.0,
            (0.25 * lsb) as f32,
            (-0.5 * lsb) as f32,
            (0.125 + 0.25 * lsb) as f32,
            0.75,
            -0.75,
            (0.75 + 0.5 * lsb) as f32,
            (-0.75 - 0.5 * lsb) as f32,
        ];
        let mut plugin = DitherPlugin::from_params(
            CHANNELS,
            DitherPluginParams {
                bit_depth,
                noise_shaping: false,
                dither_type: 0,
            },
        );
        plugin.initialize(48_000.0).unwrap();
        let mut sums = [0.0_f64; CHANNELS];
        let mut squares = [0.0_f64; CHANNELS];
        let mut block = vec![0.0; BLOCK * CHANNELS];
        let mut remaining = FRAMES;
        while remaining > 0 {
            let frames = remaining.min(BLOCK);
            let samples = &mut block[..frames * CHANNELS];
            for frame in samples.as_chunks_mut::<CHANNELS>().0 {
                frame.copy_from_slice(&levels);
            }
            plugin
                .process_in_place(samples, &ProcessContext::new(48_000, frames))
                .unwrap();
            for frame in samples.as_chunks::<CHANNELS>().0 {
                for ch in 0..CHANNELS {
                    let error = (f64::from(frame[ch]) - f64::from(levels[ch])) * scale;
                    sums[ch] += error;
                    squares[ch] += error * error;
                }
            }
            remaining -= frames;
        }
        for ch in 0..CHANNELS {
            let mean = sums[ch] / FRAMES as f64;
            let second_moment = squares[ch] / FRAMES as f64;
            // More than eight standard errors for the mean, with deterministic
            // PRNG seeds. The expected second moment is in units of LSB².
            if mean.abs() >= 0.008 || (second_moment - 0.25).abs() >= 0.008 {
                failures.push(format!(
                    "{bits}-bit level {}: mean {mean} LSB, second moment {second_moment} LSB²",
                    levels[ch]
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
