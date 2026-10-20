//! Immutable reflection scenes and source-hop-aligned directional gains.

use super::binaural_decoder_plugin::{BinauralConfig, BinauralDecoderPlugin, BinauralFft};
use super::types::BinauralState;
use crate::hrtf::PreparedHrtfSpectra;
use sotf_host::sofa::{SofaFile, SourcePosition};
use std::sync::Arc;

// Four source hops of advertised latency plus one producer hop fit in eight
// slots. Output consumption is interleaved with source acceptance.
pub(super) const GAIN_HOPS: usize = 8;

#[derive(Default)]
pub(super) struct ReflectionScene {
    pub(super) tap_count: usize,
    pub(super) rays: Vec<ReflectionRay>,
}
pub(super) struct ReflectionRay {
    index: usize,
    gain: f32,
    azimuth: f32,
    elevation: f32,
}

impl BinauralDecoderPlugin {
    pub(super) fn configure_reflection_tracking(&mut self) {
        use std::collections::BTreeMap;
        let mut indices = BTreeMap::new();
        let mut base = Vec::new();
        for group in &self.room.reflection_groups {
            for tap in &group.taps {
                indices.insert((group.delay_samples, tap.channel), base.len());
                base.push([tap.left_gain, tap.right_gain]);
            }
        }
        let mut rays = Vec::new();
        for (channel, reflections) in self.room.cached_reflections.iter().enumerate() {
            for r in reflections {
                if r.gain != 0.0 {
                    rays.push(ReflectionRay {
                        index: indices[&(r.delay_samples, channel)],
                        gain: r.gain,
                        azimuth: r.azimuth_deg,
                        elevation: r.elevation_deg,
                    });
                }
            }
        }
        self.config.reflection_scene = Arc::new(ReflectionScene {
            tap_count: base.len(),
            rays,
        });
        self.room.reflection_gain_frames = vec![base.clone(); GAIN_HOPS];
        self.room.reflection_effective = base.clone();
        self.room.reflection_candidate = base.clone();
        self.room.reflection_base_gains = base;
        let old = self.state.load_full();
        let state = Arc::new(BinauralState {
            hrtf_filters_freq: old.hrtf_filters_freq.clone(),
            design_diffuse_field_eq: old.design_diffuse_field_eq.clone(),
            reflection_gains: Some(self.room.reflection_base_gains.clone()),
            _hrtf_data: old._hrtf_data.clone(),
        });
        self.state.store(Arc::clone(&state));
        self.crossfade.current_state_snapshot = state;
        self.crossfade.crossfade_prev_state = None;
        self.crossfade.crossfade_remaining = 0;
        self.crossfade.effective_previous_active = false;
        self.reset_reflection_tracking();
    }

    pub(super) fn prepare_reflection_gains(
        config: &BinauralConfig,
        fft: &BinauralFft,
        sofa: Option<&SofaFile>,
        prepared: Option<&PreparedHrtfSpectra>,
        angles: (f32, f32, f32),
    ) -> Vec<[f32; 2]> {
        let mut result = vec![[0.0; 2]; config.reflection_scene.tap_count];
        for ray in &config.reflection_scene.rays {
            let (az, el) = Self::rotate_speaker_position(
                ray.azimuth,
                ray.elevation,
                angles.0,
                angles.1,
                angles.2,
            );
            let ear = if let Some(sofa) = sofa {
                let target = SourcePosition::new(az, el, 1.0);
                let (nearest, gains) = crate::hrtf::find_hrtf_neighbors(sofa, &target);
                if let Some(prepared) = prepared {
                    crate::hrtf::interpolate_hrtf_broadband_prepared(&nearest, &gains, prepared)
                } else {
                    let (left, right) = crate::hrtf::interpolate_hrtf_frequency_domain(
                        &nearest,
                        &gains,
                        sofa,
                        config.fft_size,
                        config.sample_rate,
                        &fft.hrtf_r2c,
                        0.0,
                        az,
                        el,
                    );
                    let h = crate::room::ReflectionHrtf::from_freq_domain(left, right);
                    [h.left_gain_broadband, h.right_gain_broadband]
                }
            } else {
                let (left, right) = crate::room::azimuth_to_pan_gains(az.to_radians());
                [left, right]
            };
            for (out, gain) in result[ray.index].iter_mut().zip(ear) {
                *out += ray.gain * gain;
            }
        }
        result
    }

    pub(super) fn queue_reflection_gains(&mut self) {
        let current = &self.crossfade.current_state_snapshot;
        let active =
            self.crossfade.crossfade_remaining > 0 && self.crossfade.crossfade_prev_state.is_some();
        let alpha = if active {
            1.0 - self.crossfade.crossfade_remaining as f32 / self.crossfade.crossfade_total as f32
        } else {
            1.0
        };
        let slot = &mut self.room.reflection_gain_frames[self.room.reflection_gain_write];
        for (i, out) in slot.iter_mut().enumerate() {
            let target = current
                .reflection_gains
                .as_ref()
                .and_then(|g| g.get(i))
                .copied()
                .unwrap_or(self.room.reflection_base_gains[i]);
            let previous = if !active {
                target
            } else if self.crossfade.effective_previous_active {
                self.room.reflection_effective[i]
            } else {
                self.crossfade
                    .crossfade_prev_state
                    .as_ref()
                    .and_then(|s| s.reflection_gains.as_ref())
                    .and_then(|g| g.get(i))
                    .copied()
                    .unwrap_or(self.room.reflection_base_gains[i])
            };
            *out = std::array::from_fn(|ear| previous[ear] * (1.0 - alpha) + target[ear] * alpha);
        }
        self.room.reflection_gain_write = (self.room.reflection_gain_write + 1) & (GAIN_HOPS - 1);
    }

    pub(super) fn reset_reflection_tracking(&mut self) {
        self.room.reflection_gain_write = 0;
        self.room.reflection_gain_read = 0;
        self.room.reflection_gain_phase = 0;
        let current = self
            .crossfade
            .current_state_snapshot
            .reflection_gains
            .as_ref();
        for frame in &mut self.room.reflection_gain_frames {
            for (i, gain) in frame.iter_mut().enumerate() {
                *gain = current
                    .and_then(|g| g.get(i))
                    .copied()
                    .unwrap_or(self.room.reflection_base_gains[i]);
            }
        }
    }
}
