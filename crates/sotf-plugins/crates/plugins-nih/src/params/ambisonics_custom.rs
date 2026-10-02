//! Ambisonics custom-geometry carrier for the native NIH wrapper.
//!
//! Target index 8 selects user-authored loudspeaker geometry. The geometry
//! itself travels as DSP custom-layout JSON in the
//! [`AMBISONICS_CUSTOM_STATE_FIELD`] state field; the live NIH scalar
//! parameters keep carrying order, target, weighting, dual-band and
//! algorithm, so the carrier holds geometry only.
//!
//! Staging mirrors the Hiss/EQ restore protocol: [`AmbisonicsCustomRestoreState`]
//! keeps a committed geometry plus a pending restore, an invalid flag,
//! and a fieldless-restore record that keeps target-8 construction from
//! silently resurrecting committed geometry after a fieldless state.
//! Control-thread code validates candidates with [`decode_ambisonics_custom_field`]
//! before mutating anything accepted, publishes staged geometry through an
//! [`AmbisonicsCustomRestoreAttempt`](super::AmbisonicsCustomRestoreAttempt)
//! only after the candidate DSP instance initializes, and the audio callback
//! never touches this module. The DSP decoder revalidates authoritatively at
//! construction; the host DTO validation here is fail-fast agreement.
//!
//! [`AMBISONICS_CUSTOM_STATE_FIELD`]: sotf_host::external_plugin::AMBISONICS_CUSTOM_STATE_FIELD

// Rust guideline compliant 2026-02-21

use sotf_host::external_plugin::AMBISONICS_CUSTOM_STATE_FIELD;
use sotf_host::external_plugin::NativeAmbisonicsCustomGeometry;

/// Transactionally staged custom-geometry restore.
///
/// `committed` is the last geometry a candidate DSP instance accepted.
/// `pending_restore` holds a validated candidate from state until the
/// attempt commits or drops it. `invalid_restore` records a malformed
/// candidate so construction fails instead of silently running named.
/// `missing_field` records a fieldless restore so target-8 construction
/// fails instead of silently resurrecting the committed geometry; named
/// construction ignores the carrier entirely.
#[derive(Debug, Default)]
pub struct AmbisonicsCustomRestoreState {
    /// Last geometry accepted by a constructed candidate.
    pub committed: Option<NativeAmbisonicsCustomGeometry>,
    /// Validated candidate awaiting candidate acceptance.
    pub pending_restore: Option<NativeAmbisonicsCustomGeometry>,
    /// True after a malformed candidate until a valid one stages.
    pub invalid_restore: bool,
    /// True after a fieldless restore until a field stages or commits.
    pub missing_field: bool,
}

/// Parses and validates a custom-geometry state field.
///
/// Accepts exactly the DSP custom-layout JSON shape (`name` plus
/// output-ordered `speakers`); the host DTO shares that shape, so a
/// payload the host validated parses here and vice versa.
///
/// # Errors
///
/// Returns a message when the payload is not custom-layout JSON or
/// violates a name, label, angle or count bound.
pub fn decode_ambisonics_custom_field(
    encoded: &str,
) -> Result<NativeAmbisonicsCustomGeometry, String> {
    let geometry: NativeAmbisonicsCustomGeometry = serde_json::from_str(encoded)
        .map_err(|error| format!("Ambisonics custom geometry is not valid JSON: {error}"))?;
    geometry.validate()?;
    Ok(geometry)
}

/// Serializes committed or staged geometry back into its state field.
///
/// Uses the compact form so saved states match the host rewrite path
/// byte for byte.
///
/// # Errors
///
/// Returns a message when the geometry cannot be serialized.
pub fn encode_ambisonics_custom_field(
    geometry: &NativeAmbisonicsCustomGeometry,
) -> Result<String, String> {
    serde_json::to_string(geometry)
        .map_err(|error| format!("Ambisonics custom geometry is not serializable: {error}"))
}

/// State-field key for custom geometry, shared with the host.
pub fn ambisonics_custom_state_field() -> &'static str {
    AMBISONICS_CUSTOM_STATE_FIELD
}

/// `target_layout` choice index selecting custom geometry.
///
/// Appended after the eight named indices 0 through 7. Pinned against
/// the host constant by `target_index_matches_host_schema`.
pub const AMBISONICS_CUSTOM_TARGET_INDEX: usize = 8;

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo_json() -> String {
        serde_json::json!({
            "name": "stereo",
            "speakers": [
                {"label": "FL", "azimuth_deg": 30.0, "elevation_deg": 0.0, "is_lfe": false},
                {"label": "FR", "azimuth_deg": -30.0, "elevation_deg": 0.0, "is_lfe": false}
            ]
        })
        .to_string()
    }

    #[test]
    fn decode_accepts_dsp_layout_json() {
        let geometry = decode_ambisonics_custom_field(&stereo_json()).unwrap();
        assert_eq!(geometry.name, "stereo");
        assert_eq!(geometry.total_channels(), 2);
    }

    #[test]
    fn decode_rejects_malformed_and_invalid_payloads() {
        assert!(decode_ambisonics_custom_field("not json").is_err());
        assert!(decode_ambisonics_custom_field("{\"name\": \"empty\", \"speakers\": []}").is_err());
        let duplicate = serde_json::json!({
            "name": "dup",
            "speakers": [
                {"label": "C", "azimuth_deg": 0.0, "elevation_deg": 0.0, "is_lfe": false},
                {"label": "C", "azimuth_deg": 90.0, "elevation_deg": 0.0, "is_lfe": false}
            ]
        })
        .to_string();
        assert!(decode_ambisonics_custom_field(&duplicate).is_err());
    }

    #[test]
    fn encode_roundtrips_decode() {
        let geometry = decode_ambisonics_custom_field(&stereo_json()).unwrap();
        let encoded = encode_ambisonics_custom_field(&geometry).unwrap();
        assert_eq!(decode_ambisonics_custom_field(&encoded).unwrap(), geometry);
    }

    #[test]
    fn field_key_matches_host_schema() {
        assert_eq!(ambisonics_custom_state_field(), "sotf_ambisonics_custom");
    }

    #[test]
    fn target_index_matches_host_schema() {
        assert_eq!(
            sotf_host::external_plugin::AMBISONICS_CUSTOM_TARGET_CHOICE_INDEX,
            AMBISONICS_CUSTOM_TARGET_INDEX as i32
        );
    }

    #[test]
    fn speaker_ceiling_matches_decoder_bound() {
        assert_eq!(
            sotf_host::external_plugin::MAX_AMBISONICS_CUSTOM_SPEAKERS,
            64
        );
    }
}
