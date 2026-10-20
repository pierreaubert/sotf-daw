//! First-frame and linked-channel safety of the post-render wet limiter.

// Rust guideline compliant 2026-02-21
use super::*;
use crate::initialize_tests::MatrixFile;
use crate::realtime_tests::callback_counts;

#[test]
fn finite_extreme_transients_preserve_ratios_and_diagnostic_bypass() {
    let mut plugin = XtcPlugin::new(XtcPluginParams::default(), 48_000).unwrap();
    plugin.initialize(48_000.0).unwrap();
    for peak in [
        1.0 + f32::EPSILON,
        1.001,
        2.0,
        3.0,
        127.0,
        32768.0,
        f32::MAX,
    ] {
        for sign in [-1.0, 1.0] {
            plugin.reset();
            let source = [sign * peak, -sign * peak * 0.125];
            let mut output = source;
            assert_eq!(
                callback_counts(|| plugin.apply_wet_limiter(&mut output, 1)),
                (0, 0)
            );
            assert!(output.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
            assert!((output[1] + output[0] * 0.125).abs() < 2e-7);
            assert!(output[0].signum() == source[0].signum());
        }
    }
    plugin.reset();
    let mut quiet = [0.125, -0.25];
    plugin.apply_wet_limiter(&mut quiet, 1);
    assert_eq!(quiet, [0.125, -0.25]);
    plugin.params.bypass_xtc_filters = true;
    let mut bypass = [4.0, -1.0];
    plugin.apply_wet_limiter(&mut bypass, 1);
    assert_eq!(bypass, [4.0, -1.0]);
    assert_eq!(plugin.dynamics.limiter_envelope, 1.0);
}

#[test]
fn first_transient_uses_one_gain_for_every_output_channel() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for channels in [2, 4, 8] {
            let fixture = MatrixFile::new(rate, channels);
            let mut plugin = XtcPlugin::new(fixture.params(), rate).unwrap();
            plugin.initialize(f64::from(rate)).unwrap();
            let source: Vec<f32> = (0..channels)
                .map(|ch| if ch == 0 { 4.0 } else { -1.0 / ch as f32 })
                .collect();
            let mut output = source.clone();
            assert_eq!(
                callback_counts(|| plugin.apply_wet_limiter(&mut output, 1)),
                (0, 0)
            );
            let gain = output[0] / source[0];
            let error = output
                .iter()
                .zip(&source)
                .map(|(a, b)| (a - b * gain).abs())
                .fold(0.0_f32, f32::max);
            println!(
                "rate={rate} channels={channels} linked_error={error:e} peak={}",
                output.iter().map(|v| v.abs()).fold(0.0_f32, f32::max)
            );
            assert!(
                output
                    .iter()
                    .all(|sample| sample.is_finite() && sample.abs() <= 1.0)
            );
            assert!(
                error < 2e-7,
                "first transient changed interchannel ratios: {error}"
            );
            assert!(gain > 0.0 && gain <= 0.25);
        }
    }
}

#[test]
fn transient_limiter_is_partition_invariant_and_resettable() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for channels in [2, 4, 8] {
            let fixture = MatrixFile::new(rate, channels);
            let make = || {
                let mut plugin = XtcPlugin::new(fixture.params(), rate).unwrap();
                plugin.initialize(f64::from(rate)).unwrap();
                plugin
            };
            let mut whole = make();
            let mut split = make();
            let source: Vec<f32> = (0..4097)
                .flat_map(|frame| {
                    (0..channels).map(move |ch| {
                        let scale = match frame % 517 {
                            0 => 16.0,
                            1 => -8.0,
                            _ => 0.1,
                        };
                        scale / (ch + 1) as f32
                    })
                })
                .collect();
            let mut expected = source.clone();
            whole.apply_wet_limiter(&mut expected, 4097);
            for shape in [1, 73, 1024] {
                split.reset();
                let mut actual = source.clone();
                for chunk in actual.chunks_mut(shape * channels) {
                    let frames = chunk.len() / channels;
                    assert_eq!(
                        callback_counts(|| split.apply_wet_limiter(chunk, frames)),
                        (0, 0)
                    );
                }
                assert_eq!(actual, expected);
                for (out, input) in actual
                    .chunks_exact(channels)
                    .zip(source.chunks_exact(channels))
                {
                    let gain = out[0] / input[0];
                    for (&a, &b) in out.iter().zip(input) {
                        assert!(a.abs() <= 1.0);
                        assert!((a - b * gain).abs() < 2e-7);
                    }
                }
            }
        }
    }
}
