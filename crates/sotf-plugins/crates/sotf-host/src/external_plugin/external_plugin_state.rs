use super::plugin_descriptor::PluginDescriptor;
use super::plugin_format::PluginFormat;
use super::types::ExternalPluginSandboxMode;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const AMBISONICS_CLAP_PLUGIN_ID: &str = "org.spinorama.sotf.ambisonics";
const AMBISONICS_VST3_CLASS_ID: &str = "536F7466416D6269736E696330303031";
const BAND_SPLIT_CLAP_PLUGIN_ID: &str = "org.spinorama.sotf.band-split";
const BAND_SPLIT_VST3_CLASS_ID: &str = "536F746642616E6453706C7430303031";

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
    /// Selects an indexed multi-band output route for the SOTF BandSplit plugin.
    BandSplit {
        /// Number of stereo bands produced by the processor, from two through four.
        num_bands: u8,
        /// Format-specific output-bus representation.
        output_layout: NativeBandSplitOutputLayout,
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

    #[cfg(feature = "external-plugin-clap")]
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

    #[cfg(feature = "external-plugin-vst3")]
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
            Self::BandSplit { num_bands, .. } if (2..=4).contains(num_bands) => {
                Ok((2, usize::from(*num_bands) * 2))
            }
            Self::BandSplit { num_bands, .. } => Err(format!(
                "BandSplit band count {num_bands} is unsupported; expected two through four"
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
