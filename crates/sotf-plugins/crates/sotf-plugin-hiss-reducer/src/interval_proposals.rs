//! Source-labelled Hiss interval inference for control threads and background workers.
//!
//! Requires an explicit model digest and exact recording context. Scores describe
//! similarity to labelled development data, not calibrated probabilities or safe
//! capture guarantees. This module never mutates a noise profile. Loading and
//! extraction allocate and must not run on the audio thread. No default model is
//! selected, and enabling the feature does not enable inference in the plugin.

// Rust guideline compliant 2026-02-21
use rustfft::{FftPlanner, num_complex::Complex};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{error::Error, fmt};

// These constants are the versioned Python training/inference contract.
const MAX_MODEL_BYTES: usize = 1_048_576;
const FEATURE_COUNT: usize = 16;
const CORE_NAMES: [&str; 8] = [
    "log_hf_flatness",
    "hf_fraction",
    "dominant_fraction",
    "envelope_cv",
    "log_rms_p95_p05",
    "mean_flux",
    "p95_flux",
    "zero_crossing_fraction",
];
const POLICY: &str = r#"{"ridge":0.01,"bootstrap_models":50,"bootstrap_seed":20711026,"minimum_source_groups":8,"minimum_class_source_groups":6,"minimum_noise_lower_score":0.95,"maximum_support_distance":1.5,"minimum_cv_noise_coverage":0.25,"maximum_cv_programme_proposals":0,"feature_scale_floor":0.05,"minimum_channel_rms":0.00001,"maximum_model_bytes":1048576}"#;

/// Recording geometry and semantic context required by an interval model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntervalContext {
    /// Physical sample rate in frames per second.
    pub sample_rate: usize,
    /// Number of interleaved physical channels.
    pub channels: usize,
    /// Hiss reducer cutoff in hertz.
    pub frequency_hz: f64,
    /// User-declared recording chain identifier.
    pub capture_setup_id: String,
    /// Must be `unwanted_stationary_hiss`.
    pub noise_definition: String,
}

impl IntervalContext {
    /// Checks the supported geometry and declared target.
    ///
    /// # Errors
    /// Returns an error for unsupported geometry, cutoff, setup identifier or target.
    pub fn validate(&self) -> Result<(), IntervalError> {
        if !(44_100..=192_000).contains(&self.sample_rate)
            || !(1..=8).contains(&self.channels)
            || !self.frequency_hz.is_finite()
            || !(1000.0..=16_000.0_f64.min(0.45 * self.sample_rate as f64))
                .contains(&self.frequency_hz)
            || !(1..=128).contains(&self.capture_setup_id.chars().count())
            || self.noise_definition != "unwanted_stationary_hiss"
        {
            return Err(IntervalError::Context);
        }
        Ok(())
    }
}

/// Reasons an interval inference request cannot be evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntervalError {
    /// Model exceeds the size bound or does not match the explicit digest.
    Integrity,
    /// Model schema, policy or numeric parameters are unsupported.
    Model,
    /// Recording context is invalid or differs from the model context.
    Context,
    /// PCM is not exactly one finite second within the supported amplitude bound.
    Pcm,
    /// At least one physical channel is silent or insufficiently excited.
    Excitation,
    /// Finite inputs produced an unrepresentable inference result.
    Numeric,
}

impl fmt::Display for IntervalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Integrity => "interval model size or SHA256 mismatch",
            Self::Model => "unsupported interval model schema or parameters",
            Self::Context => "invalid or mismatched interval context",
            Self::Pcm => "expected exactly one finite second of interleaved PCM",
            Self::Excitation => "every physical channel requires noise excitation",
            Self::Numeric => "non-finite interval inference result",
        })
    }
}

impl Error for IntervalError {}

fn sha256_hex(raw: &[u8]) -> String {
    Sha256::digest(raw)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Debug, Deserialize)]
struct Logistic {
    center: [f64; FEATURE_COUNT],
    scale: [f64; FEATURE_COUNT],
    weights: [f64; FEATURE_COUNT + 1],
}

impl Logistic {
    fn valid(&self) -> bool {
        self.center
            .iter()
            .chain(&self.weights)
            .all(|v| v.is_finite())
            && self.scale.iter().all(|v| v.is_finite() && *v >= 0.05)
    }

    fn probability(&self, feature: &[f64; FEATURE_COUNT]) -> Result<f64, IntervalError> {
        let logit = self.weights[FEATURE_COUNT]
            + (0..FEATURE_COUNT)
                .map(|i| ((feature[i] - self.center[i]) / self.scale[i]) * self.weights[i])
                .sum::<f64>();
        if !logit.is_finite() {
            return Err(IntervalError::Numeric);
        }
        // Stable sigmoid on both sides of zero.
        Ok(if logit >= 0.0 {
            1.0 / (1.0 + (-logit).exp())
        } else {
            let e = logit.exp();
            e / (1.0 + e)
        })
    }
}

#[derive(Debug, Deserialize)]
struct Parameters {
    main: Logistic,
    bootstrap: Vec<Logistic>,
    noise_support: Vec<[f64; FEATURE_COUNT]>,
}

#[derive(Debug, Deserialize)]
struct ModelDocument {
    format: String,
    admitted: bool,
    automatic_capture: bool,
    context: IntervalContext,
    feature_names: Vec<String>,
    policy: serde_json::Value,
    parameters: Parameters,
}

/// Validated, explicitly selected interval classifier with no capture capability.
#[derive(Debug)]
pub struct IntervalModel {
    context: IntervalContext,
    parameters: Parameters,
}

/// Conditional review scores; these are not calibrated noise probabilities.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct IntervalScores {
    /// Whether a person may review this interval as a possible noise sample.
    pub review_candidate: bool,
    /// Main logistic score.
    pub noise_score: f64,
    /// Fifth percentile of whole-source bootstrap scores.
    pub bootstrap_lower_score: f64,
    /// Nearest labelled noise feature distance in training scale units.
    pub nearest_noise_support_distance: f64,
    /// Always false; review never starts capture.
    pub automatic_capture: bool,
}

impl IntervalModel {
    /// Validates bounded model bytes against an explicit SHA256 and recording context.
    ///
    /// Loading allocates and belongs on a control thread. Bytes are hashed and
    /// parsed from the same slice; callers can perform their own bounded file I/O.
    ///
    /// # Errors
    /// Rejects integrity failures, unsupported models and context mismatches.
    pub fn from_bytes(
        raw: &[u8],
        expected_sha256: &str,
        context: &IntervalContext,
    ) -> Result<Self, IntervalError> {
        context.validate()?;
        if raw.len() > MAX_MODEL_BYTES
            || expected_sha256.len() != 64
            || sha256_hex(raw) != expected_sha256
        {
            return Err(IntervalError::Integrity);
        }
        let doc: ModelDocument = serde_json::from_slice(raw).map_err(|_| IntervalError::Model)?;
        let names: Vec<String> = ["channel_min_", "channel_max_"]
            .into_iter()
            .flat_map(|prefix| CORE_NAMES.map(|name| format!("{prefix}{name}")))
            .collect();
        let policy: serde_json::Value =
            serde_json::from_str(POLICY).map_err(|_| IntervalError::Model)?;
        let p = &doc.parameters;
        if doc.format != "sotf-hiss-interval-logistic-v1"
            || !doc.admitted
            || doc.automatic_capture
            || doc.feature_names != names
            || doc.policy != policy
            || !(48..=50).contains(&p.bootstrap.len())
            || !p.main.valid()
            || !p.bootstrap.iter().all(Logistic::valid)
            || !(1..=256).contains(&p.noise_support.len())
            || !p.noise_support.iter().flatten().all(|v| v.is_finite())
        {
            return Err(IntervalError::Model);
        }
        doc.context.validate()?;
        if doc.context != *context {
            return Err(IntervalError::Context);
        }
        Ok(Self {
            context: doc.context,
            parameters: doc.parameters,
        })
    }

    /// Returns the exact recording context this model accepts.
    pub fn context(&self) -> &IntervalContext {
        &self.context
    }

    /// Scores one complete physical-channel interval on a background or control thread.
    ///
    /// Feature extraction allocates. The returned review decision never changes
    /// the native processor or its stored profile.
    ///
    /// # Errors
    /// Rejects context changes, malformed PCM, unexcited channels or numeric overflow.
    pub fn evaluate(
        &self,
        pcm: &[f32],
        context: &IntervalContext,
    ) -> Result<IntervalScores, IntervalError> {
        if *context != self.context {
            return Err(IntervalError::Context);
        }
        self.score_features(&interval_features(pcm, context)?)
    }

    fn score_features(
        &self,
        feature: &[f64; FEATURE_COUNT],
    ) -> Result<IntervalScores, IntervalError> {
        let p = &self.parameters;
        let noise_score = p.main.probability(feature)?;
        let mut bootstrap = p
            .bootstrap
            .iter()
            .map(|m| m.probability(feature))
            .collect::<Result<Vec<_>, _>>()?;
        let lower = quantile(&mut bootstrap, 0.05);
        let mut distance = f64::INFINITY;
        for support in &p.noise_support {
            let squared = (0..FEATURE_COUNT)
                .map(|i| ((support[i] - feature[i]) / p.main.scale[i]).powi(2))
                .sum::<f64>();
            if !squared.is_finite() {
                return Err(IntervalError::Numeric);
            }
            distance = distance.min((squared / FEATURE_COUNT as f64).sqrt());
        }
        Ok(IntervalScores {
            review_candidate: lower >= 0.95 && distance <= 1.5,
            noise_score,
            bootstrap_lower_score: lower,
            nearest_noise_support_distance: distance,
            automatic_capture: false,
        })
    }
}

// NumPy's default linear quantile on a nonempty finite slice.
fn quantile(values: &mut [f64], q: f64) -> f64 {
    values.sort_unstable_by(f64::total_cmp);
    let at = (values.len() - 1) as f64 * q;
    let lo = at.floor() as usize;
    let hi = at.ceil() as usize;
    values[lo] + (values[hi] - values[lo]) * (at - lo as f64)
}

/// Extracts the versioned sixteen features from one complete interleaved second.
///
/// Uses full 20 ms periodic-Hann frames with 10 ms hops, without EOF padding.
/// Spectral mass doubles interior real-FFT bins; high-band flatness uses raw
/// powers. Each physical channel is normalized by its own one-second RMS.
/// Results contain eight channel minima followed by eight channel maxima.
/// This allocating operation belongs on a background or control thread.
///
/// # Errors
/// Rejects invalid context, PCM geometry/amplitude, insufficient excitation or non-finite features.
pub fn interval_features(
    pcm: &[f32],
    context: &IntervalContext,
) -> Result<[f64; FEATURE_COUNT], IntervalError> {
    context.validate()?;
    let rate = context.sample_rate;
    let channels = context.channels;
    if pcm.len() != rate * channels || pcm.iter().any(|v| !v.is_finite() || v.abs() > 32.0) {
        return Err(IntervalError::Pcm);
    }
    // Integer rates make the 20 ms size integral at usual audio rates. Ties-to-even
    // also matches Python for arbitrary supported integer sample rates.
    let n = (0.02 * rate as f64).round_ties_even() as usize;
    let hop = (0.01 * rate as f64).round_ties_even() as usize;
    let bins = n / 2 + 1;
    let frames = (rate - n) / hop + 1;
    let fft = FftPlanner::<f64>::new().plan_fft_forward(n);
    let mut scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
    let mut buffer = vec![Complex::default(); n];
    let window: Vec<_> = (0..n)
        .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos())
        .collect();
    let mut result = [f64::INFINITY; FEATURE_COUNT];
    result[8..].fill(f64::NEG_INFINITY);
    for channel in 0..channels {
        let rms = (pcm
            .iter()
            .skip(channel)
            .step_by(channels)
            .map(|v| f64::from(*v).powi(2))
            .sum::<f64>()
            / rate as f64)
            .sqrt();
        if rms < 1e-5 {
            return Err(IntervalError::Excitation);
        }
        let x: Vec<_> = pcm
            .iter()
            .skip(channel)
            .step_by(channels)
            .map(|v| f64::from(*v) / rms)
            .collect();
        let mut previous = vec![0.0; bins];
        let mut mass = vec![0.0; bins];
        let mut envelope = Vec::with_capacity(frames);
        let mut flux = Vec::with_capacity(frames - 1);
        let mut flat = 0.0;
        let mut fraction = 0.0;
        let mut dominant = 0.0;
        for (frame_index, start) in (0..=rate - n).step_by(hop).enumerate() {
            envelope
                .push((x[start..start + n].iter().map(|v| v * v).sum::<f64>() / n as f64).sqrt());
            for i in 0..n {
                buffer[i] = Complex::new(x[start + i] * window[i], 0.0);
            }
            fft.process_with_scratch(&mut buffer, &mut scratch);
            let mut hf_log = 0.0;
            let mut hf_sum = 0.0;
            let mut hf_count = 0;
            for i in 0..bins {
                let raw = buffer[i].norm_sqr();
                mass[i] = raw
                    * if i == 0 || (n.is_multiple_of(2) && i == n / 2) {
                        1.0
                    } else {
                        2.0
                    };
                if i as f64 * rate as f64 / n as f64 >= context.frequency_hz {
                    hf_log += raw.max(1e-12).ln();
                    hf_sum += raw.max(1e-12);
                    hf_count += 1;
                }
            }
            flat += hf_log / hf_count as f64 - (hf_sum / hf_count as f64).ln();
            let total = mass.iter().sum::<f64>().max(1e-20);
            let mut hf_mass = 0.0;
            let mut peak: f64 = 0.0;
            for (i, value) in mass.iter_mut().enumerate() {
                *value /= total;
                peak = peak.max(*value);
                if i as f64 * rate as f64 / n as f64 >= context.frequency_hz {
                    hf_mass += *value;
                }
            }
            fraction += hf_mass;
            dominant += peak;
            if frame_index > 0 {
                flux.push(
                    mass.iter()
                        .zip(&previous)
                        .map(|(a, b)| (a - b).abs())
                        .sum::<f64>(),
                );
            }
            previous.copy_from_slice(&mass);
        }
        let mean = envelope.iter().sum::<f64>() / frames as f64;
        let cv = (envelope.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / frames as f64).sqrt()
            / mean.max(1e-12);
        let ratio = (quantile(&mut envelope, 0.95).max(1e-12)
            / quantile(&mut envelope, 0.05).max(1e-12))
        .ln()
        .min(20.0);
        let flux_mean = flux.iter().sum::<f64>() / flux.len() as f64;
        let zcr = x
            .windows(2)
            .filter(|pair| pair[0].is_sign_negative() != pair[1].is_sign_negative())
            .count() as f64
            / (rate - 1) as f64;
        let each = [
            flat / frames as f64,
            fraction / frames as f64,
            dominant / frames as f64,
            cv,
            ratio,
            flux_mean,
            quantile(&mut flux, 0.95),
            zcr,
        ];
        for i in 0..8 {
            result[i] = result[i].min(each[i]);
            result[8 + i] = result[8 + i].max(each[i]);
        }
    }
    if result.iter().any(|v| !v.is_finite()) {
        return Err(IntervalError::Numeric);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL: &[u8] = include_bytes!("../../../models/hiss-intervals/model.synthetic.json");
    const PARITY: &str = include_str!("../tests/fixtures/interval-parity.json");

    fn context() -> IntervalContext {
        serde_json::from_value(
            serde_json::from_slice::<serde_json::Value>(MODEL).unwrap()["context"].clone(),
        )
        .unwrap()
    }

    fn load(raw: &[u8], ctx: &IntervalContext) -> Result<IntervalModel, IntervalError> {
        IntervalModel::from_bytes(raw, &sha256_hex(raw), ctx)
    }

    fn pcm(rate: usize, channels: usize, kind: &str, seed: u32) -> Vec<f32> {
        let mut state = seed;
        let mut samples = Vec::with_capacity(rate * channels);
        for frame in 0..rate {
            for channel in 0..channels {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (f64::from(state) / 4_294_967_296.0 - 0.5) * 0.02;
                let saw = ((frame * (channel + 3)) % 97) as f64 / 97.0 - 0.5;
                let envelope = 0.1 + (frame % 10007) as f64 / 10007.0;
                let value = if kind == "noise" {
                    noise
                } else {
                    noise * 0.1 + saw * envelope
                };
                samples.push((value / (channel + 1) as f64) as f32);
            }
        }
        samples
    }

    fn near(a: f64, b: f64, tolerance: f64) {
        assert!(
            (a - b).abs() <= tolerance,
            "{a:.17} != {b:.17}; tolerance {tolerance}"
        );
    }

    #[test]
    fn python_parity_across_rates_channels_and_odd_fft_size() {
        let fixture: serde_json::Value = serde_json::from_str(PARITY).unwrap();
        let model =
            IntervalModel::from_bytes(MODEL, fixture["model_sha256"].as_str().unwrap(), &context())
                .unwrap();
        for row in fixture["cases"].as_array().unwrap() {
            let ctx: IntervalContext = serde_json::from_value(row["context"].clone()).unwrap();
            let input = pcm(
                ctx.sample_rate,
                ctx.channels,
                row["kind"].as_str().unwrap(),
                row["seed"].as_u64().unwrap() as u32,
            );
            let bytes: Vec<_> = input.iter().flat_map(|v| v.to_le_bytes()).collect();
            assert_eq!(sha256_hex(&bytes), row["pcm_sha256"].as_str().unwrap());
            let features = interval_features(&input, &ctx).unwrap();
            for (actual, expected) in features.iter().zip(row["features"].as_array().unwrap()) {
                near(*actual, expected.as_f64().unwrap(), 1e-10);
            }
            // The private scorer comparison outside the trained context tests
            // arithmetic only. Public evaluation must reject these contexts.
            let actual = model.score_features(&features).unwrap();
            let expected = &row["scores"];
            near(
                actual.noise_score,
                expected["noise_score"].as_f64().unwrap(),
                1e-10,
            );
            near(
                actual.bootstrap_lower_score,
                expected["bootstrap_lower_score"].as_f64().unwrap(),
                1e-10,
            );
            near(
                actual.nearest_noise_support_distance,
                expected["nearest_noise_support_distance"].as_f64().unwrap(),
                1e-10,
            );
            assert_eq!(
                actual.review_candidate,
                expected["review_candidate"].as_bool().unwrap()
            );
            assert!(!actual.automatic_capture);
            if ctx == context() {
                assert_eq!(model.evaluate(&input, &ctx).unwrap(), actual);
            } else {
                assert_eq!(model.evaluate(&input, &ctx), Err(IntervalError::Context));
            }
        }
    }

    #[test]
    fn integrity_schema_policy_and_context_are_enforced() {
        let ctx = context();
        assert!(load(MODEL, &ctx).is_ok());
        assert!(matches!(
            IntervalModel::from_bytes(MODEL, &"0".repeat(64), &ctx),
            Err(IntervalError::Integrity)
        ));
        assert!(matches!(
            load(&vec![0; MAX_MODEL_BYTES + 1], &ctx),
            Err(IntervalError::Integrity)
        ));
        assert!(matches!(load(b"{}", &ctx), Err(IntervalError::Model)));
        for pointer in [
            "/admitted",
            "/automatic_capture",
            "/feature_names/0",
            "/policy/maximum_support_distance",
            "/parameters/main/scale/0",
            "/parameters/main/weights",
            "/parameters/noise_support",
            "/parameters/bootstrap",
        ] {
            let mut doc: serde_json::Value = serde_json::from_slice(MODEL).unwrap();
            let target = doc.pointer_mut(pointer).unwrap();
            *target = match pointer {
                "/admitted" => serde_json::json!(false),
                "/automatic_capture" => serde_json::json!(true),
                "/feature_names/0" => serde_json::json!("wrong"),
                "/parameters/main/scale/0" => serde_json::json!(0.049),
                "/policy/maximum_support_distance" => serde_json::json!(2.0),
                _ => serde_json::json!([]),
            };
            assert!(
                matches!(
                    load(&serde_json::to_vec(&doc).unwrap(), &ctx),
                    Err(IntervalError::Model)
                ),
                "{pointer}"
            );
        }
        let changed = IntervalContext {
            capture_setup_id: "other-chain".into(),
            ..ctx.clone()
        };
        assert!(matches!(load(MODEL, &changed), Err(IntervalError::Context)));
        for invalid in [
            IntervalContext {
                channels: 0,
                ..ctx.clone()
            },
            IntervalContext {
                sample_rate: 0,
                ..ctx.clone()
            },
            IntervalContext {
                frequency_hz: f64::NAN,
                ..ctx.clone()
            },
            IntervalContext {
                capture_setup_id: String::new(),
                ..ctx.clone()
            },
        ] {
            assert_eq!(invalid.validate(), Err(IntervalError::Context));
        }
    }

    #[test]
    fn incomplete_nonfinite_and_unexcited_physical_channels_abstain() {
        let ctx = context();
        assert_eq!(interval_features(&[], &ctx), Err(IntervalError::Pcm));
        assert_eq!(
            interval_features(&vec![0.0; ctx.sample_rate], &ctx),
            Err(IntervalError::Excitation)
        );
        let mut samples = pcm(ctx.sample_rate, 1, "noise", 42);
        for bad in [f32::NAN, f32::INFINITY, 32.01] {
            samples[0] = bad;
            assert_eq!(interval_features(&samples, &ctx), Err(IntervalError::Pcm));
        }
        let stereo = IntervalContext {
            channels: 2,
            ..ctx.clone()
        };
        let input: Vec<_> = pcm(ctx.sample_rate, 1, "noise", 42)
            .iter()
            .flat_map(|v| [*v, 0.0])
            .collect();
        assert_eq!(
            interval_features(&input, &stereo),
            Err(IntervalError::Excitation)
        );
    }

    #[test]
    fn extreme_finite_parameters_refuse_numeric_overflow() {
        let mut doc: serde_json::Value = serde_json::from_slice(MODEL).unwrap();
        doc["parameters"]["main"]["weights"][0] = serde_json::json!(1e308);
        let model = load(&serde_json::to_vec(&doc).unwrap(), &context()).unwrap();
        assert_eq!(
            model.score_features(&[1e308; FEATURE_COUNT]),
            Err(IntervalError::Numeric)
        );
    }
}
