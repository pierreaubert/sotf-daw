//! Fixed-size inference for explicit experimental residual echo models.
//!
//! Parse and verify on a control thread; inference owns no mutable shared state.
//! Audio integration must gate proposals by source endpoint/generation and keep
//! adaptive cancellation active. Loading a model alone never changes AEC audio.

// Rust guideline compliant 2026-02-21
use crate::residual_features::{BANDS, FEATURES, FeatureFrame};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;

const HIDDEN: usize = 24;
const MAX_BYTES: usize = 262_144;
const GAIN_FLOOR: f32 = 0.25;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelFile {
    format: String,
    feature_version: u32,
    bands: usize,
    features: usize,
    hidden: usize,
    gain_floor: f32,
    w1: Vec<Vec<f32>>,
    b1: Vec<f32>,
    w2: Vec<f32>,
    b2: f32,
    #[serde(default, rename = "training")]
    _training: serde_json::Value,
}

/// Immutable, bounded, shared-weight band mask estimator.
#[derive(Debug)]
pub struct ResidualMaskModel {
    w1: [[f32; FEATURES]; HIDDEN],
    b1: [f32; HIDDEN],
    w2: [f32; HIDDEN],
    b2: f32,
}

impl ResidualMaskModel {
    /// Validate explicitly pinned model bytes outside the processing callback.
    ///
    /// # Errors
    /// Rejects oversized input, hash mismatch, unsupported contracts, invalid
    /// dimensions, non-finite weights and weights with magnitude above 100.
    pub fn from_bytes(bytes: &[u8], expected_sha256: &str) -> Result<Self, String> {
        if bytes.len() > MAX_BYTES {
            return Err("AEC residual model exceeds 256 KiB".into());
        }
        let mut hash = String::with_capacity(64);
        for byte in Sha256::digest(bytes) {
            write!(hash, "{byte:02x}").expect("writing to String cannot fail");
        }
        if hash != expected_sha256 {
            return Err("AEC residual model SHA-256 mismatch".into());
        }
        let file: ModelFile = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if file.format != "sotf_aec_residual_mask_v1"
            || file.feature_version != 1
            || file.bands != BANDS
            || file.features != FEATURES
            || file.hidden != HIDDEN
            || file.gain_floor != GAIN_FLOOR
        {
            return Err("AEC residual feature/model contract mismatch".into());
        }
        if file.w1.len() != HIDDEN
            || file.w1.iter().any(|row| row.len() != FEATURES)
            || file.b1.len() != HIDDEN
            || file.w2.len() != HIDDEN
        {
            return Err("AEC residual model dimensions mismatch".into());
        }
        if file
            .w1
            .iter()
            .flatten()
            .chain(&file.b1)
            .chain(&file.w2)
            .chain(std::iter::once(&file.b2))
            .any(|value| !value.is_finite() || value.abs() > 100.0)
        {
            return Err("AEC residual model contains invalid weights".into());
        }
        let mut w1 = [[0.0; FEATURES]; HIDDEN];
        for (out, row) in w1.iter_mut().zip(file.w1) {
            out.copy_from_slice(&row);
        }
        Ok(Self {
            w1,
            b1: file.b1.try_into().map_err(|_| "invalid first bias")?,
            w2: file.w2.try_into().map_err(|_| "invalid second weights")?,
            b2: file.b2,
        })
    }

    /// Predict bounded gains without allocation, lookahead or adaptive mutation.
    ///
    /// Returns `None` for a non-finite or out-of-contract feature. A caller must
    /// retain its classical path when no valid, fresh proposal is available.
    pub fn predict(&self, frame: &FeatureFrame) -> Option<[f32; BANDS]> {
        if frame
            .iter()
            .flatten()
            .any(|x| !x.is_finite() || !(0.0..=1.0).contains(x))
        {
            return None;
        }
        let mut gains = [1.0; BANDS];
        for (out, features) in gains.iter_mut().zip(frame) {
            let mut logit = self.b2;
            for hidden in 0..HIDDEN {
                let mut activation = self.b1[hidden];
                for (&weight, &input) in self.w1[hidden].iter().zip(features) {
                    activation += weight * input;
                }
                logit += self.w2[hidden] * activation.tanh();
            }
            let sigmoid = 1.0 / (1.0 + (-logit.clamp(-20.0, 20.0)).exp());
            *out = GAIN_FLOOR + (1.0 - GAIN_FLOOR) * sigmoid;
        }
        Some(gains)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> serde_json::Value {
        serde_json::json!({"format":"sotf_aec_residual_mask_v1", "feature_version":1,
            "bands":BANDS,"features":FEATURES,"hidden":HIDDEN,"gain_floor":0.25,
            "w1":vec![vec![0.0;FEATURES];HIDDEN], "b1":vec![0.0;HIDDEN],
            "w2":vec![0.0;HIDDEN],"b2":0.0})
    }

    fn load(value: serde_json::Value) -> Result<ResidualMaskModel, String> {
        let bytes = serde_json::to_vec(&value).unwrap();
        let mut hash = String::new();
        for byte in Sha256::digest(&bytes) {
            write!(hash, "{byte:02x}").unwrap();
        }
        ResidualMaskModel::from_bytes(&bytes, &hash)
    }

    #[test]
    fn midpoint_network_matches_scalar_equations_and_rejects_invalid_features() {
        let model = load(fixture()).unwrap();
        let mut features = [[0.5; FEATURES]; BANDS];
        assert_eq!(model.predict(&features), Some([0.625; BANDS]));
        features[0][0] = f32::NAN;
        assert!(model.predict(&features).is_none());
        features[0][0] = 1.01;
        assert!(model.predict(&features).is_none());
    }

    #[test]
    fn malformed_dimensions_contract_and_unbounded_weights_fail_closed() {
        for key in ["hidden", "features", "bands", "feature_version"] {
            let mut value = fixture();
            value[key] = 99.into();
            assert!(load(value).is_err());
        }
        let mut value = fixture();
        value["w1"][0][0] = 101.0.into();
        assert!(load(value).is_err());
        assert!(ResidualMaskModel::from_bytes(b"{}", &"0".repeat(64)).is_err());
        assert!(ResidualMaskModel::from_bytes(&vec![0; MAX_BYTES + 1], "unused").is_err());
    }
}
