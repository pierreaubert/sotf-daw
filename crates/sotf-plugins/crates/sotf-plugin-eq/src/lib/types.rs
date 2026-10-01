use super::misc::default_order;
use math_audio_iir_fir::BiquadCoefficients;
use serde::{Deserialize, Serialize};
use sotf_host::auto_gain::AutoGainParams;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EqFilterTopology {
    #[default]
    Biquad,
    WarpedBiquad,
    KautzFilter,
}

/// Per-band stereo-field placement for the ordered EQ route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EqBandPlacement {
    Stereo,
    Left,
    Right,
    Mid,
    Side,
}

impl EqBandPlacement {
    pub(super) fn requires_stereo_pair(self) -> bool {
        matches!(self, Self::Left | Self::Right | Self::Mid | Self::Side)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KautzSectionConfig {
    #[serde(alias = "freq", alias = "frequency", alias = "pole_freq_hz")]
    pub pole_freq: f64,
    pub q: f64,
    /// Linear modal output weight, not decibels.
    #[serde(default)]
    pub gain: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BiquadFilterConfig {
    pub filter_type: String,
    pub freq: f64,
    pub q: f64,
    /// Gain in dB for Biquad and WarpedBiquad. For the legacy Kautz fallback
    /// (empty `kautz_sections`), this serialized field is used as a linear
    /// modal weight. Its historical name and value are retained for preset
    /// compatibility; it is not converted from dB on that path.
    #[serde(default)]
    pub db_gain: f64,
    /// Filter order: 2 (default, single biquad), 4, 6, or 8.
    /// Higher orders cascade N/2 biquads with Butterworth Q staggering.
    #[serde(default = "default_order")]
    pub order: usize,
    /// Runtime filter topology. Defaults to standard biquad for backwards-compatible
    /// Roomeq/host JSON.
    #[serde(default)]
    pub topology: EqFilterTopology,
    /// Optional per-band stereo-field placement. Absence retains the exact legacy route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<EqBandPlacement>,
    /// Warping coefficient for `topology=warped_biquad`. If omitted, the plugin
    /// uses the Bark-scale lambda for the active sample rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lambda: Option<f64>,
    /// Kautz sections for `topology=kautz_filter`; each `gain` is a linear
    /// modal weight. If omitted, the filter's `freq`, `q`, and legacy `db_gain`
    /// value define one section, with `db_gain` retained as-is as its weight.
    #[serde(default, alias = "sections", skip_serializing_if = "Vec::is_empty")]
    pub kautz_sections: Vec<KautzSectionConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EqPluginParams {
    #[serde(default)]
    pub filters: Vec<BiquadFilterConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_filters: Option<Vec<Vec<BiquadFilterConfig>>>,
    /// Explicit disjoint channels for Left/Right/Mid/Side bands on multichannel input.
    /// Two-channel input uses [0, 1] when this value is absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stereo_pairs: Option<Vec<[usize; 2]>>,
    #[serde(default)]
    pub auto_gain: AutoGainParams,
}

/// Per-band coefficient transition state for parameter smoothing.
/// Stores per-stage old and new coefficients so all biquad stages transition
/// smoothly, not just the first one (fixes glitch for order > 2 bands).
pub(super) struct BandTransition {
    /// Old coefficients for each stage (len = num_stages for this band).
    pub(super) old_coeffs_per_channel: Vec<Vec<BiquadCoefficients>>,
    /// New coefficients for each stage (len = num_stages for this band).
    pub(super) new_coeffs_per_channel: Vec<Vec<BiquadCoefficients>>,
    pub(super) samples_remaining: usize,
    pub(super) total_samples: usize,
}
