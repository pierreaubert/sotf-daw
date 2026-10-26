//! QA-only bounded inverse proposals prepared entirely on the control thread.
//!
//! Import does not establish causal waveform accuracy or measured cancellation.
//! The fixed centered KEMAR candidate remains unadmitted. Native initialization
//! restores the configured deterministic source; reset retains the prepared proposal.

// Rust guideline compliant 2026-02-21
use crate::XtcPlugin;
use crate::filters::XtcFilters;
use crate::types::PendingFilterUpdate;
use rustfft::num_complex::Complex;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::sync::atomic::Ordering;

const BINS: usize = 1025;
const GAIN: f64 = 4.0;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    model_sha256: String,
    sample_rate: u32,
    fft_size: usize,
    speaker_azimuths_deg: [f64; 2],
    model_lookahead_samples: usize,
    filters_real_imag: Vec<[[[f64; 2]; 2]; 2]>,
    deployment_admitted: bool,
    native_imported: bool,
    scope: String,
}

/// Immutable, validated matrix resource for the centered44.1kHz QA profile.
pub struct PreparedProposal {
    filters: Arc<XtcFilters>,
    model_sha256: String,
    proposal_sha256: String,
    maximum_operator_gain: f64,
}
impl std::fmt::Debug for PreparedProposal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedProposal")
            .field("model_sha256", &self.model_sha256)
            .field("proposal_sha256", &self.proposal_sha256)
            .field("maximum_operator_gain", &self.maximum_operator_gain)
            .finish_non_exhaustive()
    }
}

fn operator_gain(m: [[Complex<f64>; 2]; 2]) -> f64 {
    let a = m[0][0].norm_sqr() + m[1][0].norm_sqr();
    let d = m[0][1].norm_sqr() + m[1][1].norm_sqr();
    let b = m[0][0].conj() * m[0][1] + m[1][0].conj() * m[1][1];
    (0.5 * (a + d + (a - d).hypot(2.0 * b.norm()))).sqrt()
}

impl PreparedProposal {
    /// Validate and prepare a proposal before touching a native plugin.
    ///
    /// The expected model hash is supplied independently by the caller. Matrix
    /// rows are speaker outputs; columns are intended-ear inputs.
    ///
    /// # Errors
    /// Rejects files over1MiB, identity/geometry/admission mismatches, nonfinite
    /// coefficients, complex real-FFT endpoints or operator gain above4.
    pub fn from_json(
        bytes: &[u8],
        proposal_sha256: &str,
        model_sha256: &str,
    ) -> Result<Self, String> {
        if bytes.len() > 1024 * 1024 {
            return Err("XTC proposal exceeds1MiB".into());
        }
        let actual: String = Sha256::digest(bytes)
            .iter()
            .map(|v| format!("{v:02x}"))
            .collect();
        if actual != proposal_sha256
            || model_sha256.len() != 64
            || !model_sha256.bytes().all(|v| v.is_ascii_hexdigit())
        {
            return Err("XTC proposal/model hash mismatch".into());
        }
        let doc: Document = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if doc.model_sha256 != model_sha256
            || doc.sample_rate != 44100
            || doc.fft_size != 2048
            || doc.speaker_azimuths_deg != [30.0, 330.0]
            || doc.model_lookahead_samples != 0
            || doc.deployment_admitted
            || doc.native_imported
            || doc.filters_real_imag.len() != BINS
            || doc.scope
                != "frequency-domain off-thread proposal; no causal renderer or true-peak claim"
        {
            return Err("unsupported XTC proposal contract".into());
        }
        let mut columns: [Vec<Complex<f32>>; 4] = std::array::from_fn(|_| Vec::with_capacity(BINS));
        let mut maximum_operator_gain = 0.0_f64;
        for (bin, values) in doc.filters_real_imag.iter().enumerate() {
            if values
                .iter()
                .flatten()
                .flatten()
                .any(|v| !v.is_finite() || v.abs() > GAIN + 1e-12)
            {
                return Err("XTC proposal coefficient outside bounded finite domain".into());
            }
            let mut matrix = values.map(|row| row.map(|v| Complex::new(v[0], v[1])));
            if operator_gain(matrix) > GAIN + 1e-12 {
                return Err("XTC proposal operator gain exceeds4".into());
            }
            if bin == 0 || bin == BINS - 1 {
                if matrix.iter().flatten().any(|v| v.im.abs() > 1e-12) {
                    return Err("XTC real-FFT endpoints must be real".into());
                }
                for value in matrix.iter_mut().flatten() {
                    value.im = 0.0;
                }
            }
            // Keep one real matrix scale and leave room for f32 rounding.
            // This guard is not a second independent coefficient knee.
            let narrowed = matrix.map(|row| row.map(|v| Complex::new(v.re as f32, v.im as f32)));
            let widened =
                narrowed.map(|row| row.map(|v| Complex::new(f64::from(v.re), f64::from(v.im))));
            let gain = operator_gain(widened);
            let ceiling = GAIN * (1.0 - 8.0 * f64::from(f32::EPSILON));
            let scale = if gain > ceiling {
                (ceiling / gain) as f32
            } else {
                1.0
            };
            let prepared = narrowed.map(|row| row.map(|v| v * scale));
            let widened =
                prepared.map(|row| row.map(|v| Complex::new(f64::from(v.re), f64::from(v.im))));
            let gain = operator_gain(widened);
            if gain > GAIN {
                return Err("XTC narrowed proposal exceeds operator budget".into());
            }
            maximum_operator_gain = maximum_operator_gain.max(gain);
            columns[0].push(prepared[0][0]);
            columns[1].push(prepared[0][1]);
            columns[2].push(prepared[1][0]);
            columns[3].push(prepared[1][1]);
        }
        let [filter_ll, filter_lr, filter_rl, filter_rr] = columns;
        Ok(Self {
            filters: Arc::new(XtcFilters {
                filter_ll,
                filter_lr,
                filter_rl: Some(filter_rl),
                filter_rr: Some(filter_rr),
                is_symmetric: false,
                speaker_filters: None,
            }),
            model_sha256: doc.model_sha256,
            proposal_sha256: actual,
            maximum_operator_gain,
        })
    }

    /// Read the largest singular value of the prepared f32 matrices.
    pub fn maximum_operator_gain(&self) -> f64 {
        self.maximum_operator_gain
    }

    /// Read a prepared complex coefficient without allocating.
    pub fn coefficient(&self, bin: usize, speaker: usize, input: usize) -> Option<[f32; 2]> {
        let vector = match (speaker, input) {
            (0, 0) => &self.filters.filter_ll,
            (0, 1) => &self.filters.filter_lr,
            (1, 0) => self.filters.filter_rl.as_ref()?,
            (1, 1) => self.filters.filter_rr.as_ref()?,
            _ => return None,
        };
        vector.get(bin).map(|v| [v.re, v.im])
    }
}

impl XtcPlugin {
    /// Read the prepared analysis window for independent offline reconstruction.
    pub fn proposal_analysis_window(&self) -> &[f32] {
        &self.fft.analysis_window
    }

    /// Read native IFFT/COLA scaling for independent offline reconstruction.
    pub fn proposal_output_scale(&self) -> f32 {
        self.fft.output_scale
    }

    /// Read an accepted native or learned matrix coefficient without allocation.
    pub fn prepared_filter_coefficient(
        &self,
        bin: usize,
        speaker: usize,
        input: usize,
    ) -> Option<[f32; 2]> {
        let f = &self.filter_state.cached_current_filters;
        if f.speaker_filters.is_some() {
            return None;
        }
        let vector = match (speaker, input) {
            (0, 0) => &f.filter_ll,
            (0, 1) => &f.filter_lr,
            (1, 0) => f.filter_rl.as_ref().unwrap_or(&f.filter_lr),
            (1, 1) => f.filter_rr.as_ref().unwrap_or(&f.filter_ll),
            _ => return None,
        };
        vector.get(bin).map(|v| [v.re, v.im])
    }

    /// Install a prepared QA matrix before accepted input, off the audio thread.
    ///
    /// Reset retains this resource. Successful initialize restores the configured
    /// native source. Only the aligned enabled/bypass control remains mutable.
    ///
    /// # Errors
    /// Rejects uninitialized/live/draining instances or a mismatched fixed profile.
    /// Rejection preserves the accepted filters and all audio state.
    pub fn install_learned_proposal(&mut self, proposal: PreparedProposal) -> Result<(), String> {
        if !self.initialized
            || self.drain_state.received_input
            || self.drain_state.remaining.is_some()
            || self.fft.fft_size != 2048
            || self.fft.sample_rate != 44100.0
            || self.params.source_mode != "synthetic"
            || self.filter_state.cached_current_filters.output_channels() != 2
            || self.params.room_reflections_enabled
            || self.params.auto_gain_enabled
            || self.params.bypass_xtc_filters
            || (self.params.distance_m - 1.4).abs() > 1e-6
            || self.params.speaker_angle_deg != 30.0
            || self.params.head_yaw_deg != 0.0
            || self.params.head_offset_x != 0.0
            || self.params.head_offset_z != 0.0
        {
            return Err("XTC proposal requires a reset, initialized centered44.1kHz/FFT2048 synthetic profile without AutoGain or room resources".into());
        }
        let generation = self
            .filter_state
            .filter_update_generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        let update = Arc::new(PendingFilterUpdate {
            generation,
            filters: Arc::clone(&proposal.filters),
            hrtf_transfer_functions: None,
            room_reflection_cache: None,
            room_params_hash: self.filter_state.room_params_hash,
        });
        self.filter_state.cached_current_filters = Arc::clone(&update.filters);
        self.filter_state.active_filter_update = Some(update);
        self.filter_state.hrtf_transfer_functions = None;
        self.filter_state.room_reflection_cache = None;
        self.filter_state.prev_filters = None;
        self.filter_state.crossfade_progress = 1.0;
        self.filter_state
            .exchange
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pending = None;
        self.qa_proposal = Some(proposal);
        Ok(())
    }

    /// Read the selected proposal identity without allocating.
    pub fn learned_proposal_sha256(&self) -> Option<&str> {
        self.qa_proposal
            .as_ref()
            .map(|p| p.proposal_sha256.as_str())
    }
}
