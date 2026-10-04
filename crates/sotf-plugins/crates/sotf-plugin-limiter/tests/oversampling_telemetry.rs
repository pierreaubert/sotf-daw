//! Public meter oracles independent of contributor queues and DSP gain helpers.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_limiter::{LimiterData, LimiterPlugin, LimiterPluginParams};
use std::f64::consts::TAU;
use std::sync::Arc;

fn make(rate: u32, channels: usize, choice: i32, mix: f32, isp: bool) -> LimiterPlugin {
    let params: LimiterPluginParams = serde_json::from_value(serde_json::json!({
        "oversampling":choice,"threshold_db":-6.0,"release_ms":10.0,
        "lookahead_ms":0.25,"soft":false,"true_peak":isp,"isp_mode":isp,
        "dual_release":false,"mix":mix,"link_amount":1.0,
    }))
    .unwrap();
    let mut plugin = LimiterPlugin::from_params(channels, params);
    plugin.initialize(f64::from(rate)).unwrap();
    plugin
}

fn meter(plugin: &LimiterPlugin) -> Arc<LimiterData> {
    plugin
        .get_data()
        .unwrap()
        .downcast::<LimiterData>()
        .unwrap()
}

struct Publications {
    frames: usize,
    interval: usize,
    count: usize,
}
impl Publications {
    fn new(rate: u32) -> Self {
        Self {
            frames: 0,
            interval: rate as usize / 10,
            count: 0,
        }
    }
    fn advance(
        &mut self,
        frames: usize,
        plugin: &LimiterPlugin,
        check: &mut impl FnMut(usize, &LimiterData),
    ) {
        let previous = self.frames / self.interval;
        self.frames += frames;
        let current = self.frames / self.interval;
        assert!(
            current - previous <= 1,
            "observe every publication, never skip an interval"
        );
        if current != previous {
            self.count += 1;
            check(self.frames, &meter(plugin));
        }
    }
}

fn feed(
    plugin: &mut LimiterPlugin,
    rate: u32,
    input: &[f32],
    publications: &mut Publications,
    mix_events: &[(usize, f32)],
    check: &mut impl FnMut(usize, &LimiterData),
) -> Vec<f32> {
    let channels = plugin.channels();
    let mut output = input.to_vec();
    let mut position = 0;
    let mut calls = 0;
    while position < input.len() / channels {
        if let Some(&(_, mix)) = mix_events.iter().find(|&&(frame, _)| frame == position) {
            plugin
                .parametric_set_parameter(ParameterId::from("mix"), ParameterValue::Float(mix))
                .unwrap();
        }
        let boundary = mix_events
            .iter()
            .filter(|&&(frame, _)| frame > position)
            .map(|&(frame, _)| frame)
            .min()
            .unwrap_or(input.len() / channels);
        let frames = [1, 257, 63][calls % 3]
            .min(boundary - position)
            .min(input.len() / channels - position);
        assert_eq!(
            plugin
                .process_in_place(
                    &mut output[position * channels..(position + frames) * channels],
                    &ProcessContext::new(rate, frames)
                )
                .unwrap(),
            frames
        );
        publications.advance(frames, plugin, check);
        position += frames;
        calls += 1;
    }
    output
}

fn finish(
    plugin: &mut LimiterPlugin,
    rate: u32,
    publications: &mut Publications,
    check: &mut impl FnMut(usize, &LimiterData),
) {
    let channels = plugin.channels();
    let mut output = vec![0.0; 256 * channels];
    for call in 0..20_000 {
        let capacity = [1, 256, 17][call % 3];
        let drained = plugin
            .drain(
                &mut output[..capacity * channels],
                &ProcessContext::new(rate, 0),
            )
            .unwrap();
        publications.advance(drained.frames, plugin, check);
        if drained.complete {
            return;
        }
    }
    panic!("finite limiter drain did not finish");
}

fn neutral(_: usize, data: &LimiterData) {
    assert_eq!(
        data.gain_reduction_db, 0.0,
        "unity gain must publish exact zero"
    );
    assert!(!data.is_limiting);
}

#[test]
fn every_residual_phase_keeps_all_neutral_publications_zero_through_mix_reset_and_eos() {
    let rate = 48_000;
    let interval = rate as usize / 10;
    for choice in [1, 2] {
        for phase in 0..256 {
            let isp = phase % 4 == 0;
            let initial_mix = if isp { 1.0 } else { [0.0, 0.5, 1.0][phase % 3] };
            let mut plugin = make(rate, 2, choice, initial_mix, isp);
            // Every phase ends before the second publication, but close enough
            // that the finite converter continuation crosses that boundary.
            let limit = 2 * interval - 257;
            let frames = limit - (limit - phase) % 256;
            assert_eq!(frames % 256, phase);
            let frequency = [1000.0, 0.45 * f64::from(rate), 0.49 * f64::from(rate)][phase % 3];
            let input: Vec<_> = (0..frames)
                .flat_map(|frame| {
                    let sample =
                        (0.001 * (TAU * frequency * frame as f64 / f64::from(rate)).sin()) as f32;
                    [sample, -sample]
                })
                .collect();
            let events = if isp {
                vec![]
            } else {
                vec![(interval / 2, 0.75), (interval + 31, 0.25)]
            };
            for _epoch in 0..2 {
                plugin.reset();
                let mut publications = Publications::new(rate);
                feed(
                    &mut plugin,
                    rate,
                    &input,
                    &mut publications,
                    &events,
                    &mut |frame, data| {
                        neutral(frame, data);
                        // Initial cache peak is 0 dB. This independent input-level
                        // observation proves a real publication was inspected.
                        assert!((data.peak_db + 60.0).abs() < 0.1);
                    },
                );
                assert_eq!(publications.count, 1);
                finish(&mut plugin, rate, &mut publications, &mut neutral);
                assert_eq!(publications.count, 2, "choice{choice} phase{phase}");
            }
        }
    }
}

fn coherent_amplitude(signal: &[f32], frequency: f64, rate: u32) -> f64 {
    let (mut real, mut imaginary) = (0.0, 0.0);
    for (frame, &sample) in signal.iter().enumerate() {
        let phase = TAU * frequency * frame as f64 / f64::from(rate);
        real += f64::from(sample) * phase.cos();
        imaginary += f64::from(sample) * phase.sin();
    }
    2.0 * real.hypot(imaginary) / signal.len() as f64
}

// DC response of the independent finite Hann-sinc reconstruction used by the
// ceiling oracle. The phase sums are not normalized to unity: ignoring their
// small overshoot incorrectly treats legitimate ISP attenuation as meter error.
fn detector_dc_gain(rate: u32) -> f64 {
    let factor = if rate < 96_000 {
        4
    } else if rate < 192_000 {
        2
    } else {
        return 1.0;
    };
    (0..factor)
        .map(|phase| {
            (0..25)
                .filter_map(|past| {
                    let j = factor * past + phase;
                    if j > 48 {
                        return None;
                    }
                    let offset = j as f64 - 24.0;
                    let angle = std::f64::consts::PI * offset / factor as f64;
                    let sinc = if offset == 0.0 {
                        1.0
                    } else {
                        angle.sin() / angle
                    };
                    Some(sinc * 0.5 * (1.0 - (TAU * j as f64 / 48.0).cos()))
                })
                .sum::<f64>()
        })
        .fold(1.0, f64::max)
}

#[test]
fn independently_measured_neutral_filter_attenuation_is_not_gain_reduction() {
    for rate in [48_000, 96_000] {
        let interval = rate as usize / 10;
        for choice in [1, 2] {
            for frequency in [1000.0, 0.45 * f64::from(rate), 0.49 * f64::from(rate)] {
                let mut plugin = make(rate, 1, choice, 1.0, false);
                let input: Vec<_> = (0..4 * interval)
                    .map(|frame| {
                        (0.001 * (TAU * frequency * frame as f64 / f64::from(rate)).sin()) as f32
                    })
                    .collect();
                let mut publications = Publications::new(rate);
                let output = feed(
                    &mut plugin,
                    rate,
                    &input,
                    &mut publications,
                    &[],
                    &mut neutral,
                );
                assert_eq!(publications.count, 4);
                let source = coherent_amplitude(&input[3 * interval..], frequency, rate);
                let emitted = coherent_amplitude(&output[3 * interval..], frequency, rate);
                let loss_db = -20.0 * (emitted / source).log10();
                assert!(source > 0.00099 && emitted.is_finite());
                if frequency == 0.49 * f64::from(rate) {
                    assert!(
                        loss_db > 0.01,
                        "negative control needs real filter loss: rate{rate} choice{choice} loss{loss_db}"
                    );
                }
                eprintln!(
                    "NEUTRAL rate={rate} factor={} frequency={frequency} independently measured loss={loss_db:.6} dB, published GR=0",
                    1 << choice
                );
                finish(&mut plugin, rate, &mut publications, &mut neutral);
            }
        }
    }
}

#[test]
fn settled_dc_publishes_independent_limiting_gain_and_reset_clears_the_meter_epoch() {
    let mut maximum_error = 0.0_f64;
    for rate in [44_100, 48_000, 96_000, 192_000] {
        let interval = rate as usize / 10;
        for channels in [1, 2, 6] {
            for choice in [1, 2] {
                for (isp, mix) in [(false, 0.0), (false, 0.5), (false, 1.0), (true, 1.0)] {
                    let mut plugin = make(rate, channels, choice, mix, isp);
                    // A settled constant input has no interpolation alias or
                    // changing envelope. In ISP mode, the stricter native or
                    // high-rate reconstruction DC phase determines the limit.
                    // Serial stages enforce the same ceiling, so their DC
                    // calibration is a maximum, not a product of phase gains.
                    let amplitude = 4.0_f64;
                    let detector = if isp {
                        detector_dc_gain(rate).max(detector_dc_gain(rate * (1 << choice)))
                    } else {
                        1.0
                    };
                    let gain = 10.0_f64.powf(-6.0 / 20.0) / (amplitude * detector);
                    let effective_gain = (1.0 - f64::from(mix)) + f64::from(mix) * gain;
                    let expected_db = -20.0 * effective_gain.log10();
                    let input = vec![amplitude as f32; (10 * interval - 1) * channels];
                    let mut publications = Publications::new(rate);
                    let mut settled = |frame: usize, data: &LimiterData| {
                        // Five 100-ms periods exceed all converter support and
                        // 10-ms release settling; inspect every later update.
                        if frame >= 5 * interval {
                            let error = (f64::from(data.gain_reduction_db) - expected_db).abs();
                            maximum_error = maximum_error.max(error);
                            assert!(
                                error < 0.01,
                                "rate{rate} channels{channels} choice{choice} ISP{isp} mix{mix} frame{frame}: GR{} expected{expected_db} error{error}",
                                data.gain_reduction_db
                            );
                            assert_eq!(data.is_limiting, expected_db > 0.01);
                        }
                    };
                    feed(
                        &mut plugin,
                        rate,
                        &input,
                        &mut publications,
                        &[],
                        &mut settled,
                    );
                    assert_eq!(publications.count, 9);
                    finish(&mut plugin, rate, &mut publications, &mut settled);
                    assert_eq!(
                        publications.count, 10,
                        "observe final plateau interval during EOS"
                    );
                    plugin.reset();
                    let quiet: Vec<_> = (0..2 * interval * channels)
                        .map(|index| ((index as f64 * 0.171).sin() * 0.001) as f32)
                        .collect();
                    let mut publications = Publications::new(rate);
                    feed(
                        &mut plugin,
                        rate,
                        &quiet,
                        &mut publications,
                        &[],
                        &mut neutral,
                    );
                    assert_eq!(publications.count, 2);
                }
            }
        }
    }
    eprintln!("PLATEAU independent expected-gain maximum error={maximum_error:.9} dB");
}
