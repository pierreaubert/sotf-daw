//! Qualify native neural candidates alongside continuously running classical fallback.
//!
//! This module is available only through `qa`. It accepts the offline candidate
//! format with deployment admission explicitly false. No production factory or
//! model ID selects it. Loading, hashing, context checks and preparation occur on
//! the control thread. Processing/reset/toggling use fixed storage without I/O,
//! locking, allocation or model replacement. Candidate history and the classical
//! stage run continuously, including while disabled. An unsupported input makes
//! every affected history window use classical output until valid history returns.
//! Switching is immediate for qualification; click-free live activation and actual
//! hardware/host admission remain separate work.

// Rust guideline compliant 2026-02-21
use crate::AnalogColorStage;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::backtrace::Backtrace;
use std::collections::BTreeMap;
use std::fmt;

/// Maximum candidate and expected-context JSON size, read on the control thread.
pub const MAX_CANDIDATE_BYTES: usize = 1024 * 1024;
const MAX_TAPS: usize = 64;
const MAX_HIDDEN: usize = 32;

/// Control-thread candidate loading or preparation failure.
#[derive(Debug)]
pub struct CandidateError {
    message: String,
    backtrace: Backtrace,
}

impl fmt::Display for CandidateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}\n{}", self.message, self.backtrace)
    }
}
impl std::error::Error for CandidateError {}
fn error(message: impl Into<String>) -> CandidateError {
    CandidateError {
        message: message.into(),
        backtrace: Backtrace::capture(),
    }
}

/// Rejected callback rate, without state/output mutation or realtime allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CandidateRateError {
    expected: u32,
    actual: u32,
}
impl fmt::Display for CandidateRateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "candidate callback rate {} differs from prepared {}",
            self.actual, self.expected
        )
    }
}
impl std::error::Error for CandidateRateError {}

#[derive(Deserialize)]
struct Architecture {
    taps: usize,
    hidden: usize,
    activation: String,
    history_order: String,
}
#[derive(Deserialize)]
struct Classical {
    degree: usize,
    weights: Vec<f64>,
}
#[derive(Deserialize)]
struct RawCandidate {
    format: String,
    context: Value,
    deployment_admitted: bool,
    architecture: Architecture,
    input_scale: f64,
    input_peak_limit: f64,
    linear: Vec<f64>,
    hidden_weights: Vec<Vec<f64>>,
    hidden_bias: Vec<f64>,
    output_weights: Vec<f64>,
    output_bias: f64,
    global_output_bound: f64,
    hammerstein: Classical,
    latency_samples: usize,
    history_frames: usize,
    policy: Value,
}

#[derive(Deserialize)]
struct Context {
    data_origin: String,
    sample_rate: u32,
    channels: usize,
    hardware: Hardware,
    calibration: Calibration,
    alignment: Alignment,
    rights: Rights,
}
#[derive(Deserialize)]
struct Hardware {
    manufacturer: String,
    model: String,
    unit_id: String,
    configuration_id: String,
    controls: BTreeMap<String, Value>,
}
#[derive(Deserialize)]
struct Calibration {
    calibration_id: String,
    measurement_reference: String,
    reference: String,
    shared_clock: bool,
    input_full_scale_dbu: f64,
    output_full_scale_dbu: f64,
    source_impedance_ohms: f64,
    load_impedance_ohms: f64,
}
#[derive(Deserialize)]
struct Alignment {
    paired_sample_aligned: bool,
    maximum_residual_delay_samples: f64,
    maximum_residual_drift_ppm: f64,
    measurement_reference: String,
}
#[derive(Deserialize)]
struct Rights {
    license: String,
    attribution: String,
    reference: String,
    training_authorized: bool,
}

fn identifier(s: &str, maximum: usize) -> bool {
    (1..=maximum).contains(&s.chars().count())
}
fn bounded(x: f64, low: f64, high: f64) -> bool {
    x.is_finite() && (low..=high).contains(&x)
}
fn validate_context(raw: &Value) -> Result<Context, CandidateError> {
    let ctx: Context = serde_json::from_value(raw.clone())
        .map_err(|e| error(format!("invalid candidate context: {e}")))?;
    let h = &ctx.hardware;
    let c = &ctx.calibration;
    let a = &ctx.alignment;
    let r = &ctx.rights;
    let ids = [
        &h.manufacturer,
        &h.model,
        &h.unit_id,
        &h.configuration_id,
        &c.calibration_id,
        &c.measurement_reference,
        &a.measurement_reference,
        &r.license,
        &r.attribution,
        &r.reference,
    ];
    if !["measured_hardware", "synthetic_validation"].contains(&ctx.data_origin.as_str())
        || !(44100..=192000).contains(&ctx.sample_rate)
        || ctx.channels != 1
        || ids.iter().any(|id| !identifier(id, 128))
        || !(1..=32).contains(&h.controls.len())
        || h.controls.iter().any(|(k, v)| {
            !identifier(k, 128)
                || match v {
                    Value::String(s) => !identifier(s, 128),
                    Value::Number(n) => n.as_f64().is_none_or(|v| !bounded(v, -1e6, 1e6)),
                    _ => true,
                }
        })
        || c.reference != "full_scale_sine_rms_dbu"
        || !c.shared_clock
        || !bounded(c.input_full_scale_dbu, -100.0, 100.0)
        || !bounded(c.output_full_scale_dbu, -100.0, 100.0)
        || !bounded(c.source_impedance_ohms, 0.0, 1e9)
        || !bounded(c.load_impedance_ohms, 1.0, 1e12)
        || !a.paired_sample_aligned
        || !bounded(a.maximum_residual_delay_samples, 0.0, 0.1)
        || !bounded(a.maximum_residual_drift_ppm, 0.0, 1.0)
        || !r.training_authorized
    {
        return Err(error(
            "candidate context violates the paired hardware contract",
        ));
    }
    Ok(ctx)
}

// Match Python's numeric context equality while retaining exact integer identity.
fn same_json(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => {
            let integer = |n: &serde_json::Number| {
                n.as_i64()
                    .map(i128::from)
                    .or_else(|| n.as_u64().map(i128::from))
            };
            match (a.is_f64(), b.is_f64()) {
                (false, false) => integer(a).zip(integer(b)).is_some_and(|(a, b)| a == b),
                (true, true) => a.as_f64() == b.as_f64(),
                _ => {
                    let (float, int) = if a.is_f64() {
                        (a.as_f64(), integer(b))
                    } else {
                        (b.as_f64(), integer(a))
                    };
                    float
                        .zip(int)
                        .is_some_and(|(f, i)| f.is_finite() && f.fract() == 0.0 && f as i128 == i)
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_json(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, a)| b.get(k).is_some_and(|b| same_json(a, b)))
        }
        _ => a == b,
    }
}

/// Validated fixed-size weights for an explicitly unadmitted qualification candidate.
#[derive(Clone, Debug)]
pub struct HardwareCandidate {
    sample_rate: u32,
    taps: usize,
    hidden: usize,
    input_scale: f64,
    input_peak: f64,
    linear: [f64; MAX_TAPS],
    weights: [[f64; MAX_HIDDEN]; MAX_TAPS],
    hidden_bias: [f64; MAX_HIDDEN],
    output_weights: [f64; MAX_HIDDEN],
    output_bias: f64,
}

impl HardwareCandidate {
    /// Load bounded bytes with explicit hash and exact hardware context.
    ///
    /// The caller owns filesystem access. This method performs allocation and
    /// hashing and belongs on the control thread. Metadata declarations do not
    /// constitute hardware admission. Production-admitted formats are refused.
    ///
    /// # Errors
    /// Rejects oversized bytes, SHA256/context mismatch, invalid calibration,
    /// architecture, coefficients, comparison policy, or software magnitude bound.
    pub fn from_json(
        bytes: &[u8],
        expected_sha256: &str,
        expected_context: &[u8],
    ) -> Result<Self, CandidateError> {
        if bytes.is_empty()
            || bytes.len() > MAX_CANDIDATE_BYTES
            || expected_context.len() > MAX_CANDIDATE_BYTES
            || expected_sha256.len() != 64
            || !expected_sha256
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            || !Sha256::digest(bytes)
                .iter()
                .zip(expected_sha256.as_bytes().as_chunks::<2>().0.iter())
                .all(|(byte, pair)| {
                    let hex = b"0123456789abcdef";
                    pair[0] == hex[usize::from(byte >> 4)] && pair[1] == hex[usize::from(byte & 15)]
                })
        {
            return Err(error("candidate size or explicit SHA256 mismatch"));
        }
        let raw: RawCandidate = serde_json::from_slice(bytes)
            .map_err(|e| error(format!("invalid candidate JSON: {e}")))?;
        let expected: Value = serde_json::from_slice(expected_context)
            .map_err(|e| error(format!("invalid expected context: {e}")))?;
        let context = validate_context(&raw.context)?;
        if !same_json(&raw.context, &expected) {
            return Err(error(
                "candidate hardware/calibration/rate context mismatch",
            ));
        }
        let a = raw.architecture;
        let policy = json!({"minimum_train_groups":8,"minimum_validation_groups":2,"minimum_test_groups":2,
            "maximum_nmse_db":-25.0,"minimum_hammerstein_advantage_db":1.0,"maximum_peak":0.999,
            "neural_ridge":1e-6,"hammerstein_ridge":1e-5,"seed":20261101});
        if raw.format != "sotf-analog-hardware-candidate-v1"
            || raw.deployment_admitted
            || !(1..=MAX_TAPS).contains(&a.taps)
            || !(2..=MAX_HIDDEN).contains(&a.hidden)
            || a.activation != "tanh"
            || a.history_order != "current_then_past"
            || raw.latency_samples != 0
            || raw.history_frames != a.taps - 1
            || !same_json(&raw.policy, &policy)
            || !bounded(raw.input_scale, 1e-4, 1.0)
            || !raw.input_peak_limit.is_finite()
            || raw.input_peak_limit <= 0.0
            || raw.input_peak_limit > 0.999
            || raw.linear.len() != a.taps
            || raw.hidden_weights.len() != a.taps
            || raw.hidden_weights.iter().any(|r| r.len() != a.hidden)
            || raw.hidden_bias.len() != a.hidden
            || raw.output_weights.len() != a.hidden
            || raw
                .linear
                .iter()
                .chain(raw.hidden_weights.iter().flatten())
                .chain(&raw.hidden_bias)
                .chain(&raw.output_weights)
                .any(|v| !bounded(*v, -16.0, 16.0))
            || !raw.output_bias.is_finite()
            || !raw.global_output_bound.is_finite()
            || raw.hammerstein.degree != 5
            || raw.hammerstein.weights.len() != 5 * a.taps + 1
            || raw
                .hammerstein
                .weights
                .iter()
                .any(|v| !bounded(*v, -1e6, 1e6))
        {
            return Err(error(
                "invalid candidate architecture, frozen policy or coefficients",
            ));
        }
        let bound = raw.input_peak_limit / raw.input_scale
            * raw.linear.iter().map(|v| v.abs()).sum::<f64>()
            + raw.output_weights.iter().map(|v| v.abs()).sum::<f64>()
            + raw.output_bias.abs();
        if !bounded(bound, 0.0, 128.0) || (bound - raw.global_output_bound).abs() > 1e-10 {
            return Err(error("candidate global magnitude bound mismatch"));
        }
        let mut result = Self {
            sample_rate: context.sample_rate,
            taps: a.taps,
            hidden: a.hidden,
            input_scale: raw.input_scale,
            input_peak: raw.input_peak_limit,
            linear: [0.0; MAX_TAPS],
            weights: [[0.0; MAX_HIDDEN]; MAX_TAPS],
            hidden_bias: [0.0; MAX_HIDDEN],
            output_weights: [0.0; MAX_HIDDEN],
            output_bias: raw.output_bias,
        };
        result.linear[..a.taps].copy_from_slice(&raw.linear);
        for (destination, source) in result.weights.iter_mut().zip(&raw.hidden_weights) {
            destination[..a.hidden].copy_from_slice(source);
        }
        result.hidden_bias[..a.hidden].copy_from_slice(&raw.hidden_bias);
        result.output_weights[..a.hidden].copy_from_slice(&raw.output_weights);
        Ok(result)
    }

    /// Return the number of current/past samples used by each prediction.
    pub fn taps(&self) -> usize {
        self.taps
    }

    /// Return the exact training/capture sample rate required for qualification.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

#[derive(Debug)]
struct Stream {
    model: HardwareCandidate,
    history: [f64; MAX_TAPS],
    valid: [bool; MAX_TAPS],
    cursor: usize,
}
impl Stream {
    fn new(model: HardwareCandidate) -> Self {
        Self {
            model,
            history: [0.0; MAX_TAPS],
            valid: [true; MAX_TAPS],
            cursor: 0,
        }
    }
    fn reset(&mut self) {
        self.history.fill(0.0);
        self.valid.fill(true);
        self.cursor = 0;
    }
    fn sample(&mut self, input: f32, enabled: bool) -> Option<Result<f32, ()>> {
        let m = &self.model;
        let valid = input.is_finite() && f64::from(input.abs()) <= m.input_peak;
        self.history[self.cursor] = if valid {
            f64::from(input) / m.input_scale
        } else {
            0.0
        };
        self.valid[self.cursor] = valid;
        let cursor = self.cursor;
        self.cursor = (self.cursor + 1) % MAX_TAPS;
        if !enabled {
            return None;
        }
        let mut u = [0.0; MAX_TAPS];
        for (k, value) in u[..m.taps].iter_mut().enumerate() {
            let index = (cursor + MAX_TAPS - k) % MAX_TAPS;
            if !self.valid[index] {
                return None;
            }
            *value = self.history[index];
        }
        let mut result = m.output_bias;
        for (&value, &weight) in u[..m.taps].iter().zip(&m.linear) {
            result += value * weight;
        }
        for j in 0..m.hidden {
            let mut z = m.hidden_bias[j];
            for (k, &value) in u[..m.taps].iter().enumerate() {
                z += value * m.weights[k][j];
            }
            result += z.tanh() * m.output_weights[j];
        }
        if !result.is_finite() || result.abs() > 128.0 {
            return Some(Err(()));
        }
        let output = result as f32;
        Some(Ok(if output.is_subnormal() { 0.0 } else { output }))
    }
}

/// Frame-level qualification decisions, copied without allocation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct CandidateCounters {
    /// Frames replaced by native neural output.
    pub neural: u64,
    /// Frames kept classical while qualification was disabled.
    pub disabled: u64,
    /// Enabled frames with a missing or rejected candidate.
    pub unavailable: u64,
    /// Frames whose current/past input exceeds the trained domain.
    pub out_of_domain: u64,
    /// Frames with an invalid calculated neural output.
    pub numerical: u64,
}

/// Fixed-storage candidate selection with caller-owned deterministic fallback.
///
/// The caller must continuously compute its own classical sample. Unsupported
/// clocks or controls invalidate the affected neural history windows. Disabled
/// selection still records valid input history. No model replacement is exposed.
#[derive(Debug)]
pub struct ExternalFallbackQualification {
    candidate: Option<Stream>,
    sample_rate: u32,
    counters: CandidateCounters,
}

impl ExternalFallbackQualification {
    /// Prepare a candidate stream for an immutable capture clock.
    ///
    /// # Errors
    /// Rejects unsupported clocks or a candidate captured at another rate.
    pub fn new(
        candidate: Option<HardwareCandidate>,
        sample_rate: u32,
    ) -> Result<Self, CandidateError> {
        if !(44100..=192000).contains(&sample_rate)
            || candidate
                .as_ref()
                .is_some_and(|c| c.sample_rate != sample_rate)
        {
            return Err(error("external fallback needs a matching capture clock"));
        }
        Ok(Self {
            candidate: candidate.map(Stream::new),
            sample_rate,
            counters: CandidateCounters::default(),
        })
    }

    /// Clear candidate history and counters without replacing its capture.
    pub fn reset(&mut self) {
        if let Some(candidate) = &mut self.candidate {
            candidate.reset();
        }
        self.counters = CandidateCounters::default();
    }

    /// Copy per-sample selection decisions without allocation.
    pub fn counters(&self) -> CandidateCounters {
        self.counters
    }

    /// Select a neural sample or preserve the caller's exact classical sample.
    ///
    /// Validity belongs to the entire tapped history. A clock/control mismatch
    /// stamps an invalid input rather than allowing stale-context history to be
    /// reused on return. This does not constrain the caller's fallback output.
    pub fn select(
        &mut self,
        input: f32,
        classical: f32,
        rate: u32,
        enabled: bool,
        controls_match: bool,
    ) -> f32 {
        let valid_context = rate == self.sample_rate && controls_match;
        let neural = self
            .candidate
            .as_mut()
            .and_then(|c| c.sample(if valid_context { input } else { f32::NAN }, enabled));
        if !enabled {
            self.counters.disabled = self.counters.disabled.saturating_add(1);
        } else if self.candidate.is_none() {
            self.counters.unavailable = self.counters.unavailable.saturating_add(1);
        } else {
            match neural {
                Some(Ok(output)) => {
                    self.counters.neural = self.counters.neural.saturating_add(1);
                    return output;
                }
                Some(Err(())) => {
                    self.counters.numerical = self.counters.numerical.saturating_add(1)
                }
                None => self.counters.out_of_domain = self.counters.out_of_domain.saturating_add(1),
            }
        }
        classical
    }
}

/// Explicit QA-only candidate stream with continuously prepared classical fallback.
pub struct HardwareQualification {
    classical: AnalogColorStage,
    candidate: Option<Stream>,
    sample_rate: u32,
    enabled: bool,
    counters: CandidateCounters,
}
impl fmt::Debug for HardwareQualification {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HardwareQualification")
            .field("sample_rate", &self.sample_rate)
            .field("enabled", &self.enabled)
            .field("candidate", &self.candidate.is_some())
            .field("counters", &self.counters)
            .finish_non_exhaustive()
    }
}
impl HardwareQualification {
    /// Prepare fresh mono qualification while retaining the chosen classical controls.
    ///
    /// Missing/rejected candidates are represented by `None`. This constructor
    /// allocates and resets; call it on the control thread. Qualification starts
    /// disabled and cannot grant production hardware admission.
    ///
    /// # Errors
    /// Rejects nonmono fallback, unsupported rate or candidate-rate mismatch.
    pub fn new(
        mut classical: AnalogColorStage,
        candidate: Option<HardwareCandidate>,
        sample_rate: u32,
    ) -> Result<Self, CandidateError> {
        if classical.spec.channels != 1
            || !(44100..=192000).contains(&sample_rate)
            || candidate
                .as_ref()
                .is_some_and(|c| c.sample_rate != sample_rate)
        {
            return Err(error(
                "qualification needs matched mono candidate and callback rate",
            ));
        }
        classical.prepare(sample_rate, 1).map_err(error)?;
        classical.reset();
        Ok(Self {
            classical,
            candidate: candidate.map(Stream::new),
            sample_rate,
            enabled: false,
            counters: CandidateCounters::default(),
        })
    }

    /// Toggle immediate qualification output while both histories continue running.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Clear both histories and counters without changing controls or enable state.
    pub fn reset(&mut self) {
        self.classical.reset();
        if let Some(c) = &mut self.candidate {
            c.reset();
        }
        self.counters = CandidateCounters::default();
    }

    /// Copy qualification frame counters without allocation.
    pub fn counters(&self) -> CandidateCounters {
        self.counters
    }

    /// Process mono input with fixed storage and immediate classical fallback.
    ///
    /// Nonfinite/out-of-domain input is handled by the native classical safety
    /// path and marks every affected neural history window invalid. Subsequent
    /// neural output resumes only with fully valid history. No model is dropped
    /// or replaced, and no I/O, hashing or allocation occurs here.
    ///
    /// # Errors
    /// A rate mismatch is rejected before output/history/counter mutation.
    pub fn process(
        &mut self,
        samples: &mut [f32],
        sample_rate: u32,
    ) -> Result<(), CandidateRateError> {
        if sample_rate != self.sample_rate {
            return Err(CandidateRateError {
                expected: self.sample_rate,
                actual: sample_rate,
            });
        }
        for value in samples {
            let input = *value;
            let mut fallback = [input];
            // Prepared mono geometry is owned here and cannot be invalidated.
            self.classical
                .process_interleaved(&mut fallback, 1)
                .expect("qualification owns prepared mono fallback");
            let neural = self
                .candidate
                .as_mut()
                .and_then(|c| c.sample(input, self.enabled));
            *value = fallback[0];
            if !self.enabled {
                self.counters.disabled = self.counters.disabled.saturating_add(1);
            } else if self.candidate.is_none() {
                self.counters.unavailable = self.counters.unavailable.saturating_add(1);
            } else {
                match neural {
                    Some(Ok(output)) => {
                        *value = output;
                        self.counters.neural = self.counters.neural.saturating_add(1);
                    }
                    Some(Err(())) => {
                        self.counters.numerical = self.counters.numerical.saturating_add(1)
                    }
                    None => {
                        self.counters.out_of_domain = self.counters.out_of_domain.saturating_add(1)
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrupted_internal_output_uses_fallback_and_counts_numerical_failure() {
        let bytes = include_bytes!("../../../models/analog-hardware/candidate.synthetic.json");
        let value: Value = serde_json::from_slice(bytes).unwrap();
        let mut hash = String::new();
        use std::fmt::Write;
        for b in Sha256::digest(bytes) {
            write!(&mut hash, "{b:02x}").unwrap();
        }
        let mut candidate = HardwareCandidate::from_json(
            bytes,
            &hash,
            &serde_json::to_vec(&value["context"]).unwrap(),
        )
        .unwrap();
        // Fault injection is private to this unit test; loaders reject this state.
        candidate.output_bias = f64::INFINITY;
        let mut qualification =
            HardwareQualification::new(AnalogColorStage::new(1), Some(candidate), 48_000).unwrap();
        qualification.set_enabled(true);
        let mut input = [0.1; 17];
        let mut expected = input;
        let mut reference = AnalogColorStage::new(1);
        reference.prepare(48_000, 1).unwrap();
        reference.reset();
        reference.process_interleaved(&mut expected, 17).unwrap();
        qualification.process(&mut input, 48_000).unwrap();
        assert_eq!(input, expected);
        assert_eq!(qualification.counters().numerical, 17);
    }
}
