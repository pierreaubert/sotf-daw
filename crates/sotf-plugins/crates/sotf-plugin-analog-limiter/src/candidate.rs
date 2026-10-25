//! Qualify unadmitted neural color behind the deterministic limiter ceiling.
//!
//! This QA-only stream has no factory selector, production preset or drain
//! contract. It uses the actual limiter gain computer, continuously running
//! classical fallback and the production final sample clamp. Neural selection
//! never controls the threshold, release, lookahead or blend. Explicit core
//! automation uses the production validator. Switching is immediate and does
//! not establish click-free live activation or hardware-model admission.

// Rust guideline compliant 2026-02-21
use super::{AnalogLimiterPlugin, AnalogLimiterPluginParams, CORE_KEYS};
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_analog_common::AnalogColorStage;
use sotf_plugin_analog_common::hardware_candidate::{
    CandidateCounters, HardwareCandidate, HardwareQualification,
};

/// Mono QA stream with a learned color candidate inside the native safety bound.
pub struct LimiterQualification {
    limiter: AnalogLimiterPlugin,
    color: HardwareQualification,
    sample_rate: u32,
}

impl std::fmt::Debug for LimiterQualification {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LimiterQualification")
            .field("sample_rate", &self.sample_rate)
            .field("latency_samples", &self.latency_samples())
            .field("color", &self.color)
            .finish_non_exhaustive()
    }
}

impl LimiterQualification {
    /// Prepare a fresh stream, initially using its classical fallback.
    ///
    /// Construction allocates and belongs on the control thread. Candidate
    /// bytes/context/hash must already pass `HardwareCandidate::from_json`.
    /// A rejected or unavailable candidate is represented by `None`.
    ///
    /// # Errors
    /// Rejects invalid native controls, unsupported rate or candidate-rate mismatch.
    pub fn new(
        params: AnalogLimiterPluginParams,
        candidate: Option<HardwareCandidate>,
        sample_rate: u32,
    ) -> Result<Self, String> {
        if !(44_100..=192_000).contains(&sample_rate) {
            return Err("limiter qualification requires44.1..192kHz".into());
        }
        // Retain the production object as owner of validation, gain computation
        // and final clamp. Its internal color stage is not processed here.
        let mut limiter = AnalogLimiterPlugin::from_params(1, params.clone())?;
        limiter.initialize(f64::from(sample_rate))?;
        let mut fallback = AnalogColorStage::new(1);
        fallback.set_model_id(limiter.model_id)?;
        fallback.set_drive_db(params.analog_drive)?;
        fallback.set_color(params.analog_color)?;
        fallback.set_character(params.analog_character)?;
        fallback.set_output_trim_db(params.analog_trim)?;
        let color = HardwareQualification::new(fallback, candidate, sample_rate)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            limiter,
            color,
            sample_rate,
        })
    }

    /// Return the native core lookahead; neural coloration adds no lookahead.
    pub fn latency_samples(&self) -> usize {
        self.limiter.latency_samples()
    }

    /// Select QA output immediately while both color histories remain current.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.color.set_enabled(enabled);
    }

    /// Copy frame selection counters without allocating.
    pub fn counters(&self) -> CandidateCounters {
        self.color.counters()
    }

    /// Reset core, color histories and counters while retaining controls/selection.
    pub fn reset(&mut self) {
        self.limiter.reset();
        self.color.reset();
    }

    /// Apply explicit native core automation; the model cannot call this method.
    ///
    /// # Errors
    /// Rejects noncore controls and updates rejected by the production validator.
    /// In particular, lookahead cannot change on this prepared stream.
    pub fn set_core_parameter(
        &mut self,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<(), String> {
        if !CORE_KEYS.contains(&id.as_str()) {
            return Err("qualification core update cannot change color controls".into());
        }
        self.limiter.parametric_set_parameter(id, value)
    }

    /// Process the requested mono prefix through core, color and final clamp.
    ///
    /// No allocation, model loading, hashing, locks or learned control updates
    /// occur here. The sample ceiling is guaranteed only at fully wet mix,
    /// matching production. This is not an output inter-sample peak guarantee.
    ///
    /// # Errors
    /// Rejects rate or buffer geometry mismatch before mutating stream or output.
    pub fn process(
        &mut self,
        samples: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        if context.sample_rate != f64::from(self.sample_rate) || context.num_frames > samples.len()
        {
            return Err("limiter qualification needs matched mono callback geometry/rate".into());
        }
        let frames = context.num_frames;
        let used = &mut samples[..frames];
        let done = self.limiter.core.process_in_place(used, context)?;
        if done != frames {
            return Err("limiter qualification core frame count mismatch".into());
        }
        self.color
            .process(used, self.sample_rate)
            .expect("qualification owns matching immutable color sample rate");
        self.limiter.apply_final_ceiling(used);
        Ok(done)
    }
}
