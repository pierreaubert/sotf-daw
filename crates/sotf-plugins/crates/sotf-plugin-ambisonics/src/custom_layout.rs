//! User-defined loudspeaker layouts for the Ambisonics decoder.
//!
//! Named SOTF layouts are `&'static` tables owned by `sotf-host` and cannot
//! represent user-authored geometry. This module provides the owned,
//! serializable counterpart: [`CustomSpeaker`] positions plus a [`CustomLayout`]
//! container with construction-time validation. Decoder matrices for custom
//! geometry are built with [`DecodeMatrix::build_for_custom`] and
//! [`DecodeMatrix::build_allrad_for_custom`], which reuse the same
//! regularized SVD, max-rE and VBAP helpers as the named path.
//!
//! [`DecodeMatrix::build_for_custom`]: crate::decode_matrix::DecodeMatrix::build_for_custom
//! [`DecodeMatrix::build_allrad_for_custom`]: crate::decode_matrix::DecodeMatrix::build_allrad_for_custom

// Rust guideline compliant 2026-02-21

use crate::params::Params;
use serde::{Deserialize, Serialize};

/// Maximum speakers in a custom layout.
///
/// 64 matches the order-7 HOA input ceiling so dense experimental arrays can
/// be decoded at the DSP level. Engine output admission stays at 16 channels
/// (`MAX_OUTPUT_CHANNELS`); wider custom layouts need the shared engine patch
/// before they can run end to end.
pub const MAX_CUSTOM_SPEAKERS: usize = 64;

/// Maximum length of a custom layout name in characters.
pub const MAX_CUSTOM_NAME_LEN: usize = 64;

/// Maximum length of a custom speaker label in characters.
pub const MAX_CUSTOM_LABEL_LEN: usize = 16;

/// Selection key for user geometry in `Params::target_layout`.
///
/// Appended after the named SOTF layouts so existing choice indices are
/// unchanged. `Params` itself carries no geometry; selecting this key
/// requires a [`CustomDecoderConfig`] (which bundles the params with its
/// mandatory `custom_layout`) passed to
/// `AmbisonicsDecoderPlugin::new_custom()`. The named-only `new()`
/// constructor rejects this key transactionally.
pub const CUSTOM_LAYOUT_KEY: &str = "custom";

/// One user-authored loudspeaker position.
///
/// Channel index is the position inside [`CustomLayout::speakers`]; there is
/// no separate channel field to keep validation single-sourced. Angles use the
/// SOTF convention: azimuth 0 is front, +90 is left; elevation 0 is ear
/// level, +90 is overhead. LFE entries carry no direction; their angles are
/// validated for finiteness but ignored by every decode builder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomSpeaker {
    /// Short unique label (for example `"FL"` or `"TBL"`).
    pub label: String,
    /// Horizontal angle in degrees, -180 to +180.
    pub azimuth_deg: f32,
    /// Vertical angle in degrees, -90 to +90.
    pub elevation_deg: f32,
    /// True for the low-frequency channel, which is always decoded silent.
    pub is_lfe: bool,
}

impl CustomSpeaker {
    /// Converts spherical position to a unit Cartesian vector.
    ///
    /// Returns `[x, y, z]` with `x` lateral, `y` toward front and `z` up,
    /// matching `SpeakerPosition::to_cartesian` exactly.
    ///
    /// # Examples
    ///
    /// ```
    /// use sotf_plugin_ambisonics::custom_layout::CustomSpeaker;
    ///
    /// let front = CustomSpeaker {
    ///     label: "C".to_owned(),
    ///     azimuth_deg: 0.0,
    ///     elevation_deg: 0.0,
    ///     is_lfe: false,
    /// };
    /// let vector = front.to_cartesian();
    /// assert!((vector[1] - 1.0).abs() < 1e-6);
    /// ```
    pub fn to_cartesian(&self) -> [f32; 3] {
        let azimuth = self.azimuth_deg.to_radians();
        let elevation = self.elevation_deg.to_radians();
        let cos_elevation = elevation.cos();
        [
            cos_elevation * azimuth.sin(),
            cos_elevation * azimuth.cos(),
            elevation.sin(),
        ]
    }
}

/// Owned user-defined loudspeaker layout.
///
/// Serializable with serde so custom geometry persists through factory,
/// engine and preset save/reload. Always validated with [`CustomLayout::validate`]
/// before decoder construction; builders reject invalid geometry instead of
/// clamping it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomLayout {
    /// User-visible layout name.
    pub name: String,
    /// Speaker entries in output-channel order.
    pub speakers: Vec<CustomSpeaker>,
}

impl CustomLayout {
    /// Validates every name, label, angle and count bound.
    ///
    /// Requires a non-empty name, 1 to [`MAX_CUSTOM_SPEAKERS`] speakers, at
    /// least one non-LFE speaker, unique non-empty labels, finite angles with
    /// azimuth in [-180, 180] and elevation in [-90, 90]. Degenerate but
    /// well-formed geometry (collinear or duplicate directions) passes here;
    /// the decode builders report its rank loss or reject unbounded gain.
    ///
    /// # Errors
    ///
    /// Returns a message naming the first violated bound.
    ///
    /// # Examples
    ///
    /// ```
    /// use sotf_plugin_ambisonics::custom_layout::{CustomLayout, CustomSpeaker};
    ///
    /// let layout = CustomLayout {
    ///     name: "mono".to_owned(),
    ///     speakers: vec![CustomSpeaker {
    ///         label: "C".to_owned(),
    ///         azimuth_deg: 0.0,
    ///         elevation_deg: 0.0,
    ///         is_lfe: false,
    ///     }],
    /// };
    /// assert!(layout.validate().is_ok());
    /// ```
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("Custom layout name must not be empty".to_owned());
        }
        if self.name.chars().count() > MAX_CUSTOM_NAME_LEN {
            return Err(format!(
                "Custom layout name exceeds {MAX_CUSTOM_NAME_LEN} characters"
            ));
        }
        if self.speakers.is_empty() {
            return Err("Custom layout must contain at least one speaker".to_owned());
        }
        if self.speakers.len() > MAX_CUSTOM_SPEAKERS {
            return Err(format!(
                "Custom layout has {} speakers, at most {MAX_CUSTOM_SPEAKERS} are supported",
                self.speakers.len()
            ));
        }
        for (index, speaker) in self.speakers.iter().enumerate() {
            if speaker.label.is_empty() {
                return Err(format!("Custom speaker {index} has an empty label"));
            }
            if speaker.label.chars().count() > MAX_CUSTOM_LABEL_LEN {
                return Err(format!(
                    "Custom speaker label '{}' exceeds {MAX_CUSTOM_LABEL_LEN} characters",
                    speaker.label
                ));
            }
            if !speaker.azimuth_deg.is_finite() || !speaker.elevation_deg.is_finite() {
                return Err(format!(
                    "Custom speaker '{}' has a non-finite angle",
                    speaker.label
                ));
            }
            if !(-180.0..=180.0).contains(&speaker.azimuth_deg) {
                return Err(format!(
                    "Custom speaker '{}' azimuth {} is outside [-180, 180]",
                    speaker.label, speaker.azimuth_deg
                ));
            }
            if !(-90.0..=90.0).contains(&speaker.elevation_deg) {
                return Err(format!(
                    "Custom speaker '{}' elevation {} is outside [-90, 90]",
                    speaker.label, speaker.elevation_deg
                ));
            }
        }
        let mut labels: Vec<&str> = self
            .speakers
            .iter()
            .map(|speaker| speaker.label.as_str())
            .collect();
        labels.sort_unstable();
        for pair in labels.windows(2) {
            if pair[0] == pair[1] {
                return Err(format!(
                    "Custom speaker label '{}' is used more than once",
                    pair[0]
                ));
            }
        }
        if self.speakers.iter().all(|speaker| speaker.is_lfe) {
            return Err("Custom layout must contain at least one non-LFE speaker".to_owned());
        }
        Ok(())
    }

    /// Counts output channels (one per speaker entry).
    pub fn total_channels(&self) -> usize {
        self.speakers.len()
    }

    /// Counts speakers that take part in the directional solve.
    pub fn non_lfe_count(&self) -> usize {
        self.speakers
            .iter()
            .filter(|speaker| !speaker.is_lfe)
            .count()
    }

    /// Serializes this layout to pretty JSON for export and persistence.
    ///
    /// # Errors
    ///
    /// Returns a message if serialization fails.
    pub fn export_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self)
            .map_err(|error| format!("Failed to export custom layout: {error}"))
    }

    /// Parses and validates a layout previously written by `export_json`.
    ///
    /// # Errors
    ///
    /// Returns a message if parsing fails or the parsed layout is invalid.
    pub fn import_json(json: &str) -> Result<Self, String> {
        let layout: Self = serde_json::from_str(json)
            .map_err(|error| format!("Failed to parse custom layout: {error}"))?;
        layout.validate()?;
        Ok(layout)
    }
}

/// Serializable custom-decoder configuration (R1 persistence).
///
/// Carries the standard [`Params`] fields inline plus the required user
/// geometry. `Params` itself is unchanged so every existing literal
/// construction, preset and snapshot keeps compiling; hosts select this shape
/// when `target_layout` is `"custom"`. Accepted by
/// `AmbisonicsDecoderPlugin::new_custom`.
///
/// Save contract: hosts persist a custom decoder by retaining the exact
/// JSON value they constructed (this struct serialized), and rebuild with
/// `new_custom()` on reload. A running instance can also reconstruct this
/// value through `AmbisonicsDecoderPlugin::custom_config()` for typed save
/// paths. `Plugin::get_data` is intentionally not part of the contract: the
/// host reserves `Some` there for analyzer payloads, so the decoder always
/// returns `None`.
///
/// # Examples
///
/// ```
/// use sotf_plugin_ambisonics::custom_layout::CustomDecoderConfig;
///
/// let config: CustomDecoderConfig = serde_json::from_value(serde_json::json!({
///     "order": 1,
///     "target_layout": "custom",
///     "custom_layout": {
///         "name": "stereo",
///         "speakers": [
///             {"label": "FL", "azimuth_deg": 30.0, "elevation_deg": 0.0, "is_lfe": false},
///             {"label": "FR", "azimuth_deg": -30.0, "elevation_deg": 0.0, "is_lfe": false}
///         ]
///     }
/// }))
/// .unwrap();
/// assert_eq!(config.custom_layout.total_channels(), 2);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomDecoderConfig {
    /// Standard decoder parameters; `target_layout` must be `"custom"`.
    #[serde(flatten)]
    pub params: Params,
    /// Required user-defined loudspeaker geometry.
    pub custom_layout: CustomLayout,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn speaker(label: &str, azimuth_deg: f32, elevation_deg: f32, is_lfe: bool) -> CustomSpeaker {
        CustomSpeaker {
            label: label.to_owned(),
            azimuth_deg,
            elevation_deg,
            is_lfe,
        }
    }

    fn stereo() -> CustomLayout {
        CustomLayout {
            name: "stereo".to_owned(),
            speakers: vec![
                speaker("FL", 30.0, 0.0, false),
                speaker("FR", -30.0, 0.0, false),
            ],
        }
    }

    #[test]
    fn valid_layout_passes() {
        assert!(stereo().validate().is_ok());
    }

    #[test]
    fn empty_name_is_rejected() {
        let mut layout = stereo();
        layout.name.clear();
        assert!(layout.validate().is_err());
    }

    #[test]
    fn empty_speakers_are_rejected() {
        let layout = CustomLayout {
            name: "empty".to_owned(),
            speakers: Vec::new(),
        };
        assert!(layout.validate().is_err());
    }

    #[test]
    fn all_lfe_is_rejected() {
        let layout = CustomLayout {
            name: "lfe-only".to_owned(),
            speakers: vec![speaker("LFE", 0.0, 0.0, true)],
        };
        assert!(layout.validate().is_err());
    }

    #[test]
    fn out_of_range_angles_are_rejected() {
        for (azimuth, elevation) in [
            (181.0, 0.0),
            (-181.0, 0.0),
            (0.0, 91.0),
            (0.0, -91.0),
            (f32::NAN, 0.0),
            (0.0, f32::INFINITY),
        ] {
            let layout = CustomLayout {
                name: "bad-angle".to_owned(),
                speakers: vec![speaker("C", azimuth, elevation, false)],
            };
            assert!(
                layout.validate().is_err(),
                "azimuth={azimuth}, elevation={elevation}"
            );
        }
    }

    #[test]
    fn duplicate_labels_are_rejected() {
        let layout = CustomLayout {
            name: "dup".to_owned(),
            speakers: vec![
                speaker("C", 0.0, 0.0, false),
                speaker("C", 90.0, 0.0, false),
            ],
        };
        assert!(layout.validate().is_err());
    }

    #[test]
    fn too_many_speakers_are_rejected() {
        let layout = CustomLayout {
            name: "huge".to_owned(),
            speakers: (0..=MAX_CUSTOM_SPEAKERS)
                .map(|index| speaker(&format!("S{index}"), 0.0, 0.0, false))
                .collect(),
        };
        assert_eq!(layout.speakers.len(), MAX_CUSTOM_SPEAKERS + 1);
        assert!(layout.validate().is_err());
    }

    #[test]
    fn export_import_roundtrip_preserves_geometry() {
        let layout = stereo();
        let json = layout.export_json().unwrap();
        assert_eq!(CustomLayout::import_json(&json).unwrap(), layout);
    }

    #[test]
    fn import_rejects_malformed_and_invalid_payloads() {
        assert!(CustomLayout::import_json("{not json").is_err());
        let mut layout = stereo();
        layout.speakers[0].azimuth_deg = 999.0;
        let json = serde_json::to_string(&layout).unwrap();
        assert!(CustomLayout::import_json(&json).is_err());
    }

    #[test]
    fn custom_decoder_config_roundtrip_serde() {
        let config = CustomDecoderConfig {
            params: Params {
                target_layout: CUSTOM_LAYOUT_KEY.to_owned(),
                ..Params::default()
            },
            custom_layout: stereo(),
        };
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["target_layout"], "custom");
        assert_eq!(json["custom_layout"]["name"], "stereo");
        let restored: CustomDecoderConfig = serde_json::from_value(json).unwrap();
        assert_eq!(restored, config);
    }

    #[test]
    fn cartesian_matches_sotf_convention() {
        let cases = [
            (speaker("C", 0.0, 0.0, false), [0.0, 1.0, 0.0]),
            (speaker("L", 90.0, 0.0, false), [1.0, 0.0, 0.0]),
            (speaker("T", 0.0, 90.0, false), [0.0, 0.0, 1.0]),
        ];
        for (speaker, expected) in cases {
            let vector = speaker.to_cartesian();
            for (actual, expected) in vector.iter().zip(expected.iter()) {
                assert!((actual - expected).abs() < 1e-6);
            }
        }
    }
}
