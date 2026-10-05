use super::native_crossover_layout::NativeCrossoverStructure;
pub use super::native_crossover_layout::{
    NativeCrossoverInputLayout, NativeCrossoverMode, NativeCrossoverOutputLayout,
    NativeCrossoverTopology,
};
use super::plugin_descriptor::PluginDescriptor;
use super::plugin_format::PluginFormat;
use super::types::ExternalPluginSandboxMode;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const AMBISONICS_CLAP_PLUGIN_ID: &str = "org.spinorama.sotf.ambisonics";
const AMBISONICS_VST3_CLASS_ID: &str = "536F7466416D6269736E696330303031";
const BAND_SPLIT_CLAP_PLUGIN_ID: &str = "org.spinorama.sotf.band-split";
const BAND_SPLIT_VST3_CLASS_ID: &str = "536F746642616E6453706C7430303031";
const CROSSOVER_CLAP_PLUGIN_ID: &str = "org.spinorama.sotf.crossover";
const CROSSOVER_VST3_CLASS_ID: &str = "536F746643726F73736F766572303031";
const DEESSER_CLAP_PLUGIN_ID: &str = "org.spinorama.sotf.de-esser";
const DEESSER_VST3_CLASS_ID: &str = "536F7466446545737365723030303031";
const GATE_CLAP_PLUGIN_ID: &str = "org.spinorama.sotf.gate";
const GATE_VST3_CLASS_ID: &str = "536F7466476174653030303030303031";

/// Stable placeholder schema for saving/restoring external plugin state.
///
/// Native CLAP/VST3/AU loaders can later fill `opaque_state` with the format's
/// binary state blob. Until then, descriptor and sandbox metadata still round-trip
/// through presets/projects without pretending the native state was loaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalPluginState {
    pub schema_version: u32,
    pub descriptor: PluginDescriptor,
    pub format: PluginFormat,
    pub plugin_id: String,
    pub plugin_path: PathBuf,
    pub sandbox_mode: ExternalPluginSandboxMode,
    pub opaque_state: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_setup: Option<NativePluginAudioSetup>,
}

/// Describes an explicit per-instance native audio layout request.
///
/// This state is separate from [`PluginDescriptor`], whose channel counts
/// describe the scanned plugin rather than one selected instance layout.
/// The public enum exposes its variant fields (qualifiers are illegal on
/// enum variants) so external hosts and integration tests can construct
/// setups directly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativePluginAudioSetup {
    /// Configures the SOTF Ambisonics decoder with an ACN/SN3D input order.
    Ambisonics {
        /// Ambisonics order, from one through seven.
        order: u8,
        /// Named speaker target selected for the decoder output.
        target_layout: NativeAmbisonicsTargetLayout,
    },
    /// Configures the decoder with user-authored custom speaker geometry.
    ///
    /// The geometry travels beside the opaque native state on purpose:
    /// restore validates that the typed geometry and the geometry embedded
    /// in the native state agree before a replacement instance can be
    /// committed. Structural readback reports choice index 8 for this
    /// variant; named indices 0 through 7 are unchanged.
    AmbisonicsCustom {
        /// Ambisonics order, from one through seven.
        order: u8,
        /// Validated custom speaker geometry for the decoder output.
        custom: NativeAmbisonicsCustomGeometry,
    },
    /// Selects an indexed multi-band output route for the SOTF BandSplit plugin.
    BandSplit {
        /// Number of stereo bands produced by the processor, from two through four.
        num_bands: u8,
        /// Format-specific output-bus representation.
        output_layout: NativeBandSplitOutputLayout,
    },
    /// Selects an explicit named input layout and native Crossover route.
    ///
    /// Mode/topology/count are duplicated with the opaque native state on
    /// purpose: restore validates that the typed route and native structural
    /// readback agree before a replacement instance can be committed.
    Crossover {
        input_layout: NativeCrossoverInputLayout,
        num_bands: u8,
        topology: NativeCrossoverTopology,
        mode: NativeCrossoverMode,
        output_layout: NativeCrossoverOutputLayout,
    },
    /// Routes a main program bus plus an independent detector key bus.
    ///
    /// The packed instance input carries program channels first, then
    /// key channels; outputs carry program only. CLAP selects the
    /// two-input-port configuration and VST3 activates the auxiliary
    /// input bus. Bus-count changes require recreating the instance.
    Sidechain {
        /// Program bus width, one or two.
        main_channels: u8,
        /// Key bus width, one or two.
        key_channels: u8,
    },
}

/// Native output-bus representation for the recognized BandSplit plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeBandSplitOutputLayout {
    /// One CLAP main port carries band-major stereo pairs.
    ClapPacked,
    /// Four fixed VST3 stereo buses carry bands one through four.
    Vst3Buses,
    /// The historical VST3 four-channel main bus plus optional later-band buses.
    Vst3LegacyPacked,
}

/// Names an existing SOTF Ambisonics decoder speaker target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeAmbisonicsTargetLayout {
    /// Legacy default target with the existing 5.1 identity channel order.
    FiveOne,
    /// 7.1 with separate side and rear speaker pairs.
    SevenOne,
    /// 5.1 with two front-height speakers.
    FiveOneTwo,
    /// 5.1 with front and rear height pairs.
    FiveOneFour,
    /// 7.1 with a front-height pair.
    SevenOneTwo,
    /// 7.1 with front and rear height pairs.
    SevenOneFour,
    /// 9.1.4 with left and right wide speakers.
    NineOneFour,
    /// 9.1.6 with wide and top-middle speakers.
    NineOneSixWide,
}

impl NativeAmbisonicsTargetLayout {
    pub(crate) const fn plugin_parameter_choice_index(self) -> i32 {
        match self {
            Self::FiveOne => 0,
            Self::SevenOne => 1,
            Self::FiveOneTwo => 2,
            Self::FiveOneFour => 3,
            Self::SevenOneTwo => 4,
            Self::SevenOneFour => 5,
            Self::NineOneFour => 6,
            Self::NineOneSixWide => 7,
        }
    }

    pub(crate) const fn output_channels(self) -> usize {
        match self {
            Self::FiveOne => 6,
            Self::SevenOne | Self::FiveOneTwo => 8,
            Self::FiveOneFour | Self::SevenOneTwo => 10,
            Self::SevenOneFour => 12,
            Self::NineOneFour => 14,
            Self::NineOneSixWide => 16,
        }
    }

    pub(crate) const fn clap_configuration_target(self) -> Option<u32> {
        match self {
            Self::FiveOne => Some(0),
            Self::SevenOne => Some(1),
            Self::FiveOneTwo => Some(2),
            Self::FiveOneFour => Some(3),
            Self::SevenOneTwo => Some(4),
            Self::SevenOneFour => Some(5),
            Self::NineOneFour | Self::NineOneSixWide => None,
        }
    }

    // Ungated: the feature-independent custom-geometry methods compare
    // against this table, so it must exist in every configuration.
    pub(crate) const fn clap_channel_map(self) -> Option<&'static [u8]> {
        match self {
            Self::FiveOne => Some(&[0, 1, 2, 3, 9, 10]),
            Self::SevenOne => Some(&[0, 1, 2, 3, 9, 10, 4, 5]),
            Self::FiveOneTwo => Some(&[0, 1, 2, 3, 9, 10, 12, 14]),
            Self::FiveOneFour => Some(&[0, 1, 2, 3, 9, 10, 12, 14, 15, 17]),
            Self::SevenOneTwo => Some(&[0, 1, 2, 3, 9, 10, 4, 5, 12, 14]),
            Self::SevenOneFour => Some(&[0, 1, 2, 3, 9, 10, 4, 5, 12, 14, 15, 17]),
            Self::NineOneFour | Self::NineOneSixWide => None,
        }
    }

    // Ungated: the feature-independent custom-geometry methods compare
    // against this table, so it must exist in every configuration.
    pub(crate) const fn vst3_speaker_arrangement(self) -> u64 {
        match self {
            Self::FiveOne => 0x0000_0000_0000_003f,
            Self::SevenOne => 0x0000_0000_0000_063f,
            Self::FiveOneTwo => 0x0000_0000_0000_503f,
            Self::FiveOneFour => 0x0000_0000_0002_d03f,
            Self::SevenOneTwo => 0x0000_0000_0000_563f,
            Self::SevenOneFour => 0x0000_0000_0002_d63f,
            Self::NineOneFour => 0x1800_0000_0002_d63f,
            Self::NineOneSixWide => 0x1800_0000_0302_d63f,
        }
    }

    #[cfg(feature = "external-plugin-vst3")]
    pub(crate) const fn vst3_bus_to_sotf_permutation(self) -> &'static [usize] {
        match self {
            Self::FiveOne => &[0, 1, 2, 3, 4, 5],
            Self::SevenOne => &[0, 1, 2, 3, 6, 7, 4, 5],
            Self::FiveOneTwo => &[0, 1, 2, 3, 4, 5, 6, 7],
            Self::FiveOneFour => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
            Self::SevenOneTwo => &[0, 1, 2, 3, 6, 7, 4, 5, 8, 9],
            Self::SevenOneFour => &[0, 1, 2, 3, 6, 7, 4, 5, 8, 9, 10, 11],
            Self::NineOneFour => &[0, 1, 2, 3, 6, 7, 4, 5, 10, 11, 12, 13, 8, 9],
            Self::NineOneSixWide => &[0, 1, 2, 3, 6, 7, 4, 5, 10, 11, 12, 13, 14, 15, 8, 9],
        }
    }
}

/// Maximum custom speakers mirrored from the decoder bound.
///
/// `sotf-host` cannot depend on the Ambisonics DSP crate without a
/// dependency cycle, so this mirrors DSP `MAX_CUSTOM_SPEAKERS`. The
/// decoder revalidates every bound at construction; this copy only
/// produces early host-side errors with clear reasons. Shared with
/// the NIH plugin crate so both sides size one permutation.
pub const MAX_AMBISONICS_CUSTOM_SPEAKERS: usize = 64;

/// Maximum custom layout name length mirrored from the decoder.
const MAX_AMBISONICS_CUSTOM_NAME_LEN: usize = 64;

/// Maximum custom speaker label length mirrored from the decoder.
const MAX_AMBISONICS_CUSTOM_LABEL_LEN: usize = 16;

/// Native `target_layout` choice index selecting custom geometry.
///
/// Appended after the eight named indices 0 through 7; matches the DSP
/// `target_layout` choice table without depending on the decoder crate.
/// Shared with the NIH plugin crate so both sides name one index.
pub const AMBISONICS_CUSTOM_TARGET_CHOICE_INDEX: i32 = 8;

/// One user-authored loudspeaker in a native custom setup.
///
/// Mirrors the DSP custom-speaker JSON shape field-for-field with
/// primitives only, so the host never depends on the decoder crate.
/// Angles use the SOTF convention: azimuth 0 is front, +90 is left;
/// elevation 0 is ear level, +90 is overhead.
///
/// # Examples
///
/// ```
/// use sotf_host::external_plugin::NativeAmbisonicsCustomGeometry;
///
/// let geometry: NativeAmbisonicsCustomGeometry =
///     serde_json::from_value(serde_json::json!({
///         "name": "stereo",
///         "speakers": [
///             {"label": "FL", "azimuth_deg": 30.0, "elevation_deg": 0.0, "is_lfe": false},
///             {"label": "FR", "azimuth_deg": -30.0, "elevation_deg": 0.0, "is_lfe": false}
///         ]
///     }))
///     .unwrap();
/// assert!(geometry.validate().is_ok());
/// assert_eq!(geometry.total_channels(), 2);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeAmbisonicsCustomSpeaker {
    /// Short unique label (for example `"FL"`).
    pub label: String,
    /// Horizontal angle in degrees, -180 to +180.
    pub azimuth_deg: f32,
    /// Vertical angle in degrees, -90 to +90.
    pub elevation_deg: f32,
    /// True for the low-frequency channel, always decoded silent.
    pub is_lfe: bool,
}

// Manual `Eq`: [`NativeAmbisonicsCustomGeometry::validate`] rejects
// non-finite angles, so equality is reflexive for every geometry the host
// accepts. `Eq` carries no unsafe implications; an unvalidated NaN angle
// would be a caller logic error, never undefined behavior.
impl Eq for NativeAmbisonicsCustomSpeaker {}

/// Owned user-defined loudspeaker layout for native activation.
///
/// Serializable counterpart of the DSP custom layout with the exact same
/// JSON shape. Always validated before use; the decoder revalidates at
/// construction. See [`NativeAmbisonicsCustomSpeaker`] for an example.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeAmbisonicsCustomGeometry {
    /// User-visible layout name.
    pub name: String,
    /// Speaker entries in output-channel order.
    pub speakers: Vec<NativeAmbisonicsCustomSpeaker>,
}

impl Eq for NativeAmbisonicsCustomGeometry {}

/// Standard speaker role recognized for native custom geometry.
///
/// Every role maps to an exact VST3 speaker bit (derived from the eight
/// advertised native masks) and, except wides, to an exact CLAP surround
/// role (derived from the six advertised native channel maps). Speakers
/// that match no role fail closed with a clear reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CustomStandardRole {
    FrontLeft,
    FrontRight,
    FrontCenter,
    Lfe,
    SurroundLeft,
    SurroundRight,
    SideLeft,
    SideRight,
    BackLeft,
    BackRight,
    WideLeft,
    WideRight,
    TopFrontLeft,
    TopFrontRight,
    TopBackLeft,
    TopBackRight,
    TopMidLeft,
    TopMidRight,
}

impl CustomStandardRole {
    /// VST3 speaker bit for this role, matching the pinned SDK `kSpeaker*` constants.
    const fn vst3_bit(self) -> u32 {
        match self {
            Self::FrontLeft => 0,
            Self::FrontRight => 1,
            Self::FrontCenter => 2,
            Self::Lfe => 3,
            Self::SurroundLeft | Self::BackLeft => 4,
            Self::SurroundRight | Self::BackRight => 5,
            Self::SideLeft => 9,
            Self::SideRight => 10,
            Self::TopFrontLeft => 12,
            Self::TopFrontRight => 14,
            Self::TopBackLeft => 15,
            Self::TopBackRight => 17,
            Self::TopMidLeft => 24,
            Self::TopMidRight => 25,
            Self::WideLeft => 59,
            Self::WideRight => 60,
        }
    }

    /// CLAP surround role id, or `None` when CLAP defines none.
    ///
    /// Only wide speakers lack a standard CLAP surround role; top-middle
    /// speakers use the top-side roles 18 and 19.
    const fn clap_role(self) -> Option<u8> {
        match self {
            Self::FrontLeft => Some(0),
            Self::FrontRight => Some(1),
            Self::FrontCenter => Some(2),
            Self::Lfe => Some(3),
            Self::BackLeft => Some(4),
            Self::BackRight => Some(5),
            Self::SurroundLeft | Self::SideLeft => Some(9),
            Self::SurroundRight | Self::SideRight => Some(10),
            Self::TopFrontLeft => Some(12),
            Self::TopFrontRight => Some(14),
            Self::TopBackLeft => Some(15),
            Self::TopBackRight => Some(17),
            Self::TopMidLeft => Some(18),
            Self::TopMidRight => Some(19),
            Self::WideLeft | Self::WideRight => None,
        }
    }
}

/// Which ear-level surround pairs a custom layout contains.
///
/// The VST3 and CLAP surround assignments are contextual: a lone
/// ±110-degree pair takes the Ls/Rs bits, while layouts with separate
/// ±90-degree sides and ±150-degree backs take Sl/Sr plus Ls/Rs. Any
/// other surround combination fails closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CustomSurroundTopology {
    pair_110: bool,
    pair_90: bool,
    pair_150: bool,
}

/// Classifies the ear-level surround pairs of a custom layout.
fn custom_surround_topology(speakers: &[NativeAmbisonicsCustomSpeaker]) -> CustomSurroundTopology {
    let has = |azimuth: f32| {
        speakers.iter().any(|speaker| {
            !speaker.is_lfe && speaker.elevation_deg == 0.0 && speaker.azimuth_deg == azimuth
        })
    };
    CustomSurroundTopology {
        pair_110: has(110.0) && has(-110.0),
        pair_90: has(90.0) && has(-90.0),
        pair_150: has(150.0) && has(-150.0),
    }
}

/// Maps one custom speaker to its standard role, if exactly matched.
///
/// Matching uses exact angle equality; near-misses fail closed. LFE is
/// matched by flag (its angles carry no direction); surrounds use the
/// layout topology from [`custom_surround_topology`].
fn custom_standard_role(
    speaker: &NativeAmbisonicsCustomSpeaker,
    topology: CustomSurroundTopology,
) -> Option<CustomStandardRole> {
    use CustomStandardRole::*;
    if speaker.is_lfe {
        return Some(Lfe);
    }
    let (azimuth, elevation) = (speaker.azimuth_deg, speaker.elevation_deg);
    if azimuth == 30.0 && elevation == 0.0 {
        return Some(FrontLeft);
    }
    if azimuth == -30.0 && elevation == 0.0 {
        return Some(FrontRight);
    }
    if azimuth == 0.0 && elevation == 0.0 {
        return Some(FrontCenter);
    }
    if azimuth == 60.0 && elevation == 0.0 {
        return Some(WideLeft);
    }
    if azimuth == -60.0 && elevation == 0.0 {
        return Some(WideRight);
    }
    if azimuth == 30.0 && elevation == 45.0 {
        return Some(TopFrontLeft);
    }
    if azimuth == -30.0 && elevation == 45.0 {
        return Some(TopFrontRight);
    }
    if azimuth == 150.0 && elevation == 45.0 {
        return Some(TopBackLeft);
    }
    if azimuth == -150.0 && elevation == 45.0 {
        return Some(TopBackRight);
    }
    if azimuth == 90.0 && elevation == 45.0 {
        return Some(TopMidLeft);
    }
    if azimuth == -90.0 && elevation == 45.0 {
        return Some(TopMidRight);
    }
    if elevation != 0.0 {
        return None;
    }
    let single_110 = topology.pair_110 && !topology.pair_90 && !topology.pair_150;
    let split_sides = !topology.pair_110 && topology.pair_90 && topology.pair_150;
    if (azimuth == 110.0 || azimuth == -110.0) && single_110 {
        return Some(if azimuth > 0.0 {
            SurroundLeft
        } else {
            SurroundRight
        });
    }
    if (azimuth == 90.0 || azimuth == -90.0) && split_sides {
        return Some(if azimuth > 0.0 { SideLeft } else { SideRight });
    }
    if (azimuth == 150.0 || azimuth == -150.0) && split_sides {
        return Some(if azimuth > 0.0 { BackLeft } else { BackRight });
    }
    None
}

impl NativeAmbisonicsCustomGeometry {
    /// Validates every name, label, angle and count bound.
    ///
    /// Mirrors the DSP `CustomLayout::validate` checks and messages so
    /// host-side and decoder-side failures name the same violated bound.
    /// Degenerate but well-formed geometry passes here; the decode
    /// builders report rank loss or reject unbounded gain at
    /// construction.
    ///
    /// # Errors
    ///
    /// Returns a message naming the first violated bound.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("Custom layout name must not be empty".to_owned());
        }
        if self.name.chars().count() > MAX_AMBISONICS_CUSTOM_NAME_LEN {
            return Err(format!(
                "Custom layout name exceeds {MAX_AMBISONICS_CUSTOM_NAME_LEN} characters"
            ));
        }
        if self.speakers.is_empty() {
            return Err("Custom layout must contain at least one speaker".to_owned());
        }
        if self.speakers.len() > MAX_AMBISONICS_CUSTOM_SPEAKERS {
            return Err(format!(
                "Custom layout has {} speakers, at most {MAX_AMBISONICS_CUSTOM_SPEAKERS} are supported",
                self.speakers.len()
            ));
        }
        for (index, speaker) in self.speakers.iter().enumerate() {
            if speaker.label.is_empty() {
                return Err(format!("Custom speaker {index} has an empty label"));
            }
            if speaker.label.chars().count() > MAX_AMBISONICS_CUSTOM_LABEL_LEN {
                return Err(format!(
                    "Custom speaker label '{}' exceeds {MAX_AMBISONICS_CUSTOM_LABEL_LEN} characters",
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

    /// Computes the VST3 speaker arrangement and bus permutation.
    ///
    /// Every speaker must match a standard role exactly, each role bit is
    /// used at most once, and the resulting mask must equal one of the
    /// eight natively advertised Ambisonics arrangements (the NIH wrapper
    /// only accepts those static masks). The permutation maps ascending
    /// VST3 bus positions to SOTF output channels. Role bits follow the
    /// pinned VST3 SDK speaker definitions (`kSpeakerSl` is bit 9 and
    /// `kSpeakerSr` is bit 10), so every advertised mask composes
    /// naturally and customs load exactly where the named target loads.
    ///
    /// # Errors
    ///
    /// Returns a message for unsupported orders, invalid geometry,
    /// unmappable or duplicated roles, or a non-advertised mask.
    pub fn vst3_arrangement(&self, order: u8) -> Result<(u64, Vec<usize>), String> {
        if !(1..=7).contains(&order) {
            return Err(format!(
                "Ambisonics order {order} is unsupported; expected an order from 1 through 7"
            ));
        }
        self.validate()?;
        let topology = custom_surround_topology(&self.speakers);
        let mut roles: Vec<(u32, usize)> = Vec::with_capacity(self.speakers.len());
        for (index, speaker) in self.speakers.iter().enumerate() {
            let role = custom_standard_role(speaker, topology).ok_or_else(|| {
                format!(
                    "Custom speaker {index} ('{}') at azimuth {} elevation {} matches no standard VST3 speaker",
                    speaker.label, speaker.azimuth_deg, speaker.elevation_deg
                )
            })?;
            roles.push((role.vst3_bit(), index));
        }
        roles.sort_by_key(|role| role.0);
        for pair in roles.windows(2) {
            if pair[0].0 == pair[1].0 {
                let (first, second) = (pair[0].1, pair[1].1);
                return Err(format!(
                    "Custom speakers {first} ('{}') and {second} ('{}') share one VST3 speaker role",
                    self.speakers[first].label, self.speakers[second].label
                ));
            }
        }
        let mut mask = 0u64;
        let mut permutation = Vec::with_capacity(roles.len());
        for &(bit, sotf) in &roles {
            mask |= 1u64 << bit;
            permutation.push(sotf);
        }
        let advertised = [
            NativeAmbisonicsTargetLayout::FiveOne,
            NativeAmbisonicsTargetLayout::SevenOne,
            NativeAmbisonicsTargetLayout::FiveOneTwo,
            NativeAmbisonicsTargetLayout::FiveOneFour,
            NativeAmbisonicsTargetLayout::SevenOneTwo,
            NativeAmbisonicsTargetLayout::SevenOneFour,
            NativeAmbisonicsTargetLayout::NineOneFour,
            NativeAmbisonicsTargetLayout::NineOneSixWide,
        ]
        .iter()
        .any(|target| target.vst3_speaker_arrangement() == mask);
        if !advertised {
            return Err(format!(
                "Custom VST3 speaker arrangement {mask:#018x} is not one of the natively advertised Ambisonics arrangements"
            ));
        }
        Ok((mask, permutation))
    }

    /// Maps every channel to its standard CLAP surround role, in order.
    ///
    /// # Errors
    ///
    /// Returns a message for invalid geometry, wide speakers (CLAP
    /// defines no standard wide role) or other unmappable speakers.
    pub fn clap_role_map(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let topology = custom_surround_topology(&self.speakers);
        let mut map = Vec::with_capacity(self.speakers.len());
        for (index, speaker) in self.speakers.iter().enumerate() {
            let role = custom_standard_role(speaker, topology).ok_or_else(|| {
                format!(
                    "Custom speaker {index} ('{}') at azimuth {} elevation {} matches no standard CLAP surround role",
                    speaker.label, speaker.azimuth_deg, speaker.elevation_deg
                )
            })?;
            let role_id = role.clap_role().ok_or_else(|| {
                format!(
                    "Custom speaker {index} ('{}') is a wide speaker; CLAP surround has no standard wide role",
                    speaker.label
                )
            })?;
            map.push(role_id);
        }
        Ok(map)
    }

    /// Finds the advertised CLAP configuration exactly matching this geometry.
    ///
    /// A custom layout activates on CLAP only when its channel count and
    /// ordered surround roles equal one advertised static configuration;
    /// the advertised map is then truthful by construction. LFE anywhere
    /// but channel 3 and wide speakers get explicit reasons.
    ///
    /// # Errors
    ///
    /// Returns a message for unsupported orders, invalid geometry,
    /// unoffered widths, misplaced LFE, unmappable roles, or no exact
    /// configuration match.
    pub fn matching_clap_configuration(
        &self,
        order: u8,
    ) -> Result<NativeAmbisonicsTargetLayout, String> {
        if !(1..=7).contains(&order) {
            return Err(format!(
                "Ambisonics order {order} is unsupported; expected an order from 1 through 7"
            ));
        }
        self.validate()?;
        // Report an unoffered width before narrower role checks: no
        // advertised configuration can claim a width it does not offer,
        // so channel roles are only meaningful for supported widths.
        let outputs = self.speakers.len();
        if ![6, 8, 10, 12].contains(&outputs) {
            return Err(format!(
                "Custom geometry has {outputs} output channels; advertised CLAP surround configurations offer 6, 8, 10 or 12"
            ));
        }
        for (index, speaker) in self.speakers.iter().enumerate() {
            if speaker.is_lfe && index != 3 {
                return Err(format!(
                    "Custom LFE speaker '{}' is at channel {index}; advertised CLAP surround configurations place LFE at channel 3",
                    speaker.label
                ));
            }
        }
        let roles = self.clap_role_map()?;
        for target in [
            NativeAmbisonicsTargetLayout::FiveOne,
            NativeAmbisonicsTargetLayout::SevenOne,
            NativeAmbisonicsTargetLayout::FiveOneTwo,
            NativeAmbisonicsTargetLayout::FiveOneFour,
            NativeAmbisonicsTargetLayout::SevenOneTwo,
            NativeAmbisonicsTargetLayout::SevenOneFour,
        ] {
            if target.output_channels() == outputs
                && target.clap_channel_map() == Some(roles.as_slice())
            {
                return Ok(target);
            }
        }
        Err(format!(
            "Custom geometry with {outputs} channels matches no advertised CLAP surround configuration"
        ))
    }
}

impl NativePluginAudioSetup {
    /// Returns the negotiated full-vector input and speaker output widths.
    ///
    /// # Errors
    /// Returns an error when the order is outside the supported range.
    pub fn channel_counts(&self) -> Result<(usize, usize), String> {
        match self {
            Self::Ambisonics {
                order,
                target_layout,
            } if (1..=7).contains(order) => {
                let input_channels = (usize::from(*order) + 1).pow(2);
                Ok((input_channels, target_layout.output_channels()))
            }
            Self::Ambisonics { order, .. } => Err(format!(
                "Ambisonics order {order} is unsupported; expected an order from 1 through 7"
            )),
            Self::AmbisonicsCustom { order, custom }
                if (1..=7).contains(order)
                    && (1..=MAX_AMBISONICS_CUSTOM_SPEAKERS).contains(&custom.speakers.len()) =>
            {
                let input_channels = (usize::from(*order) + 1).pow(2);
                Ok((input_channels, custom.speakers.len()))
            }
            Self::AmbisonicsCustom { order, custom } => Err(if !(1..=7).contains(order) {
                format!(
                    "Ambisonics order {order} is unsupported; expected an order from 1 through 7"
                )
            } else {
                format!(
                    "Custom Ambisonics geometry has {} speakers; expected 1 through {MAX_AMBISONICS_CUSTOM_SPEAKERS}",
                    custom.speakers.len()
                )
            }),
            Self::BandSplit { num_bands, .. } if (2..=4).contains(num_bands) => {
                Ok((2, usize::from(*num_bands) * 2))
            }
            Self::BandSplit { num_bands, .. } => Err(format!(
                "BandSplit band count {num_bands} is unsupported; expected two through four"
            )),
            Self::Crossover {
                input_layout,
                num_bands,
                topology,
                mode,
                ..
            } if (2..=4).contains(num_bands) => {
                let input_channels = input_layout.channel_count();
                let output_channels = NativeCrossoverStructure {
                    mode: *mode,
                    topology: *topology,
                    num_bands: *num_bands,
                }
                .output_channels(input_channels)
                .ok_or_else(|| "Crossover output channel count overflowed".to_string())?;
                if output_channels > 64 {
                    return Err(format!(
                        "Crossover route requires {output_channels} output channels; maximum is 64"
                    ));
                }
                Ok((input_channels, output_channels))
            }
            Self::Crossover { num_bands, .. } => Err(format!(
                "Crossover band count {num_bands} is unsupported; expected two through four"
            )),
            Self::Sidechain {
                main_channels,
                key_channels,
            } if (1..=2).contains(main_channels) && (1..=2).contains(key_channels) => Ok((
                usize::from(*main_channels) + usize::from(*key_channels),
                usize::from(*main_channels),
            )),
            Self::Sidechain {
                main_channels,
                key_channels,
            } => Err(format!(
                "Sidechain route {main_channels}+{key_channels} is unsupported; expected one or two program channels and one or two key channels"
            )),
        }
    }

    /// Checks that this setup belongs to the exact supported native plugin.
    ///
    /// # Errors
    /// Returns an error for unsupported identities, formats, orders or targets.
    pub fn validate_for_descriptor(&self, descriptor: &PluginDescriptor) -> Result<(), String> {
        self.channel_counts()?;
        match (descriptor.format, descriptor.id.as_str(), self) {
            (
                PluginFormat::Clap,
                AMBISONICS_CLAP_PLUGIN_ID,
                Self::Ambisonics { target_layout, .. },
            ) if target_layout.clap_configuration_target().is_some() => Ok(()),
            (PluginFormat::Clap, AMBISONICS_CLAP_PLUGIN_ID, Self::Ambisonics { .. }) => Err(
                "CLAP standard surround does not represent the selected wide speaker target".into(),
            ),
            (PluginFormat::Vst3, id, Self::Ambisonics { .. })
                if id.eq_ignore_ascii_case(AMBISONICS_VST3_CLASS_ID) =>
            {
                Ok(())
            }
            (
                PluginFormat::Clap,
                AMBISONICS_CLAP_PLUGIN_ID,
                Self::AmbisonicsCustom { order, custom },
            ) => {
                custom.matching_clap_configuration(*order)?;
                Ok(())
            }
            (PluginFormat::Vst3, id, Self::AmbisonicsCustom { order, custom })
                if id.eq_ignore_ascii_case(AMBISONICS_VST3_CLASS_ID) =>
            {
                custom.vst3_arrangement(*order)?;
                Ok(())
            }
            (
                PluginFormat::Clap,
                BAND_SPLIT_CLAP_PLUGIN_ID,
                Self::BandSplit {
                    output_layout: NativeBandSplitOutputLayout::ClapPacked,
                    ..
                },
            ) => Ok(()),
            (
                PluginFormat::Vst3,
                id,
                Self::BandSplit {
                    output_layout:
                        NativeBandSplitOutputLayout::Vst3Buses
                        | NativeBandSplitOutputLayout::Vst3LegacyPacked,
                    ..
                },
            ) if id.eq_ignore_ascii_case(BAND_SPLIT_VST3_CLASS_ID) => Ok(()),
            (
                PluginFormat::Clap,
                CROSSOVER_CLAP_PLUGIN_ID,
                Self::Crossover {
                    output_layout: NativeCrossoverOutputLayout::ClapPacked,
                    ..
                },
            ) => Ok(()),
            (
                PluginFormat::Vst3,
                id,
                Self::Crossover {
                    output_layout: NativeCrossoverOutputLayout::Vst3Buses,
                    ..
                },
            ) if id.eq_ignore_ascii_case(CROSSOVER_VST3_CLASS_ID) => Ok(()),
            (PluginFormat::Clap, DEESSER_CLAP_PLUGIN_ID, Self::Sidechain { .. }) => Ok(()),
            (PluginFormat::Clap, GATE_CLAP_PLUGIN_ID, Self::Sidechain { .. }) => Ok(()),
            (PluginFormat::Vst3, id, Self::Sidechain { .. })
                if id.eq_ignore_ascii_case(DEESSER_VST3_CLASS_ID)
                    || id.eq_ignore_ascii_case(GATE_VST3_CLASS_ID) =>
            {
                Ok(())
            }
            _ => Err(format!(
                "native Ambisonics setup is not supported for {} plugin identity '{}'",
                match descriptor.format {
                    PluginFormat::Clap => "CLAP",
                    PluginFormat::Vst3 => "VST3",
                    PluginFormat::AudioUnit => "Audio Unit",
                },
                descriptor.id
            )),
        }
    }

    pub(crate) fn for_descriptor_or_legacy_default(
        descriptor: &PluginDescriptor,
        setup: Option<&Self>,
    ) -> Result<Option<Self>, String> {
        if let Some(setup) = setup {
            setup.validate_for_descriptor(descriptor)?;
            return Ok(Some(setup.clone()));
        }
        let is_ambisonics = match descriptor.format {
            PluginFormat::Clap => descriptor.id == AMBISONICS_CLAP_PLUGIN_ID,
            PluginFormat::Vst3 => descriptor.id.eq_ignore_ascii_case(AMBISONICS_VST3_CLASS_ID),
            PluginFormat::AudioUnit => false,
        };
        if is_ambisonics {
            return Ok(Some(Self::Ambisonics {
                order: 1,
                target_layout: NativeAmbisonicsTargetLayout::FiveOne,
            }));
        }
        let is_band_split = match descriptor.format {
            PluginFormat::Clap => descriptor.id == BAND_SPLIT_CLAP_PLUGIN_ID,
            PluginFormat::Vst3 => descriptor.id.eq_ignore_ascii_case(BAND_SPLIT_VST3_CLASS_ID),
            PluginFormat::AudioUnit => false,
        };
        Ok(is_band_split.then_some(Self::BandSplit {
            num_bands: 2,
            output_layout: match descriptor.format {
                PluginFormat::Clap => NativeBandSplitOutputLayout::ClapPacked,
                PluginFormat::Vst3 => NativeBandSplitOutputLayout::Vst3Buses,
                PluginFormat::AudioUnit => unreachable!("Audio Unit BandSplit is unsupported"),
            },
        }))
    }

    pub(crate) fn legacy_band_split_default(descriptor: &PluginDescriptor) -> Option<Self> {
        (descriptor.format == PluginFormat::Vst3
            && descriptor.id.eq_ignore_ascii_case(BAND_SPLIT_VST3_CLASS_ID))
        .then_some(Self::BandSplit {
            num_bands: 2,
            output_layout: NativeBandSplitOutputLayout::Vst3LegacyPacked,
        })
    }
}

impl ExternalPluginState {
    pub const SCHEMA_VERSION: u32 = 1;

    /// Returns the active audio widths for this plugin instance.
    ///
    /// A typed native setup describes the negotiated instance route and takes
    /// precedence over scanned descriptor widths. Without one, the descriptor
    /// remains the compatibility source. Invalid typed setups are reported so
    /// route planners cannot silently treat them as the scanned layout.
    pub fn effective_audio_channel_counts(&self) -> Result<(usize, usize), String> {
        let Some(setup) = self.audio_setup.as_ref() else {
            return Ok((self.descriptor.audio_inputs, self.descriptor.audio_outputs));
        };

        self.validate_descriptor_consistency()?;
        setup.validate_for_descriptor(&self.descriptor)?;
        setup.channel_counts()
    }

    pub fn new(
        descriptor: PluginDescriptor,
        sandbox_mode: ExternalPluginSandboxMode,
        opaque_state: Vec<u8>,
    ) -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            format: descriptor.format,
            plugin_id: descriptor.id.clone(),
            plugin_path: descriptor.path.clone(),
            descriptor,
            sandbox_mode,
            opaque_state,
            audio_setup: None,
        }
    }

    pub fn validate_descriptor_consistency(&self) -> Result<(), String> {
        if self.schema_version != Self::SCHEMA_VERSION {
            return Err(format!(
                "Unsupported external plugin state schema version {}",
                self.schema_version
            ));
        }
        if self.format != self.descriptor.format
            || self.plugin_id != self.descriptor.id
            || self.plugin_path != self.descriptor.path
        {
            return Err("External plugin state descriptor fields are inconsistent".to_string());
        }
        Ok(())
    }

    /// Validate both the stable state envelope and its concrete plugin
    /// descriptor before the state is inserted into a host graph.
    pub fn validate(&self) -> Result<(), String> {
        self.validate_descriptor_consistency()?;
        self.descriptor.validate()?;
        if let Some(audio_setup) = self.audio_setup.as_ref() {
            audio_setup.validate_for_descriptor(&self.descriptor)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod ambisonics_custom_setup_tests {
    use super::{
        AMBISONICS_CUSTOM_TARGET_CHOICE_INDEX, ExternalPluginState, NativeAmbisonicsCustomGeometry,
        NativeAmbisonicsCustomSpeaker, NativeAmbisonicsTargetLayout, NativePluginAudioSetup,
    };
    use crate::external_plugin::plugin_descriptor::PluginDescriptor;
    use crate::external_plugin::plugin_format::PluginFormat;
    use crate::external_plugin::types::{ExternalPluginSandboxMode, PluginScanStatus};
    use crate::speaker_config::{
        CONFIG_5_1, CONFIG_5_1_2, CONFIG_5_1_4, CONFIG_7_1, CONFIG_7_1_2, CONFIG_7_1_4,
        CONFIG_9_1_6, SpeakerConfig,
    };
    // Used only by the VST3-gated static-table reproduction test below.
    #[cfg(feature = "external-plugin-vst3")]
    use crate::speaker_config::CONFIG_9_1_4;
    use std::path::PathBuf;

    fn speaker(
        label: &str,
        azimuth_deg: f32,
        elevation_deg: f32,
        is_lfe: bool,
    ) -> NativeAmbisonicsCustomSpeaker {
        NativeAmbisonicsCustomSpeaker {
            label: label.to_owned(),
            azimuth_deg,
            elevation_deg,
            is_lfe,
        }
    }

    fn geometry(
        name: &str,
        speakers: Vec<NativeAmbisonicsCustomSpeaker>,
    ) -> NativeAmbisonicsCustomGeometry {
        NativeAmbisonicsCustomGeometry {
            name: name.to_owned(),
            speakers,
        }
    }

    fn geometry_from_const(config: &SpeakerConfig) -> NativeAmbisonicsCustomGeometry {
        geometry(
            config.name,
            config
                .speakers
                .iter()
                .map(|position| {
                    speaker(
                        position.label,
                        position.azimuth,
                        position.elevation,
                        position.is_lfe,
                    )
                })
                .collect(),
        )
    }

    fn stereo_geometry() -> NativeAmbisonicsCustomGeometry {
        geometry(
            "stereo",
            vec![
                speaker("FL", 30.0, 0.0, false),
                speaker("FR", -30.0, 0.0, false),
            ],
        )
    }

    fn custom_setup(order: u8, custom: NativeAmbisonicsCustomGeometry) -> NativePluginAudioSetup {
        NativePluginAudioSetup::AmbisonicsCustom { order, custom }
    }

    fn descriptor(format: PluginFormat, id: &str) -> PluginDescriptor {
        PluginDescriptor {
            id: id.into(),
            name: "SOTF: Ambisonics Decoder".into(),
            vendor: "SOTF".into(),
            version: "test".into(),
            format,
            path: PathBuf::from("/test/ambisonics"),
            audio_inputs: 4,
            audio_outputs: 6,
            is_instrument: false,
            categories: Vec::new(),
            scan_status: PluginScanStatus::Loadable,
        }
    }

    fn clap_descriptor() -> PluginDescriptor {
        descriptor(PluginFormat::Clap, "org.spinorama.sotf.ambisonics")
    }

    fn vst3_descriptor() -> PluginDescriptor {
        descriptor(PluginFormat::Vst3, "536F7466416D6269736E696330303031")
    }

    #[test]
    fn custom_choice_index_is_appended_after_named() {
        use NativeAmbisonicsTargetLayout::{
            FiveOne, FiveOneFour, FiveOneTwo, NineOneFour, NineOneSixWide, SevenOne, SevenOneFour,
            SevenOneTwo,
        };
        assert_eq!(AMBISONICS_CUSTOM_TARGET_CHOICE_INDEX, 8);
        assert_eq!(FiveOne.plugin_parameter_choice_index(), 0);
        assert_eq!(SevenOne.plugin_parameter_choice_index(), 1);
        assert_eq!(FiveOneTwo.plugin_parameter_choice_index(), 2);
        assert_eq!(FiveOneFour.plugin_parameter_choice_index(), 3);
        assert_eq!(SevenOneTwo.plugin_parameter_choice_index(), 4);
        assert_eq!(SevenOneFour.plugin_parameter_choice_index(), 5);
        assert_eq!(NineOneFour.plugin_parameter_choice_index(), 6);
        assert_eq!(NineOneSixWide.plugin_parameter_choice_index(), 7);
    }

    #[test]
    fn custom_geometry_json_matches_dsp_layout_shape() {
        let encoded = serde_json::to_string(&stereo_geometry()).unwrap();
        assert_eq!(
            encoded,
            r#"{"name":"stereo","speakers":[{"label":"FL","azimuth_deg":30.0,"elevation_deg":0.0,"is_lfe":false},{"label":"FR","azimuth_deg":-30.0,"elevation_deg":0.0,"is_lfe":false}]}"#
        );
        let decoded: NativeAmbisonicsCustomGeometry = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, stereo_geometry());
    }

    #[test]
    fn custom_setup_serde_roundtrips_and_legacy_named_parses() {
        let setup = custom_setup(7, geometry_from_const(&CONFIG_7_1_4));
        let encoded = serde_json::to_vec(&setup).unwrap();
        let decoded: NativePluginAudioSetup = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, setup);
        let legacy: NativePluginAudioSetup = serde_json::from_value(serde_json::json!({
            "type": "ambisonics",
            "order": 7,
            "target_layout": "seven_one_four"
        }))
        .unwrap();
        assert!(matches!(
            legacy,
            NativePluginAudioSetup::Ambisonics {
                order: 7,
                target_layout: NativeAmbisonicsTargetLayout::SevenOneFour
            }
        ));
        let state = ExternalPluginState {
            schema_version: ExternalPluginState::SCHEMA_VERSION,
            descriptor: clap_descriptor(),
            format: PluginFormat::Clap,
            plugin_id: "org.spinorama.sotf.ambisonics".into(),
            plugin_path: PathBuf::from("/test/ambisonics"),
            sandbox_mode: ExternalPluginSandboxMode::InProcess,
            opaque_state: vec![1, 2, 3],
            audio_setup: Some(setup),
        };
        let encoded = serde_json::to_vec(&state).unwrap();
        let decoded: ExternalPluginState = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, state);
    }

    #[test]
    fn custom_geometry_validation_mirrors_decoder_bounds() {
        assert!(stereo_geometry().validate().is_ok());
        let empty_name = geometry("", stereo_geometry().speakers);
        assert_eq!(
            empty_name.validate().unwrap_err(),
            "Custom layout name must not be empty"
        );
        let long_name = geometry(&"n".repeat(65), stereo_geometry().speakers);
        assert_eq!(
            long_name.validate().unwrap_err(),
            "Custom layout name exceeds 64 characters"
        );
        assert_eq!(
            geometry("empty", Vec::new()).validate().unwrap_err(),
            "Custom layout must contain at least one speaker"
        );
        let many: Vec<_> = (0..65)
            .map(|index| speaker(&format!("S{index}"), 30.0, 0.0, false))
            .collect();
        assert_eq!(
            geometry("many", many).validate().unwrap_err(),
            "Custom layout has 65 speakers, at most 64 are supported"
        );
        let all_lfe = geometry(
            "lfe",
            vec![
                speaker("LFE", 0.0, 0.0, true),
                speaker("LFE2", 0.0, 0.0, true),
            ],
        );
        assert_eq!(
            all_lfe.validate().unwrap_err(),
            "Custom layout must contain at least one non-LFE speaker"
        );
        let empty_label = geometry("labels", vec![speaker("", 30.0, 0.0, false)]);
        assert_eq!(
            empty_label.validate().unwrap_err(),
            "Custom speaker 0 has an empty label"
        );
        let long_label = geometry("labels", vec![speaker(&"l".repeat(17), 30.0, 0.0, false)]);
        assert_eq!(
            long_label.validate().unwrap_err(),
            "Custom speaker label 'lllllllllllllllll' exceeds 16 characters"
        );
        let duplicate = geometry(
            "labels",
            vec![
                speaker("FL", 30.0, 0.0, false),
                speaker("FL", -30.0, 0.0, false),
            ],
        );
        assert_eq!(
            duplicate.validate().unwrap_err(),
            "Custom speaker label 'FL' is used more than once"
        );
        let non_finite = geometry("angles", vec![speaker("FL", f32::NAN, 0.0, false)]);
        assert_eq!(
            non_finite.validate().unwrap_err(),
            "Custom speaker 'FL' has a non-finite angle"
        );
        let bad_azimuth = geometry("angles", vec![speaker("FL", 181.0, 0.0, false)]);
        assert_eq!(
            bad_azimuth.validate().unwrap_err(),
            "Custom speaker 'FL' azimuth 181 is outside [-180, 180]"
        );
        let bad_elevation = geometry("angles", vec![speaker("FL", 30.0, -91.0, false)]);
        assert_eq!(
            bad_elevation.validate().unwrap_err(),
            "Custom speaker 'FL' elevation -91 is outside [-90, 90]"
        );
    }

    // Compares against the VST3-only static permutation table.
    #[cfg(feature = "external-plugin-vst3")]
    #[test]
    fn custom_vst3_arrangement_reproduces_all_named_layouts() {
        use NativeAmbisonicsTargetLayout::{
            FiveOne, FiveOneFour, FiveOneTwo, NineOneFour, NineOneSixWide, SevenOne, SevenOneFour,
            SevenOneTwo,
        };
        for (config, target) in [
            (&CONFIG_5_1, FiveOne),
            (&CONFIG_7_1, SevenOne),
            (&CONFIG_5_1_2, FiveOneTwo),
            (&CONFIG_5_1_4, FiveOneFour),
            (&CONFIG_7_1_2, SevenOneTwo),
            (&CONFIG_7_1_4, SevenOneFour),
            (&CONFIG_9_1_4, NineOneFour),
            (&CONFIG_9_1_6, NineOneSixWide),
        ] {
            let custom = geometry_from_const(config);
            assert!(custom.validate().is_ok(), "{}", config.id);
            let (mask, permutation) = custom.vst3_arrangement(7).unwrap();
            assert_eq!(mask, target.vst3_speaker_arrangement(), "{}", config.id);
            assert_eq!(
                permutation.as_slice(),
                target.vst3_bus_to_sotf_permutation(),
                "{}",
                config.id
            );
        }
    }

    #[test]
    fn custom_vst3_arrangement_matches_advertised_7_1_2_mask() {
        // Sides are VST3 bits 9/10 (`kSpeakerSl`/`kSpeakerSr`), so the
        // 7.1.2 role set composes the advertised mask naturally with no
        // remapping; see `vst3_arrangement`.
        let custom = geometry_from_const(&CONFIG_7_1_2);
        let (mask, permutation) = custom.vst3_arrangement(7).unwrap();
        assert_eq!(mask, 0x563f);
        assert_eq!(
            mask,
            NativeAmbisonicsTargetLayout::SevenOneTwo.vst3_speaker_arrangement()
        );
        assert_eq!(permutation, vec![0, 1, 2, 3, 6, 7, 4, 5, 8, 9]);
    }

    #[test]
    fn custom_clap_role_map_reproduces_all_advertised_maps() {
        use NativeAmbisonicsTargetLayout::{
            FiveOne, FiveOneFour, FiveOneTwo, SevenOne, SevenOneFour, SevenOneTwo,
        };
        for (config, target) in [
            (&CONFIG_5_1, FiveOne),
            (&CONFIG_7_1, SevenOne),
            (&CONFIG_5_1_2, FiveOneTwo),
            (&CONFIG_5_1_4, FiveOneFour),
            (&CONFIG_7_1_2, SevenOneTwo),
            (&CONFIG_7_1_4, SevenOneFour),
        ] {
            let custom = geometry_from_const(config);
            let roles = custom.clap_role_map().unwrap();
            assert_eq!(
                roles.as_slice(),
                target.clap_channel_map().unwrap(),
                "{}",
                config.id
            );
            assert_eq!(
                custom.matching_clap_configuration(7).unwrap(),
                target,
                "{}",
                config.id
            );
        }
    }

    #[test]
    fn custom_clap_configuration_rejects_with_explicit_reasons() {
        let mut moved_lfe = geometry_from_const(&CONFIG_7_1_4);
        moved_lfe.speakers.swap(3, 5);
        let error = moved_lfe.matching_clap_configuration(7).unwrap_err();
        assert!(
            error.contains("place LFE at channel 3"),
            "unexpected: {error}"
        );
        let wide = geometry(
            "wide-six",
            vec![
                speaker("FL", 30.0, 0.0, false),
                speaker("FR", -30.0, 0.0, false),
                speaker("C", 0.0, 0.0, false),
                speaker("LFE", 0.0, 0.0, true),
                speaker("WL", 60.0, 0.0, false),
                speaker("WR", -60.0, 0.0, false),
            ],
        );
        let error = wide.matching_clap_configuration(7).unwrap_err();
        assert!(
            error.contains("no standard wide role"),
            "unexpected: {error}"
        );
        let sixteen = geometry_from_const(&CONFIG_9_1_6);
        let error = sixteen.matching_clap_configuration(7).unwrap_err();
        assert!(
            error.contains("offer 6, 8, 10 or 12"),
            "unexpected: {error}"
        );
        let mut shuffled = geometry_from_const(&CONFIG_7_1_4);
        shuffled.speakers.swap(4, 5);
        let error = shuffled.matching_clap_configuration(7).unwrap_err();
        assert!(
            error.contains("matches no advertised CLAP surround configuration"),
            "unexpected: {error}"
        );
        let error = stereo_geometry()
            .matching_clap_configuration(8)
            .unwrap_err();
        assert_eq!(
            error,
            "Ambisonics order 8 is unsupported; expected an order from 1 through 7"
        );
    }

    // Compares against the VST3-only static permutation table.
    #[cfg(feature = "external-plugin-vst3")]
    #[test]
    fn custom_vst3_arrangement_accepts_moved_lfe_with_computed_permutation() {
        let mut moved_lfe = geometry_from_const(&CONFIG_9_1_6);
        moved_lfe.speakers.swap(3, 5);
        let (mask, permutation) = moved_lfe.vst3_arrangement(7).unwrap();
        assert_eq!(
            mask,
            NativeAmbisonicsTargetLayout::NineOneSixWide.vst3_speaker_arrangement()
        );
        assert_eq!(
            permutation,
            vec![0, 1, 2, 5, 6, 7, 4, 3, 10, 11, 12, 13, 14, 15, 8, 9]
        );
        assert_ne!(
            permutation,
            NativeAmbisonicsTargetLayout::NineOneSixWide.vst3_bus_to_sotf_permutation()
        );
    }

    #[test]
    fn custom_vst3_arrangement_rejects_nonstandard_and_duplicate_roles() {
        let nonstandard = geometry(
            "nonstandard",
            vec![
                speaker("FL", 30.0, 0.0, false),
                speaker("ODD", 45.0, 0.0, false),
            ],
        );
        let error = nonstandard.vst3_arrangement(7).unwrap_err();
        assert!(
            error.contains("matches no standard VST3 speaker"),
            "unexpected: {error}"
        );
        let stereo_mask = stereo_geometry().vst3_arrangement(7).unwrap_err();
        assert!(
            stereo_mask.contains("is not one of the natively advertised"),
            "unexpected: {stereo_mask}"
        );
        assert!(
            stereo_mask.contains("0x0000000000000003"),
            "unexpected: {stereo_mask}"
        );
        let duplicate = geometry(
            "duplicate",
            vec![
                speaker("C", 0.0, 0.0, false),
                speaker("C2", 0.0, 0.0, false),
            ],
        );
        let error = duplicate.vst3_arrangement(7).unwrap_err();
        assert!(
            error.contains("share one VST3 speaker role"),
            "unexpected: {error}"
        );
        let multi_lfe = geometry(
            "multi-lfe",
            vec![
                speaker("C", 0.0, 0.0, false),
                speaker("LFE", 0.0, 0.0, true),
                speaker("LFE2", 0.0, 0.0, true),
            ],
        );
        let error = multi_lfe.vst3_arrangement(7).unwrap_err();
        assert!(
            error.contains("share one VST3 speaker role"),
            "unexpected: {error}"
        );
    }

    #[test]
    fn custom_channel_counts_derive_widths_from_order_and_speakers() {
        let sixteen = custom_setup(7, geometry_from_const(&CONFIG_9_1_6));
        assert_eq!(sixteen.channel_counts().unwrap(), (64, 16));
        let six = custom_setup(1, geometry_from_const(&CONFIG_5_1));
        assert_eq!(six.channel_counts().unwrap(), (4, 6));
        let bad_order = custom_setup(8, stereo_geometry());
        assert_eq!(
            bad_order.channel_counts().unwrap_err(),
            "Ambisonics order 8 is unsupported; expected an order from 1 through 7"
        );
        let empty = custom_setup(7, geometry("empty", Vec::new()));
        assert_eq!(
            empty.channel_counts().unwrap_err(),
            "Custom Ambisonics geometry has 0 speakers; expected 1 through 64"
        );
    }

    #[test]
    fn custom_setup_validates_per_format_with_clear_reasons() {
        let exact = custom_setup(7, geometry_from_const(&CONFIG_7_1_4));
        assert!(exact.validate_for_descriptor(&clap_descriptor()).is_ok());
        let sixteen = custom_setup(7, geometry_from_const(&CONFIG_9_1_6));
        assert!(sixteen.validate_for_descriptor(&clap_descriptor()).is_err());
        let other_id = descriptor(PluginFormat::Clap, "org.spinorama.sotf.band-split");
        let error = exact.validate_for_descriptor(&other_id).unwrap_err();
        assert!(error.contains("is not supported"), "unexpected: {error}");
        let mut moved_lfe = geometry_from_const(&CONFIG_9_1_6);
        moved_lfe.speakers.swap(3, 5);
        let vst3_custom = custom_setup(7, moved_lfe);
        assert!(
            vst3_custom
                .validate_for_descriptor(&vst3_descriptor())
                .is_ok()
        );
        let stereo = custom_setup(7, stereo_geometry());
        assert!(
            stereo
                .validate_for_descriptor(&vst3_descriptor())
                .unwrap_err()
                .contains("is not one of the natively advertised")
        );
        let audio_unit = descriptor(PluginFormat::AudioUnit, "org.spinorama.sotf.ambisonics");
        assert!(exact.validate_for_descriptor(&audio_unit).is_err());
        let named_wide = NativePluginAudioSetup::Ambisonics {
            order: 7,
            target_layout: NativeAmbisonicsTargetLayout::NineOneFour,
        };
        assert_eq!(
            named_wide
                .validate_for_descriptor(&clap_descriptor())
                .unwrap_err(),
            "CLAP standard surround does not represent the selected wide speaker target"
        );
        let named = NativePluginAudioSetup::Ambisonics {
            order: 7,
            target_layout: NativeAmbisonicsTargetLayout::SevenOneFour,
        };
        assert!(named.validate_for_descriptor(&clap_descriptor()).is_ok());
    }

    #[test]
    fn custom_geometry_eq_is_reflexive_for_validated_values() {
        let custom = geometry_from_const(&CONFIG_7_1_4);
        assert!(custom.validate().is_ok());
        assert_eq!(custom, custom.clone());
        let setup = custom_setup(7, custom);
        assert_eq!(setup, setup.clone());
    }
}

#[cfg(test)]
mod crossover_setup_tests {
    use super::{
        ExternalPluginState, NativeCrossoverInputLayout as InputLayout,
        NativeCrossoverMode as Mode, NativeCrossoverOutputLayout as OutputLayout,
        NativeCrossoverTopology as Topology, NativePluginAudioSetup,
    };
    use crate::external_plugin::plugin_descriptor::PluginDescriptor;
    use crate::external_plugin::plugin_format::PluginFormat;
    use crate::external_plugin::types::{ExternalPluginSandboxMode, PluginScanStatus};
    use std::path::PathBuf;

    fn descriptor(format: PluginFormat, id: &str) -> PluginDescriptor {
        PluginDescriptor {
            id: id.into(),
            name: "SOTF: Crossover".into(),
            vendor: "SOTF".into(),
            version: "test".into(),
            format,
            path: PathBuf::from("/test/crossover"),
            audio_inputs: 2,
            audio_outputs: 2,
            is_instrument: false,
            categories: Vec::new(),
            scan_status: PluginScanStatus::Loadable,
        }
    }

    fn setup(
        input_layout: InputLayout,
        topology: Topology,
        mode: Mode,
        num_bands: u8,
        output_layout: OutputLayout,
    ) -> NativePluginAudioSetup {
        NativePluginAudioSetup::Crossover {
            input_layout,
            num_bands,
            topology,
            mode,
            output_layout,
        }
    }

    #[test]
    fn crossover_setup_preserves_named_width_and_route_semantics() {
        let clap = setup(
            InputLayout::FiveOne,
            Topology::Bands,
            Mode::Both,
            4,
            OutputLayout::ClapPacked,
        );
        let vst3 = setup(
            InputLayout::FiveOne,
            Topology::Bands,
            Mode::Both,
            4,
            OutputLayout::Vst3Buses,
        );
        assert_eq!(clap.channel_counts().unwrap(), (6, 24));
        assert_eq!(vst3.channel_counts().unwrap(), (6, 24));
        assert_eq!(
            setup(
                InputLayout::FiveOne,
                Topology::Bands,
                Mode::Highpass,
                4,
                OutputLayout::ClapPacked,
            )
            .channel_counts()
            .unwrap(),
            (6, 6)
        );
        assert_eq!(
            setup(
                InputLayout::FiveOne,
                Topology::PerChannel,
                Mode::Both,
                4,
                OutputLayout::ClapPacked,
            )
            .channel_counts()
            .unwrap(),
            (6, 6)
        );
    }

    #[test]
    fn effective_audio_channel_counts_prefers_valid_native_setup() {
        let descriptor = descriptor(PluginFormat::Clap, "org.spinorama.sotf.crossover");
        let mut state =
            ExternalPluginState::new(descriptor, ExternalPluginSandboxMode::Isolated, Vec::new());
        assert_eq!(state.effective_audio_channel_counts().unwrap(), (2, 2));

        state.audio_setup = Some(setup(
            InputLayout::SevenOne,
            Topology::Bands,
            Mode::Both,
            4,
            OutputLayout::ClapPacked,
        ));
        assert_eq!(state.effective_audio_channel_counts().unwrap(), (8, 32));
    }

    #[test]
    fn effective_audio_channel_counts_rejects_invalid_native_setup() {
        let descriptor = descriptor(PluginFormat::Clap, "org.spinorama.sotf.crossover");
        let mut state =
            ExternalPluginState::new(descriptor, ExternalPluginSandboxMode::Isolated, Vec::new());
        state.audio_setup = Some(setup(
            InputLayout::SevenOne,
            Topology::Bands,
            Mode::Both,
            4,
            OutputLayout::Vst3Buses,
        ));

        assert!(state.effective_audio_channel_counts().is_err());
    }

    #[test]
    fn crossover_setup_is_typed_to_plugin_format_and_round_trips() {
        let clap_setup = setup(
            InputLayout::SevenOne,
            Topology::Bands,
            Mode::Both,
            3,
            OutputLayout::ClapPacked,
        );
        let clap_descriptor = descriptor(PluginFormat::Clap, "org.spinorama.sotf.crossover");
        assert!(clap_setup.validate_for_descriptor(&clap_descriptor).is_ok());
        let state = ExternalPluginState {
            schema_version: ExternalPluginState::SCHEMA_VERSION,
            descriptor: clap_descriptor.clone(),
            format: clap_descriptor.format,
            plugin_id: clap_descriptor.id.clone(),
            plugin_path: clap_descriptor.path.clone(),
            sandbox_mode: ExternalPluginSandboxMode::InProcess,
            opaque_state: vec![1, 2, 3],
            audio_setup: Some(clap_setup),
        };
        let encoded = serde_json::to_vec(&state).unwrap();
        let decoded: ExternalPluginState = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, state);

        assert!(
            setup(
                InputLayout::Stereo,
                Topology::Bands,
                Mode::Both,
                2,
                OutputLayout::Vst3Buses,
            )
            .validate_for_descriptor(&clap_descriptor)
            .is_err()
        );
        assert!(
            setup(
                InputLayout::NineOneSixWide,
                Topology::Bands,
                Mode::Both,
                4,
                OutputLayout::Vst3Buses,
            )
            .validate_for_descriptor(&descriptor(
                PluginFormat::Vst3,
                "536F746643726F73736F766572303031",
            ))
            .is_ok()
        );
    }

    #[test]
    fn crossover_setup_rejects_invalid_band_count_and_device_width_overflow() {
        assert!(
            setup(
                InputLayout::Stereo,
                Topology::Bands,
                Mode::Both,
                1,
                OutputLayout::ClapPacked,
            )
            .channel_counts()
            .is_err()
        );
        assert!(
            setup(
                InputLayout::NineOneSixWide,
                Topology::Bands,
                Mode::Both,
                5,
                OutputLayout::Vst3Buses,
            )
            .channel_counts()
            .is_err()
        );
        assert!(
            setup(
                InputLayout::NineOneSixWide,
                Topology::Bands,
                Mode::Both,
                4,
                OutputLayout::Vst3Buses,
            )
            .channel_counts()
            .is_ok_and(|(_, outputs)| outputs == 64)
        );
    }

    #[test]
    fn sidechain_setup_packs_program_then_key_and_round_trips() {
        let stereo = NativePluginAudioSetup::Sidechain {
            main_channels: 2,
            key_channels: 2,
        };
        assert_eq!(stereo.channel_counts().unwrap(), (4, 2));
        assert_eq!(
            NativePluginAudioSetup::Sidechain {
                main_channels: 1,
                key_channels: 2,
            }
            .channel_counts()
            .unwrap(),
            (3, 1)
        );
        for (main_channels, key_channels) in [(0, 2), (2, 0), (3, 2), (2, 3)] {
            assert!(
                NativePluginAudioSetup::Sidechain {
                    main_channels,
                    key_channels,
                }
                .channel_counts()
                .is_err(),
                "route {main_channels}+{key_channels} must be refused"
            );
        }

        let clap = descriptor(PluginFormat::Clap, "org.spinorama.sotf.de-esser");
        let vst3 = descriptor(PluginFormat::Vst3, "536F7466446545737365723030303031");
        assert!(stereo.validate_for_descriptor(&clap).is_ok());
        assert!(stereo.validate_for_descriptor(&vst3).is_ok());
        let gate_clap = descriptor(PluginFormat::Clap, "org.spinorama.sotf.gate");
        let gate_vst3 = descriptor(PluginFormat::Vst3, "536F7466476174653030303030303031");
        assert!(stereo.validate_for_descriptor(&gate_clap).is_ok());
        assert!(stereo.validate_for_descriptor(&gate_vst3).is_ok());
        assert!(
            stereo
                .validate_for_descriptor(&descriptor(
                    PluginFormat::Clap,
                    "org.spinorama.sotf.crossover"
                ))
                .is_err()
        );
        assert!(
            stereo
                .validate_for_descriptor(&descriptor(
                    PluginFormat::AudioUnit,
                    "org.spinorama.sotf.de-esser"
                ))
                .is_err()
        );

        let encoded = serde_json::to_value(&stereo).unwrap();
        assert_eq!(
            encoded,
            serde_json::json!({
                "type": "sidechain",
                "main_channels": 2,
                "key_channels": 2,
            })
        );
        let decoded: NativePluginAudioSetup = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, stereo);
    }
}
