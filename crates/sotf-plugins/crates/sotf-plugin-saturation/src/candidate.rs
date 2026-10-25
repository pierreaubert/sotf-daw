//! Explicit capture-bound neural wet-stage selection with native classical fallback.
//!
//! QA-only, mono, zero model lookahead. Models emulate the fixed captured
//! nonlinear wet stage before native DC blocking, output gain and wet/dry mix.
//! No production factory selector or measured-hardware admission is introduced.

// Rust guideline compliant 2026-02-21
use crate::SaturationPlugin;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sotf_plugin_analog_common::hardware_candidate::{
    ExternalFallbackQualification, HardwareCandidate,
};

pub(crate) struct SaturationCandidate {
    pub(crate) stream: ExternalFallbackQualification,
    pub(crate) enabled: bool,
    pub(crate) context_valid: bool,
    controls: Value,
}

fn controls(plugin: &SaturationPlugin) -> Value {
    json!({"mode":plugin.mode as usize,"drive":plugin.drive,"tone":plugin.tone,
        "exciter_freq":plugin.exciter_freq,"dynamic_amount":plugin.dynamic_amount,
        "dynamic_attack_ms":plugin.dynamic_attack_ms,"dynamic_release_ms":plugin.dynamic_release_ms,
        "use_adaa":plugin.use_adaa})
}

/// Hash exact native nonlinear controls for capture-context binding.
///
/// This control-thread helper allocates. Set `sotf_stage` to
/// `saturation_wet_pre_dc` and `sotf_controls_sha256` to this value in the capture's
/// hardware controls. Output gain, mix and DC blocking remain native post-stages.
pub fn capture_controls_sha256(plugin: &SaturationPlugin) -> String {
    let canonical =
        serde_json::to_vec(&controls(plugin)).expect("validated native controls serialize");
    Sha256::digest(&canonical)
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect()
}

impl SaturationCandidate {
    pub(crate) fn new(
        plugin: &SaturationPlugin,
        bytes: Option<&[u8]>,
        sha: &str,
        context: &[u8],
    ) -> Result<Self, String> {
        let captured = controls(plugin);
        let digest = capture_controls_sha256(plugin);
        let candidate = if let Some(bytes) = bytes {
            if context.len() > 1024 * 1024 {
                return Err("capture context exceeds 1MiB".into());
            }
            let expected: Value = serde_json::from_slice(context).map_err(|e| e.to_string())?;
            let setting = &expected["hardware"]["controls"];
            if setting["sotf_stage"] != "saturation_wet_pre_dc"
                || setting["sotf_controls_sha256"] != digest
            {
                return Err(
                    "capture context must bind the actual saturation wet-stage controls".into(),
                );
            }
            Some(HardwareCandidate::from_json(bytes, sha, context).map_err(|e| e.to_string())?)
        } else {
            None
        };
        Ok(Self {
            stream: ExternalFallbackQualification::new(candidate, plugin.sample_rate as u32)
                .map_err(|e| e.to_string())?,
            enabled: false,
            context_valid: true,
            controls: captured,
        })
    }

    pub(crate) fn matches(&self, plugin: &SaturationPlugin) -> bool {
        // Primitive field comparisons below avoid JSON allocation in processing.
        let c = &self.controls;
        c["mode"].as_u64() == Some(plugin.mode as u64)
            && plugin.sample_rate == f64::from(plugin.sample_rate as u32)
            && c["drive"].as_f64() == Some(f64::from(plugin.drive))
            && c["tone"].as_f64() == Some(f64::from(plugin.tone))
            && c["exciter_freq"].as_f64() == Some(f64::from(plugin.exciter_freq))
            && c["dynamic_amount"].as_f64() == Some(f64::from(plugin.dynamic_amount))
            && c["dynamic_attack_ms"].as_f64() == Some(f64::from(plugin.dynamic_attack_ms))
            && c["dynamic_release_ms"].as_f64() == Some(f64::from(plugin.dynamic_release_ms))
            && c["use_adaa"].as_bool() == Some(plugin.use_adaa)
            && plugin.drive_smoother.current() == plugin.drive
    }
}
