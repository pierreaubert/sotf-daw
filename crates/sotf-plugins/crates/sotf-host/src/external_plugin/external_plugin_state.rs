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
}
