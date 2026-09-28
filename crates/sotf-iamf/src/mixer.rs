// ============================================================================
// IAMF Mixer
// ============================================================================
//
// Combines rendered audio elements according to a mix presentation,
// applying element gains and output mix gain.
// Uses SIMD-accelerated accumulation via sotf-host when available.

use crate::error::{IamfError, IamfResult};
use crate::types::*;

use sotf_host::simd::{apply_gain_simd, scale_add_simd};

/// One animated MixGain segment (v1.1.0 §3.8.1): values in dB, evaluated
/// at `offset` samples into a `duration`-sample segment.
#[derive(Debug, Clone)]
pub struct GainCurve {
    pub animation: AnimationType,
    pub start_db: f32,
    pub end_db: f32,
    pub control_db: f32,
    /// Control-point time as a fraction of the segment duration (Bezier).
    pub control_time: f32,
    pub duration: u32,
}

impl GainCurve {
    /// Gain in dB at `offset` samples into the segment; clamps past the end.
    pub fn gain_db_at(&self, offset: u64) -> f32 {
        if self.duration == 0 {
            return self.end_db;
        }
        let t = (offset.min(self.duration as u64) as f32) / self.duration as f32;
        match self.animation {
            AnimationType::Step => self.start_db,
            AnimationType::Linear => self.start_db + (self.end_db - self.start_db) * t,
            AnimationType::Bezier => {
                let u = 1.0 - t;
                u * u * self.start_db + 2.0 * u * t * self.control_db + t * t * self.end_db
            }
        }
    }
}

/// A curve segment starting at an absolute stream sample position.
#[derive(Debug, Clone)]
pub struct GainSegment {
    pub start_sample: u64,
    pub curve: GainCurve,
}

/// Chained animation segments for one gain slot. Empty = hold the
/// persistent constant gain.
#[derive(Debug, Clone, Default)]
pub struct GainTrack {
    pub segments: Vec<GainSegment>,
}

impl GainTrack {
    /// Gain in dB at absolute stream position `pos`, or `None` when no
    /// segment covers it (caller holds the constant gain).
    pub fn gain_db_at(&self, pos: u64) -> Option<f32> {
        let seg = self.segments.iter().rfind(|s| s.start_sample <= pos)?;
        Some(seg.curve.gain_db_at(pos - seg.start_sample))
    }
}

/// Mix state for a single sub-mix
pub struct MixState {
    pub output_channels: usize,
    pub output_layout: IamfChannelLayout,
    /// Per-element gains in linear scale (persistent value after the last
    /// applied block; also the constant fast path when no track is active)
    pub element_gains: Vec<f32>,
    /// Output mix gain in linear scale (same persistence semantics)
    pub output_gain: f32,
    /// Per-element animation tracks (parallel to `element_gains`)
    element_tracks: Vec<GainTrack>,
    /// Output-gain animation track
    output_track: GainTrack,
    /// Stream position in samples at the next `mix_from_bufs` call
    position_samples: u64,
}

impl MixState {
    pub fn from_sub_mix(sub_mix: &SubMix) -> Self {
        let element_gains: Vec<f32> = sub_mix
            .element_mix_configs
            .iter()
            .map(|e| db_to_linear(e.mix_gain.default_mix_gain_db))
            .collect();
        let n = element_gains.len();

        Self {
            output_channels: sub_mix.output_layout.channel_count(),
            output_layout: sub_mix.output_layout,
            element_gains,
            output_gain: db_to_linear(sub_mix.output_mix_gain.default_mix_gain_db),
            element_tracks: vec![GainTrack::default(); n],
            output_track: GainTrack::default(),
            position_samples: 0,
        }
    }

    /// Build chained animation segments from every MixGain subblock of a
    /// parameter block, starting at the current stream position.
    fn track_for(param_block: &ParameterBlock, start_sample: u64) -> GainTrack {
        let mut segments = Vec::with_capacity(param_block.subblocks.len());
        let mut offset = 0u64;
        for sb in &param_block.subblocks {
            if let ParameterData::MixGain {
                animation_type,
                start_point_value,
                end_point_value,
                control_point_value,
                control_point_relative_time,
            } = &sb.param_data
            {
                segments.push(GainSegment {
                    start_sample: start_sample + offset,
                    curve: GainCurve {
                        animation: *animation_type,
                        start_db: *start_point_value,
                        end_db: *end_point_value,
                        control_db: *control_point_value,
                        control_time: *control_point_relative_time,
                        duration: sb.subblock_duration,
                    },
                });
            }
            offset += sb.subblock_duration as u64;
        }
        GainTrack { segments }
    }

    /// Persistent (post-block) linear gain: the last subblock's end value.
    fn end_gain(param_block: &ParameterBlock) -> Option<f32> {
        param_block.subblocks.iter().rev().find_map(|sb| {
            if let ParameterData::MixGain {
                end_point_value, ..
            } = &sb.param_data
            {
                Some(db_to_linear(*end_point_value))
            } else {
                None
            }
        })
    }

    /// Apply a parameter block to update mix gains.
    pub fn apply_parameter_block(&mut self, param_block: &ParameterBlock, sub_mix: &SubMix) {
        // Check if this parameter_id matches any element mix gain
        for (i, emc) in sub_mix.element_mix_configs.iter().enumerate() {
            if emc.mix_gain.parameter_id == param_block.parameter_id {
                if i >= self.element_tracks.len() {
                    self.element_tracks.resize(i + 1, GainTrack::default());
                    self.element_gains.resize(i + 1, 1.0);
                }
                self.element_tracks[i] = Self::track_for(param_block, self.position_samples);
                if let Some(g) = Self::end_gain(param_block) {
                    self.element_gains[i] = g;
                }
            }
        }

        // Check output mix gain
        if sub_mix.output_mix_gain.parameter_id == param_block.parameter_id {
            self.output_track = Self::track_for(param_block, self.position_samples);
            if let Some(g) = Self::end_gain(param_block) {
                self.output_gain = g;
            }
        }
    }

    /// Mix from pre-allocated element output buffers (avoids Vec<Vec<f32>> allocation).
    ///
    /// `element_out_bufs`: pre-allocated buffers, one per element (parallel to element_gains)
    /// `output`: final mix output, interleaved [frames × output_channels]
    /// `num_frames`: number of frames
    pub fn mix_from_bufs(
        &mut self,
        element_out_bufs: &[Vec<f32>],
        output: &mut [f32],
        num_frames: usize,
    ) -> IamfResult<()> {
        let out_len = num_frames * self.output_channels;
        if output.len() < out_len {
            return Err(IamfError::ParseError(format!(
                "Output buffer too small: need {out_len} samples, got {}",
                output.len()
            )));
        }
        output[..out_len].fill(0.0);

        let out_ch = self.output_channels;
        let pos = self.position_samples;
        for (elem_idx, elem_out) in element_out_bufs.iter().enumerate() {
            let src_len = elem_out.len().min(out_len);
            let src_frames = src_len / out_ch.max(1);
            // Animated gain: per-frame envelope over this block.
            // Constant gain: SIMD fast path.
            if let Some(track) = self.element_tracks.get(elem_idx)
                && track.gain_db_at(pos).is_some()
            {
                for f in 0..src_frames {
                    // Provably `Some`: the track covers `pos` and every
                    // segment clamps past its end.
                    let db = track.gain_db_at(pos + f as u64).unwrap_or(0.0);
                    let g = db_to_linear(db);
                    let base = f * out_ch;
                    for c in 0..out_ch {
                        output[base + c] += elem_out[base + c] * g;
                    }
                }
            } else {
                let gain = self.element_gains.get(elem_idx).copied().unwrap_or(1.0);
                scale_add_simd(&mut output[..src_len], &elem_out[..src_len], gain);
            }
        }

        // Output gain: animated per-frame, else SIMD constant.
        if let Some(start_db) = self.output_track.gain_db_at(pos) {
            for f in 0..num_frames {
                let g = db_to_linear(
                    self.output_track
                        .gain_db_at(pos + f as u64)
                        .unwrap_or(start_db),
                );
                if (g - 1.0).abs() > 1e-6 {
                    let base = f * out_ch;
                    for c in 0..out_ch {
                        output[base + c] *= g;
                    }
                }
            }
        } else if (self.output_gain - 1.0).abs() > 1e-6 {
            apply_gain_simd(&mut output[..out_len], self.output_gain);
        }

        self.position_samples = pos + num_frames as u64;
        Ok(())
    }

    /// Mix multiple element outputs into a single interleaved output buffer.
    ///
    /// `element_outputs`: one output buffer per audio element, each interleaved
    /// `output`: final mix output, interleaved [frames × output_channels]
    /// `num_frames`: number of frames
    pub fn mix(
        &mut self,
        element_outputs: &[Vec<f32>],
        output: &mut [f32],
        num_frames: usize,
    ) -> IamfResult<()> {
        self.mix_from_bufs(element_outputs, output, num_frames)
    }
}

fn db_to_linear(db: f32) -> f32 {
    sotf_host::db_to_linear(db)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_to_linear() {
        assert!((db_to_linear(0.0) - 1.0).abs() < 1e-6);
        assert!((db_to_linear(-6.0) - 0.501187).abs() < 1e-3);
        assert!((db_to_linear(6.0) - 1.995262).abs() < 1e-3);
    }

    #[test]
    fn test_mix_single_element() {
        let sub_mix = SubMix {
            num_audio_elements: 1,
            element_mix_configs: vec![ElementMixConfig {
                audio_element_id: 0,
                mix_gain: MixGainConfig {
                    parameter_id: 0,
                    default_mix_gain_db: 0.0,
                },
            }],
            output_mix_gain: MixGainConfig {
                parameter_id: 1,
                default_mix_gain_db: 0.0,
            },
            layouts: vec![SubMixLayout {
                layout: IamfChannelLayout::Stereo,
                loudness: LoudnessInfo {
                    info_type: 0,
                    integrated_loudness: -23.0,
                    digital_peak: -1.0,
                    true_peak: None,
                },
            }],
            output_layout: IamfChannelLayout::Stereo,
            loudness: LoudnessInfo {
                info_type: 0,
                integrated_loudness: -23.0,
                digital_peak: -1.0,
                true_peak: None,
            },
        };

        let mut state = MixState::from_sub_mix(&sub_mix);
        assert_eq!(state.output_channels, 2);
        assert!((state.output_gain - 1.0).abs() < 1e-6);

        // Mix a single stereo element
        let elem_output = vec![0.5_f32, -0.5, 0.25, -0.25]; // 2 frames
        let element_outputs = vec![elem_output];
        let mut output = vec![0.0_f32; 4];
        state.mix(&element_outputs, &mut output, 2).unwrap();

        assert!((output[0] - 0.5).abs() < 1e-6);
        assert!((output[1] - (-0.5)).abs() < 1e-6);
    }

    #[test]
    fn test_mix_with_gain() {
        let sub_mix = SubMix {
            num_audio_elements: 1,
            element_mix_configs: vec![ElementMixConfig {
                audio_element_id: 0,
                mix_gain: MixGainConfig {
                    parameter_id: 0,
                    default_mix_gain_db: -6.0, // ~0.5x
                },
            }],
            output_mix_gain: MixGainConfig {
                parameter_id: 1,
                default_mix_gain_db: 0.0,
            },
            layouts: vec![SubMixLayout {
                layout: IamfChannelLayout::Stereo,
                loudness: LoudnessInfo {
                    info_type: 0,
                    integrated_loudness: -23.0,
                    digital_peak: -1.0,
                    true_peak: None,
                },
            }],
            output_layout: IamfChannelLayout::Stereo,
            loudness: LoudnessInfo {
                info_type: 0,
                integrated_loudness: -23.0,
                digital_peak: -1.0,
                true_peak: None,
            },
        };

        let mut state = MixState::from_sub_mix(&sub_mix);
        let elem = vec![1.0_f32, -1.0];
        let mut output = vec![0.0_f32; 2];
        state.mix(&[elem], &mut output, 1).unwrap();

        // -6 dB ≈ 0.501
        assert!((output[0] - 0.501187).abs() < 1e-3);
        assert!((output[1] - (-0.501187)).abs() < 1e-3);
    }

    #[test]
    fn test_apply_parameter_block_element_gain() {
        let sub_mix = SubMix {
            num_audio_elements: 1,
            element_mix_configs: vec![ElementMixConfig {
                audio_element_id: 0,
                mix_gain: MixGainConfig {
                    parameter_id: 42,
                    default_mix_gain_db: 0.0,
                },
            }],
            output_mix_gain: MixGainConfig {
                parameter_id: 99,
                default_mix_gain_db: 0.0,
            },
            layouts: vec![SubMixLayout {
                layout: IamfChannelLayout::Stereo,
                loudness: LoudnessInfo {
                    info_type: 0,
                    integrated_loudness: -23.0,
                    digital_peak: -1.0,
                    true_peak: None,
                },
            }],
            output_layout: IamfChannelLayout::Stereo,
            loudness: LoudnessInfo {
                info_type: 0,
                integrated_loudness: -23.0,
                digital_peak: -1.0,
                true_peak: None,
            },
        };

        let mut state = MixState::from_sub_mix(&sub_mix);
        // 6 dB ≈ 1.995 linear
        let pb = ParameterBlock {
            parameter_id: 42,
            duration: 10,
            constant_subblock_duration: 10,
            subblocks: vec![ParameterSubblock {
                subblock_duration: 10,
                param_data: ParameterData::MixGain {
                    animation_type: AnimationType::Step,
                    start_point_value: 6.0,
                    end_point_value: 6.0,
                    control_point_value: 0.0,
                    control_point_relative_time: 0.0,
                },
            }],
        };
        state.apply_parameter_block(&pb, &sub_mix);
        assert!((state.element_gains[0] - 1.995262).abs() < 1e-3);
    }

    #[test]
    fn test_apply_parameter_block_output_gain() {
        let sub_mix = SubMix {
            num_audio_elements: 0,
            element_mix_configs: vec![],
            output_mix_gain: MixGainConfig {
                parameter_id: 7,
                default_mix_gain_db: 0.0,
            },
            layouts: vec![SubMixLayout {
                layout: IamfChannelLayout::Stereo,
                loudness: LoudnessInfo {
                    info_type: 0,
                    integrated_loudness: -23.0,
                    digital_peak: -1.0,
                    true_peak: None,
                },
            }],
            output_layout: IamfChannelLayout::Stereo,
            loudness: LoudnessInfo {
                info_type: 0,
                integrated_loudness: -23.0,
                digital_peak: -1.0,
                true_peak: None,
            },
        };

        let mut state = MixState::from_sub_mix(&sub_mix);
        let pb = ParameterBlock {
            parameter_id: 7,
            duration: 10,
            constant_subblock_duration: 10,
            subblocks: vec![ParameterSubblock {
                subblock_duration: 10,
                param_data: ParameterData::MixGain {
                    animation_type: AnimationType::Step,
                    start_point_value: -6.0,
                    end_point_value: -6.0,
                    control_point_value: 0.0,
                    control_point_relative_time: 0.0,
                },
            }],
        };
        state.apply_parameter_block(&pb, &sub_mix);
        assert!((state.output_gain - 0.501187).abs() < 1e-3);
    }

    #[test]
    fn test_mix_multiple_elements() {
        let sub_mix = SubMix {
            num_audio_elements: 2,
            element_mix_configs: vec![
                ElementMixConfig {
                    audio_element_id: 0,
                    mix_gain: MixGainConfig {
                        parameter_id: 0,
                        default_mix_gain_db: 0.0,
                    },
                },
                ElementMixConfig {
                    audio_element_id: 1,
                    mix_gain: MixGainConfig {
                        parameter_id: 1,
                        default_mix_gain_db: 0.0,
                    },
                },
            ],
            output_mix_gain: MixGainConfig {
                parameter_id: 2,
                default_mix_gain_db: 0.0,
            },
            layouts: vec![SubMixLayout {
                layout: IamfChannelLayout::Stereo,
                loudness: LoudnessInfo {
                    info_type: 0,
                    integrated_loudness: -23.0,
                    digital_peak: -1.0,
                    true_peak: None,
                },
            }],
            output_layout: IamfChannelLayout::Stereo,
            loudness: LoudnessInfo {
                info_type: 0,
                integrated_loudness: -23.0,
                digital_peak: -1.0,
                true_peak: None,
            },
        };

        let mut state = MixState::from_sub_mix(&sub_mix);
        let elem_a = vec![1.0_f32, 0.0];
        let elem_b = vec![0.0_f32, 1.0];
        let mut output = vec![0.0_f32; 2];
        state
            .mix_from_bufs(&[elem_a, elem_b], &mut output, 1)
            .unwrap();

        assert!((output[0] - 1.0).abs() < 1e-6);
        assert!((output[1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_mix_with_output_gain() {
        let sub_mix = SubMix {
            num_audio_elements: 1,
            element_mix_configs: vec![ElementMixConfig {
                audio_element_id: 0,
                mix_gain: MixGainConfig {
                    parameter_id: 0,
                    default_mix_gain_db: 0.0,
                },
            }],
            output_mix_gain: MixGainConfig {
                parameter_id: 1,
                default_mix_gain_db: -6.0,
            },
            layouts: vec![SubMixLayout {
                layout: IamfChannelLayout::Stereo,
                loudness: LoudnessInfo {
                    info_type: 0,
                    integrated_loudness: -23.0,
                    digital_peak: -1.0,
                    true_peak: None,
                },
            }],
            output_layout: IamfChannelLayout::Stereo,
            loudness: LoudnessInfo {
                info_type: 0,
                integrated_loudness: -23.0,
                digital_peak: -1.0,
                true_peak: None,
            },
        };

        let mut state = MixState::from_sub_mix(&sub_mix);
        let elem = vec![1.0_f32, 1.0];
        let mut output = vec![0.0_f32; 2];
        state.mix_from_bufs(&[elem], &mut output, 1).unwrap();

        assert!((output[0] - 0.501187).abs() < 1e-3);
        assert!((output[1] - 0.501187).abs() < 1e-3);
    }

    #[test]
    fn test_mix_missing_gain_defaults_to_unity() {
        // Element buffer present but no matching gain entry.
        let sub_mix = SubMix {
            num_audio_elements: 1,
            element_mix_configs: vec![ElementMixConfig {
                audio_element_id: 0,
                mix_gain: MixGainConfig {
                    parameter_id: 0,
                    default_mix_gain_db: 0.0,
                },
            }],
            output_mix_gain: MixGainConfig {
                parameter_id: 1,
                default_mix_gain_db: 0.0,
            },
            layouts: vec![SubMixLayout {
                layout: IamfChannelLayout::Stereo,
                loudness: LoudnessInfo {
                    info_type: 0,
                    integrated_loudness: -23.0,
                    digital_peak: -1.0,
                    true_peak: None,
                },
            }],
            output_layout: IamfChannelLayout::Stereo,
            loudness: LoudnessInfo {
                info_type: 0,
                integrated_loudness: -23.0,
                digital_peak: -1.0,
                true_peak: None,
            },
        };

        let mut state = MixState::from_sub_mix(&sub_mix);
        // Provide an extra element buffer with no corresponding gain.
        let elem_a = vec![0.5_f32, 0.5];
        let elem_b = vec![0.25_f32, -0.25];
        let mut output = vec![0.0_f32; 2];
        state
            .mix_from_bufs(&[elem_a, elem_b], &mut output, 1)
            .unwrap();

        // elem_a at unity + elem_b at default unity.
        assert!((output[0] - 0.75).abs() < 1e-6);
        assert!((output[1] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn test_mix_from_bufs_rejects_short_output_buffer() {
        // A crafted/shorted caller buffer must be a ParseError, not a
        // slicing panic in `output[..out_len].fill(0.0)`.
        let sub_mix = SubMix {
            num_audio_elements: 1,
            element_mix_configs: vec![ElementMixConfig {
                audio_element_id: 0,
                mix_gain: MixGainConfig {
                    parameter_id: 0,
                    default_mix_gain_db: 0.0,
                },
            }],
            output_mix_gain: MixGainConfig {
                parameter_id: 1,
                default_mix_gain_db: 0.0,
            },
            layouts: vec![SubMixLayout {
                layout: IamfChannelLayout::Stereo,
                loudness: LoudnessInfo {
                    info_type: 0,
                    integrated_loudness: -23.0,
                    digital_peak: -1.0,
                    true_peak: None,
                },
            }],
            output_layout: IamfChannelLayout::Stereo,
            loudness: LoudnessInfo {
                info_type: 0,
                integrated_loudness: -23.0,
                digital_peak: -1.0,
                true_peak: None,
            },
        };

        let mut state = MixState::from_sub_mix(&sub_mix);
        let elem = vec![0.5_f32, -0.5];
        // One stereo frame needs 2 samples; provide only 1.
        let mut output = vec![0.0_f32; 1];
        let err = state.mix_from_bufs(&[elem], &mut output, 1).unwrap_err();
        assert!(
            matches!(err, IamfError::ParseError(_)),
            "short output buffer must be a ParseError, got {err:?}"
        );
    }

    fn animated_sub_mix() -> SubMix {
        SubMix {
            num_audio_elements: 1,
            element_mix_configs: vec![ElementMixConfig {
                audio_element_id: 0,
                mix_gain: MixGainConfig {
                    parameter_id: 42,
                    default_mix_gain_db: 0.0,
                },
            }],
            output_mix_gain: MixGainConfig {
                parameter_id: 99,
                default_mix_gain_db: 0.0,
            },
            layouts: vec![SubMixLayout {
                layout: IamfChannelLayout::Stereo,
                loudness: LoudnessInfo {
                    info_type: 0,
                    integrated_loudness: -23.0,
                    digital_peak: -1.0,
                    true_peak: None,
                },
            }],
            output_layout: IamfChannelLayout::Stereo,
            loudness: LoudnessInfo {
                info_type: 0,
                integrated_loudness: -23.0,
                digital_peak: -1.0,
                true_peak: None,
            },
        }
    }

    fn mix_gain_block(
        parameter_id: u32,
        animation_type: AnimationType,
        start_db: f32,
        end_db: f32,
        control_db: f32,
        control_time: f32,
        duration: u32,
    ) -> ParameterBlock {
        ParameterBlock {
            parameter_id,
            duration,
            constant_subblock_duration: duration,
            subblocks: vec![ParameterSubblock {
                subblock_duration: duration,
                param_data: ParameterData::MixGain {
                    animation_type,
                    start_point_value: start_db,
                    end_point_value: end_db,
                    control_point_value: control_db,
                    control_point_relative_time: control_time,
                },
            }],
        }
    }

    #[test]
    fn test_linear_animation_ramps_gain() {
        let sub_mix = animated_sub_mix();
        let mut state = MixState::from_sub_mix(&sub_mix);
        // 0 dB → -6 dB linearly over 4 samples.
        let pb = mix_gain_block(42, AnimationType::Linear, 0.0, -6.0, 0.0, 0.0, 4);
        state.apply_parameter_block(&pb, &sub_mix);

        let elem = vec![1.0_f32; 8]; // 4 stereo frames of ones
        let mut output = vec![0.0_f32; 8];
        state.mix_from_bufs(&[elem], &mut output, 4).unwrap();

        // Frame 0 at t=0 holds 0 dB; frame 3 at t=3/4 is -4.5 dB.
        assert!((output[0] - 1.0).abs() < 1e-6);
        assert!((output[6] - db_to_linear(-4.5)).abs() < 1e-5);
        assert!((output[7] - db_to_linear(-4.5)).abs() < 1e-5);
        // Persistent gain settles at the block end value.
        assert!((state.element_gains[0] - db_to_linear(-6.0)).abs() < 1e-6);
    }

    #[test]
    fn test_bezier_animation_midpoint() {
        let sub_mix = animated_sub_mix();
        let mut state = MixState::from_sub_mix(&sub_mix);
        // 0 dB → 0 dB through a +6 dB control point: y(0.5) = +3 dB.
        let pb = mix_gain_block(42, AnimationType::Bezier, 0.0, 0.0, 6.0, 0.5, 4);
        state.apply_parameter_block(&pb, &sub_mix);

        let elem = vec![1.0_f32; 8];
        let mut output = vec![0.0_f32; 8];
        state.mix_from_bufs(&[elem], &mut output, 4).unwrap();

        assert!((output[0] - 1.0).abs() < 1e-6);
        assert!((output[4] - db_to_linear(3.0)).abs() < 1e-4);
    }

    #[test]
    fn test_step_animation_holds_start() {
        let sub_mix = animated_sub_mix();
        let mut state = MixState::from_sub_mix(&sub_mix);
        let pb = mix_gain_block(42, AnimationType::Step, 6.0, 6.0, 0.0, 0.0, 4);
        state.apply_parameter_block(&pb, &sub_mix);

        let elem = vec![1.0_f32; 8];
        let mut output = vec![0.0_f32; 8];
        state.mix_from_bufs(&[elem], &mut output, 4).unwrap();

        for sample in &output {
            assert!((sample - db_to_linear(6.0)).abs() < 1e-5);
        }
    }

    #[test]
    fn test_multi_subblock_chain() {
        let sub_mix = animated_sub_mix();
        let mut state = MixState::from_sub_mix(&sub_mix);
        // Two linear segments: 0 → -6 dB, then -6 → -12 dB.
        let pb = ParameterBlock {
            parameter_id: 42,
            duration: 4,
            constant_subblock_duration: 2,
            subblocks: vec![
                ParameterSubblock {
                    subblock_duration: 2,
                    param_data: ParameterData::MixGain {
                        animation_type: AnimationType::Linear,
                        start_point_value: 0.0,
                        end_point_value: -6.0,
                        control_point_value: 0.0,
                        control_point_relative_time: 0.0,
                    },
                },
                ParameterSubblock {
                    subblock_duration: 2,
                    param_data: ParameterData::MixGain {
                        animation_type: AnimationType::Linear,
                        start_point_value: -6.0,
                        end_point_value: -12.0,
                        control_point_value: 0.0,
                        control_point_relative_time: 0.0,
                    },
                },
            ],
        };
        state.apply_parameter_block(&pb, &sub_mix);

        let elem = vec![1.0_f32; 8];
        let mut output = vec![0.0_f32; 8];
        state.mix_from_bufs(&[elem], &mut output, 4).unwrap();

        assert!((output[0] - 1.0).abs() < 1e-6);
        assert!((output[2] - db_to_linear(-3.0)).abs() < 1e-5);
        assert!((output[4] - db_to_linear(-6.0)).abs() < 1e-5);
        assert!((output[6] - db_to_linear(-9.0)).abs() < 1e-5);
    }

    #[test]
    fn test_output_gain_animation() {
        let sub_mix = animated_sub_mix();
        let mut state = MixState::from_sub_mix(&sub_mix);
        // Output gain ramps 0 → -6 dB over 2 samples.
        let pb = mix_gain_block(99, AnimationType::Linear, 0.0, -6.0, 0.0, 0.0, 2);
        state.apply_parameter_block(&pb, &sub_mix);

        let elem = vec![1.0_f32; 4]; // 2 stereo frames
        let mut output = vec![0.0_f32; 4];
        state.mix_from_bufs(&[elem], &mut output, 2).unwrap();

        assert!((output[0] - 1.0).abs() < 1e-6);
        assert!((output[2] - db_to_linear(-3.0)).abs() < 1e-5);
        assert!((state.output_gain - db_to_linear(-6.0)).abs() < 1e-6);
    }
}
