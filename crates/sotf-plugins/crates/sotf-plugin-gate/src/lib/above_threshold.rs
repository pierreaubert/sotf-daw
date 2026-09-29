//! Above-threshold expansion and ducking with bounded, retained effect targets.

// Rust guideline compliant 2026-02-21
use crate::{GateMode, GatePlugin};
use math_audio_dsp::fast_math::{fast_log10, fast_pow10};

impl GatePlugin {
    fn above_threshold_magnitude(&self, level_db: f32, threshold_db: f32) -> f32 {
        let above = level_db - threshold_db;
        let hinge = if self.knee_db < 0.1 {
            above.max(0.0)
        } else if above <= -self.knee_db / 2.0 {
            0.0
        } else if above >= self.knee_db / 2.0 {
            above
        } else {
            let distance = above + self.knee_db / 2.0;
            distance * distance / (2.0 * self.knee_db)
        };
        (hinge * (self.ratio - 1.0)).min(self.effect_limit())
    }

    fn effect_limit(&self) -> f32 {
        if self.mode == GateMode::Upward {
            self.max_boost_db
        } else if self.range_db > 0.0 {
            self.range_db
        } else {
            // Preserve the documented finite limit for unlimited attenuation.
            240.0
        }
    }

    fn advance_effect(
        &mut self,
        channel: usize,
        level: f32,
        center: f32,
        opening: f32,
        closing: f32,
    ) -> f32 {
        let target = if level >= opening {
            self.gate_open[channel] = true;
            self.hold_counter[channel] = self.hold_samples;
            self.held_effect[channel] = self.above_threshold_magnitude(
                20.0 * fast_log10(level.max(super::consts::EPSILON)),
                center,
            );
            self.held_effect[channel]
        } else if self.gate_open[channel] && level >= closing {
            self.hold_counter[channel] = self.hold_samples;
            self.held_effect[channel]
        } else {
            self.gate_open[channel] = false;
            if self.hold_counter[channel] > 0 {
                self.hold_counter[channel] -= 1;
                self.held_effect[channel]
            } else {
                0.0
            }
        };
        let limit = self.effect_limit();
        self.held_effect[channel] = self.held_effect[channel].min(limit);
        let target = target.min(limit);
        // Above-threshold modes attack toward more effect and release toward
        // less effect. The downward gate retains its original timing path.
        let coefficient = if target > self.envelope[channel] {
            self.attack_coeff
        } else {
            self.release_coeff
        };
        self.envelope[channel] =
            (target + coefficient * (self.envelope[channel] - target)).min(limit);
        let signed_db = if self.mode == GateMode::Upward {
            self.envelope[channel]
        } else {
            -self.envelope[channel]
        };
        fast_pow10(signed_db / 20.0)
    }

    pub(super) fn process_above_threshold(
        &mut self,
        buffer: &mut [f32],
        frames: usize,
        stride: usize,
        external: bool,
        lookahead: bool,
    ) {
        let linked = self.link_channels && self.channels > 1;
        for frame in 0..frames {
            let center = self.threshold_smoother.advance();
            let activation_db = center
                - if self.knee_db < 0.1 {
                    0.0
                } else {
                    self.knee_db / 2.0
                };
            let opening = fast_pow10(activation_db / 20.0);
            let closing = opening * fast_pow10(-self.hysteresis_db / 20.0);
            let mix = self.mix_smoother.advance();
            let start = frame * stride;
            let sidechain = start + if external { self.channels } else { 0 };
            let mut maximum = 0.0_f32;
            for channel in 0..self.channels {
                let sample = finite_input(buffer[sidechain + channel]);
                let filtered = self.apply_sidechain_filter(channel, sample);
                let level = self.detect_level(channel, filtered);
                maximum = maximum.max(level);
                if frame + 1 == frames {
                    self.monitoring_levels[channel] =
                        20.0 * fast_log10(level.max(super::consts::EPSILON));
                }
                if !linked {
                    let gain = (1.0 - mix)
                        + mix * self.advance_effect(channel, level, center, opening, closing);
                    let input = finite_input(buffer[start + channel]);
                    let input = if lookahead {
                        self.lookahead_buffers[channel].push(input)
                    } else {
                        input
                    };
                    buffer[start + channel] = finite_output(input, gain);
                }
            }
            if linked {
                let gain =
                    (1.0 - mix) + mix * self.advance_effect(0, maximum, center, opening, closing);
                for channel in 0..self.channels {
                    let input = finite_input(buffer[start + channel]);
                    let input = if lookahead {
                        self.lookahead_buffers[channel].push(input)
                    } else {
                        input
                    };
                    buffer[start + channel] = finite_output(input, gain);
                }
            }
        }
    }
}

fn finite_input(sample: f32) -> f32 {
    if sample.is_finite() { sample } else { 0.0 }
}

fn finite_output(input: f32, gain: f32) -> f32 {
    let output = input * gain;
    if output.is_finite() {
        output
    } else {
        f32::MAX.copysign(input)
    }
}
