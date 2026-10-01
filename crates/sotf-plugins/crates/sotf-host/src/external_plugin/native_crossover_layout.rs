use serde::{Deserialize, Serialize};

/// A named Crossover input layout. Variants with the same width remain distinct
/// because hosts assign different speaker identities to their channel indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeCrossoverInputLayout {
    Mono,
    Stereo,
    Quad,
    FiveOne,
    SevenOne,
    FiveOneTwo,
    FiveOneFour,
    SevenOneTwo,
    SevenOneFour,
    NineOneFour,
    NineOneSixWide,
}

impl NativeCrossoverInputLayout {
    /// Infers a name only when a width identifies exactly one supported layout.
    /// Eight- and ten-channel inputs are intentionally ambiguous.
    pub const fn unique_for_channel_count(channels: usize) -> Option<Self> {
        match channels {
            1 => Some(Self::Mono),
            2 => Some(Self::Stereo),
            4 => Some(Self::Quad),
            6 => Some(Self::FiveOne),
            12 => Some(Self::SevenOneFour),
            14 => Some(Self::NineOneFour),
            16 => Some(Self::NineOneSixWide),
            _ => None,
        }
    }

    pub const fn channel_count(self) -> usize {
        match self {
            Self::Mono => 1,
            Self::Stereo => 2,
            Self::Quad => 4,
            Self::FiveOne => 6,
            Self::SevenOne | Self::FiveOneTwo => 8,
            Self::FiveOneFour | Self::SevenOneTwo => 10,
            Self::SevenOneFour => 12,
            Self::NineOneFour => 14,
            Self::NineOneSixWide => 16,
        }
    }

    /// Stable index in the Crossover CLAP audio-port configuration table.
    /// Stereo remains index zero for the legacy default instance layout.
    pub const fn clap_configuration_index(self) -> usize {
        match self {
            Self::Stereo => 0,
            Self::Mono => 1,
            Self::Quad => 2,
            Self::FiveOne => 3,
            Self::SevenOne => 4,
            Self::FiveOneTwo => 5,
            Self::FiveOneFour => 6,
            Self::SevenOneTwo => 7,
            Self::SevenOneFour => 8,
            Self::NineOneFour => 9,
            Self::NineOneSixWide => 10,
        }
    }

    pub fn clap_configuration_id(self, output_channels: usize) -> Option<u32> {
        let width = self.channel_count();
        let output_variant = if output_channels == width {
            0
        } else if output_channels == width * 2 {
            1
        } else if output_channels == width * 3 {
            2
        } else if output_channels == width * 4 {
            3
        } else {
            return None;
        };
        u32::try_from(self.clap_configuration_index() * 4 + output_variant).ok()
    }

    /// VST3 SDK speaker arrangement for this exact named layout.
    pub const fn vst3_speaker_arrangement(self) -> Option<u64> {
        match self {
            // These are SDK speaker identities, not width masks: kMono is
            // kSpeakerM (bit 19), and k40Music is FL/FR/LS/RS.
            Self::Mono => Some(0x0000_0000_0008_0000),
            Self::Stereo => Some(0x0000_0000_0000_0003),
            Self::Quad => Some(0x0000_0000_0000_0033),
            Self::FiveOne => Some(0x0000_0000_0000_003f),
            Self::SevenOne => Some(0x0000_0000_0000_063f),
            Self::FiveOneTwo => Some(0x0000_0000_0000_503f),
            Self::FiveOneFour => Some(0x0000_0000_0002_d03f),
            Self::SevenOneTwo => Some(0x0000_0000_0000_563f),
            Self::SevenOneFour => Some(0x0000_0000_0002_d63f),
            Self::NineOneFour => Some(0x1800_0000_0002_d63f),
            Self::NineOneSixWide => Some(0x1800_0000_0302_d63f),
        }
    }

    /// Maps host bus-channel indices into SOTF speaker order. The same map is
    /// applied independently to every band in a Both output.
    pub const fn vst3_bus_to_sotf_permutation(self) -> &'static [usize] {
        match self {
            Self::Mono => &[0],
            Self::Stereo => &[0, 1],
            Self::Quad => &[0, 1, 2, 3],
            Self::FiveOne => &[0, 1, 2, 3, 4, 5],
            Self::FiveOneTwo => &[0, 1, 2, 3, 4, 5, 6, 7],
            Self::FiveOneFour => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
            Self::SevenOne => &[0, 1, 2, 3, 6, 7, 4, 5],
            Self::SevenOneTwo => &[0, 1, 2, 3, 6, 7, 4, 5, 8, 9],
            Self::SevenOneFour => &[0, 1, 2, 3, 6, 7, 4, 5, 8, 9, 10, 11],
            Self::NineOneFour => &[0, 1, 2, 3, 6, 7, 4, 5, 10, 11, 12, 13, 8, 9],
            Self::NineOneSixWide => &[0, 1, 2, 3, 6, 7, 4, 5, 10, 11, 12, 13, 14, 15, 8, 9],
        }
    }

    pub fn band_major_sotf_index(self, band: usize, bus_channel: usize) -> Option<usize> {
        let channel = *self.vst3_bus_to_sotf_permutation().get(bus_channel)?;
        band.checked_mul(self.channel_count())?.checked_add(channel)
    }
}

/// Native output representation selected for the exact SOTF Crossover plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeCrossoverOutputLayout {
    /// One CLAP output port carries band-major channels.
    ClapPacked,
    /// Four fixed-width VST3 output buses carry bands one through four.
    Vst3Buses,
}

/// Crossover mode persisted by the native plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeCrossoverMode {
    Lowpass,
    Highpass,
    Both,
}

/// Selects global band cutoffs or independent per-channel filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeCrossoverTopology {
    Bands,
    PerChannel,
}

/// Readback of the native Crossover structural state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeCrossoverStructure {
    pub mode: NativeCrossoverMode,
    pub topology: NativeCrossoverTopology,
    pub num_bands: u8,
}

impl NativeCrossoverStructure {
    pub const fn output_channels(self, input_channels: usize) -> Option<usize> {
        if matches!(self.topology, NativeCrossoverTopology::Bands)
            && matches!(self.mode, NativeCrossoverMode::Both)
        {
            input_channels.checked_mul(self.num_bands as usize)
        } else {
            Some(input_channels)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{NativeCrossoverInputLayout as Layout, NativeCrossoverMode as Mode};
    use super::{NativeCrossoverStructure, NativeCrossoverTopology as Topology};

    #[test]
    fn same_width_named_layouts_keep_distinct_identity_and_permutation() {
        assert_eq!(
            Layout::SevenOne.channel_count(),
            Layout::FiveOneTwo.channel_count()
        );
        assert_ne!(
            Layout::SevenOne.vst3_speaker_arrangement(),
            Layout::FiveOneTwo.vst3_speaker_arrangement()
        );
        assert_ne!(
            Layout::SevenOne.vst3_bus_to_sotf_permutation(),
            Layout::FiveOneTwo.vst3_bus_to_sotf_permutation()
        );
        assert_eq!(Layout::SevenOne.band_major_sotf_index(2, 4), Some(22));
        assert_eq!(Layout::FiveOneTwo.band_major_sotf_index(2, 4), Some(20));
    }

    #[test]
    fn named_layout_widths_cover_mono_through_sixteen_channels() {
        assert_eq!(
            [
                Layout::Mono.channel_count(),
                Layout::Stereo.channel_count(),
                Layout::Quad.channel_count(),
                Layout::FiveOne.channel_count(),
                Layout::SevenOne.channel_count(),
                Layout::FiveOneTwo.channel_count(),
                Layout::FiveOneFour.channel_count(),
                Layout::SevenOneTwo.channel_count(),
                Layout::SevenOneFour.channel_count(),
                Layout::NineOneFour.channel_count(),
                Layout::NineOneSixWide.channel_count(),
            ],
            [1, 2, 4, 6, 8, 8, 10, 10, 12, 14, 16]
        );
    }

    #[test]
    fn crossover_output_width_depends_on_mode_and_topology_not_missing_buffers() {
        let both_bands = NativeCrossoverStructure {
            mode: Mode::Both,
            topology: Topology::Bands,
            num_bands: 4,
        };
        assert_eq!(both_bands.output_channels(6), Some(24));
        assert_eq!(
            NativeCrossoverStructure {
                topology: Topology::PerChannel,
                ..both_bands
            }
            .output_channels(6),
            Some(6)
        );
        assert_eq!(
            NativeCrossoverStructure {
                mode: Mode::Highpass,
                ..both_bands
            }
            .output_channels(6),
            Some(6)
        );
    }

    #[test]
    fn clap_configuration_ids_are_unique_for_packed_output_widths() {
        let layouts = [
            Layout::Stereo,
            Layout::Mono,
            Layout::Quad,
            Layout::FiveOne,
            Layout::SevenOne,
            Layout::FiveOneTwo,
            Layout::FiveOneFour,
            Layout::SevenOneTwo,
            Layout::SevenOneFour,
            Layout::NineOneFour,
            Layout::NineOneSixWide,
        ];
        let mut ids = Vec::new();
        for layout in layouts {
            for bands in 1..=4 {
                let width = layout.channel_count() * bands;
                let id = layout.clap_configuration_id(width).unwrap();
                assert!(!ids.contains(&id));
                ids.push(id);
            }
        }
        assert_eq!(ids.len(), 44);
        assert_eq!(Layout::Stereo.clap_configuration_id(2), Some(0));
        assert_eq!(Layout::Stereo.clap_configuration_id(4), Some(1));
        assert_eq!(Layout::Stereo.clap_configuration_id(6), Some(2));
        assert_eq!(Layout::Stereo.clap_configuration_id(8), Some(3));
    }
}
