//! Named native upmixer layouts and host channel mappings.

// Rust guideline compliant 2026-10-09

use nih_plug::audio_setup::{AudioIOLayout, PortNames, new_nonzero_u32};
use nih_plug::context::PluginApi;
use sotf_host::external_plugin::NativeCrossoverInputLayout as ExistingLayout;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpeakerLayout {
    Stereo,
    FiveZero,
    FiveOne,
    FiveOneTwo,
    FiveOneFour,
    FiveOneSix,
    SevenZero,
    SevenOne,
    SevenOneTwo,
    SevenOneFour,
    NineOneFour,
    NineOneSix,
}

impl SpeakerLayout {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Stereo => "2.0",
            Self::FiveZero => "5.0",
            Self::FiveOne => "5.1",
            Self::FiveOneTwo => "5.1.2",
            Self::FiveOneFour => "5.1.4",
            Self::FiveOneSix => "5.1.6",
            Self::SevenZero => "7.0",
            Self::SevenOne => "7.1",
            Self::SevenOneTwo => "7.1.2",
            Self::SevenOneFour => "7.1.4",
            Self::NineOneFour => "9.1.4",
            Self::NineOneSix => "9.1.6",
        }
    }

    const fn existing(self) -> Option<ExistingLayout> {
        Some(match self {
            Self::Stereo => ExistingLayout::Stereo,
            Self::FiveZero | Self::FiveOneSix | Self::SevenZero => return None,
            Self::FiveOne => ExistingLayout::FiveOne,
            Self::FiveOneTwo => ExistingLayout::FiveOneTwo,
            Self::FiveOneFour => ExistingLayout::FiveOneFour,
            Self::SevenOne => ExistingLayout::SevenOne,
            Self::SevenOneTwo => ExistingLayout::SevenOneTwo,
            Self::SevenOneFour => ExistingLayout::SevenOneFour,
            Self::NineOneFour => ExistingLayout::NineOneFour,
            Self::NineOneSix => ExistingLayout::NineOneSixWide,
        })
    }

    pub const fn channels(self) -> usize {
        if matches!(self, Self::FiveOneSix) {
            return 12;
        }
        if matches!(self, Self::SevenZero) {
            return 7;
        }
        match self.existing() {
            Some(layout) => layout.channel_count(),
            None => 5,
        }
    }

    pub const fn vst3_arrangement(self) -> u64 {
        if matches!(self, Self::SevenZero) {
            // The 7.1 arrangement (0x063f) without the LFE speaker (bit 3).
            return 0x0637;
        }
        if matches!(self, Self::FiveOneSix) {
            // 5.1 plus top-front, top-back, and top-side pairs.
            return 0x0302_d03f;
        }
        match self.existing() {
            Some(layout) => match layout.vst3_speaker_arrangement() {
                Some(mask) => mask,
                None => 0,
            },
            None => 0x37,
        }
    }

    pub fn host_to_sotf(self, api: PluginApi, channel: usize) -> Option<usize> {
        if api != PluginApi::Vst3 {
            return (channel < self.channels()).then_some(channel);
        }
        if matches!(self, Self::SevenZero) {
            // VST3 orders the back pair before the side pair, as in 7.1
            // without its LFE channel.
            return [0, 1, 2, 5, 6, 3, 4].get(channel).copied();
        }
        match self.existing() {
            Some(layout) => layout.vst3_bus_to_sotf_permutation().get(channel).copied(),
            None => (channel < self.channels()).then_some(channel),
        }
    }

    fn clap_channel_map(self) -> Option<&'static [u8]> {
        Some(match self {
            Self::Stereo => return None,
            Self::FiveZero => &[0, 1, 2, 9, 10],
            Self::FiveOne => &super::CLAP_MAP_51,
            Self::FiveOneTwo => &super::CLAP_MAP_512,
            Self::FiveOneFour => &super::CLAP_MAP_514,
            // CLAP's surround map has no top-middle left/right roles.
            Self::FiveOneSix => return None,
            Self::SevenZero => &[0, 1, 2, 9, 10, 4, 5],
            Self::SevenOne => &super::CLAP_MAP_71,
            Self::SevenOneTwo => &super::CLAP_MAP_712,
            Self::SevenOneFour => &super::CLAP_MAP_714,
            // CLAP does not define the front-wide speaker roles used here.
            Self::NineOneFour | Self::NineOneSix => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayoutPair {
    pub input: SpeakerLayout,
    pub output: SpeakerLayout,
    name: &'static str,
}

use SpeakerLayout::{
    FiveOne as F1, FiveOneFour as F14, FiveOneSix as F16, FiveOneTwo as F12, FiveZero as F0,
};
use SpeakerLayout::{NineOneFour as N14, NineOneSix as N16, SevenOne as S1, SevenOneFour as S14};
use SpeakerLayout::{SevenOneTwo as S12, SevenZero as S0, Stereo as St};

const VST3_PAIRS: [LayoutPair; 32] = [
    LayoutPair {
        input: St,
        output: F1,
        name: "Stereo to 5.1",
    },
    LayoutPair {
        input: F0,
        output: F0,
        name: "5.0 to 5.0",
    },
    LayoutPair {
        input: F0,
        output: F1,
        name: "5.0 to 5.1",
    },
    LayoutPair {
        input: F0,
        output: F12,
        name: "5.0 to 5.1.2",
    },
    LayoutPair {
        input: F0,
        output: F14,
        name: "5.0 to 5.1.4",
    },
    LayoutPair {
        input: F0,
        output: S1,
        name: "5.0 to 7.1",
    },
    LayoutPair {
        input: F0,
        output: S12,
        name: "5.0 to 7.1.2",
    },
    LayoutPair {
        input: F0,
        output: S14,
        name: "5.0 to 7.1.4",
    },
    LayoutPair {
        input: F0,
        output: N14,
        name: "5.0 to 9.1.4",
    },
    LayoutPair {
        input: F0,
        output: N16,
        name: "5.0 to 9.1.6",
    },
    LayoutPair {
        input: F1,
        output: F1,
        name: "5.1 to 5.1",
    },
    LayoutPair {
        input: F1,
        output: F12,
        name: "5.1 to 5.1.2",
    },
    LayoutPair {
        input: F1,
        output: F14,
        name: "5.1 to 5.1.4",
    },
    LayoutPair {
        input: F1,
        output: S1,
        name: "5.1 to 7.1",
    },
    LayoutPair {
        input: F1,
        output: S12,
        name: "5.1 to 7.1.2",
    },
    LayoutPair {
        input: F1,
        output: S14,
        name: "5.1 to 7.1.4",
    },
    LayoutPair {
        input: F1,
        output: N14,
        name: "5.1 to 9.1.4",
    },
    LayoutPair {
        input: F1,
        output: N16,
        name: "5.1 to 9.1.6",
    },
    LayoutPair {
        input: S1,
        output: S1,
        name: "7.1 to 7.1",
    },
    LayoutPair {
        input: S1,
        output: S12,
        name: "7.1 to 7.1.2",
    },
    LayoutPair {
        input: S1,
        output: S14,
        name: "7.1 to 7.1.4",
    },
    LayoutPair {
        input: S1,
        output: N14,
        name: "7.1 to 9.1.4",
    },
    LayoutPair {
        input: S1,
        output: N16,
        name: "7.1 to 9.1.6",
    },
    LayoutPair {
        input: F0,
        output: F16,
        name: "5.0 to 5.1.6",
    },
    LayoutPair {
        input: F1,
        output: F16,
        name: "5.1 to 5.1.6",
    },
    LayoutPair {
        input: F0,
        output: S0,
        name: "5.0 to 7.0",
    },
    LayoutPair {
        input: S0,
        output: S0,
        name: "7.0 to 7.0",
    },
    LayoutPair {
        input: S0,
        output: S1,
        name: "7.0 to 7.1",
    },
    LayoutPair {
        input: S0,
        output: S12,
        name: "7.0 to 7.1.2",
    },
    LayoutPair {
        input: S0,
        output: S14,
        name: "7.0 to 7.1.4",
    },
    LayoutPair {
        input: S0,
        output: N14,
        name: "7.0 to 9.1.4",
    },
    LayoutPair {
        input: S0,
        output: N16,
        name: "7.0 to 9.1.6",
    },
];

const CLAP_PAIRS: [LayoutPair; 22] = [
    VST3_PAIRS[0],
    VST3_PAIRS[1],
    VST3_PAIRS[2],
    VST3_PAIRS[3],
    VST3_PAIRS[4],
    VST3_PAIRS[5],
    VST3_PAIRS[6],
    VST3_PAIRS[7],
    VST3_PAIRS[10],
    VST3_PAIRS[11],
    VST3_PAIRS[12],
    VST3_PAIRS[13],
    VST3_PAIRS[14],
    VST3_PAIRS[15],
    VST3_PAIRS[18],
    VST3_PAIRS[19],
    VST3_PAIRS[20],
    VST3_PAIRS[25],
    VST3_PAIRS[26],
    VST3_PAIRS[27],
    VST3_PAIRS[28],
    VST3_PAIRS[29],
];

const fn make_layouts<const N: usize>(pairs: &[LayoutPair; N]) -> [AudioIOLayout; N] {
    let mut layouts = [AudioIOLayout::const_default(); N];
    let mut index = 0;
    while index < N {
        let pair = pairs[index];
        layouts[index] = AudioIOLayout {
            main_input_channels: Some(new_nonzero_u32(pair.input.channels() as u32)),
            main_output_channels: Some(new_nonzero_u32(pair.output.channels() as u32)),
            aux_input_ports: &[],
            aux_output_ports: &[],
            names: PortNames {
                layout: Some(pair.name),
                main_input: Some("Input bed"),
                main_output: Some("Speakers"),
                ..PortNames::const_default()
            },
        };
        index += 1;
    }
    layouts
}

pub static VST3_LAYOUTS: [AudioIOLayout; 32] = make_layouts(&VST3_PAIRS);
pub static CLAP_LAYOUTS: [AudioIOLayout; 22] = make_layouts(&CLAP_PAIRS);

pub fn pair(index: usize, api: PluginApi) -> Option<LayoutPair> {
    match api {
        PluginApi::Clap => CLAP_PAIRS.get(index).copied(),
        PluginApi::Vst3 | PluginApi::Standalone => VST3_PAIRS.get(index).copied(),
    }
}

pub fn layout_index(layout: &AudioIOLayout, api: PluginApi) -> Option<usize> {
    let layouts: &[AudioIOLayout] = match api {
        PluginApi::Clap => &CLAP_LAYOUTS,
        PluginApi::Vst3 | PluginApi::Standalone => &VST3_LAYOUTS,
    };
    layouts.iter().position(|candidate| candidate == layout)
}

pub fn vst3_bus_arrangement(index: usize, is_input: bool, bus_index: usize) -> Option<u64> {
    if bus_index != 0 {
        return None;
    }
    let pair = pair(index, PluginApi::Vst3)?;
    Some(if is_input { pair.input } else { pair.output }.vst3_arrangement())
}

pub fn clap_channel_map(index: usize, is_input: bool, port_index: usize) -> Option<&'static [u8]> {
    if port_index != 0 {
        return None;
    }
    let pair = pair(index, PluginApi::Clap)?;
    if is_input { pair.input } else { pair.output }.clap_channel_map()
}

pub fn clap_channel_mask_supported(mask: u64) -> bool {
    (0..CLAP_PAIRS.len()).any(|index| {
        [true, false].into_iter().any(|is_input| {
            clap_channel_map(index, is_input, 0).is_some_and(|map| {
                map.iter()
                    .fold(0_u64, |result, channel| result | (1_u64 << channel))
                    == mask
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sotf_host::speaker_config::{ChannelLayout, get_speaker_config};

    #[test]
    fn five_one_six_is_vst3_only_with_exact_speaker_roles() {
        use nih_plug::wrapper::vst3::vst3_sys::vst::{k51, kStereoTF, kStereoTR, kStereoTS};
        let expected = k51 | kStereoTF | kStereoTR | kStereoTS;
        for input in [SpeakerLayout::FiveZero, SpeakerLayout::FiveOne] {
            let index = VST3_PAIRS
                .iter()
                .position(|pair| pair.input == input && pair.output.id() == "5.1.6")
                .expect("VST3 must advertise the 5.1.6 output");
            assert_eq!(vst3_bus_arrangement(index, false, 0), Some(expected));
            let layout = pair(index, PluginApi::Vst3).unwrap().output;
            assert_eq!(layout.channels(), 12);
            for channel in 0..12 {
                assert_eq!(layout.host_to_sotf(PluginApi::Vst3, channel), Some(channel));
            }
        }
        assert!(!CLAP_PAIRS.iter().any(|pair| pair.output.id() == "5.1.6"));
    }

    #[test]
    fn seven_zero_uses_the_seven_one_host_order_without_lfe() {
        let layout = SpeakerLayout::SevenZero;
        assert_eq!(layout.id(), "7.0");
        assert_eq!(layout.channels(), 7);
        assert_eq!(
            layout.vst3_arrangement(),
            SpeakerLayout::SevenOne.vst3_arrangement() & !(1 << 3)
        );
        let permutation: Vec<_> = (0..7)
            .map(|channel| layout.host_to_sotf(PluginApi::Vst3, channel).unwrap())
            .collect();
        assert_eq!(permutation, [0, 1, 2, 5, 6, 3, 4]);
        let index = CLAP_PAIRS
            .iter()
            .position(|pair| pair.input == layout && pair.output == layout)
            .expect("CLAP must advertise the 7.0 input");
        assert_eq!(
            clap_channel_map(index, true, 0),
            Some([0_u8, 1, 2, 9, 10, 4, 5].as_slice())
        );
    }

    #[test]
    fn declared_surround_pairs_preserve_roles_and_have_distinct_host_layouts() {
        for api in [PluginApi::Clap, PluginApi::Vst3] {
            let layouts: &[AudioIOLayout] = if api == PluginApi::Clap {
                &CLAP_LAYOUTS
            } else {
                &VST3_LAYOUTS
            };
            for (index, layout) in layouts.iter().enumerate() {
                assert_eq!(layout_index(layout, api), Some(index));
                let pair = pair(index, api).unwrap();
                let input = ChannelLayout::from_speaker_config(
                    get_speaker_config(pair.input.id()).unwrap(),
                )
                .unwrap();
                let output = ChannelLayout::from_speaker_config(
                    get_speaker_config(pair.output.id()).unwrap(),
                )
                .unwrap();
                if pair.input != SpeakerLayout::Stereo {
                    for channel in &input.channels {
                        assert!(
                            output
                                .channels
                                .iter()
                                .any(|candidate| candidate.role == channel.role)
                        );
                    }
                }
                for (is_input, speaker) in [(true, pair.input), (false, pair.output)] {
                    let mut permutation: Vec<_> = (0..speaker.channels())
                        .map(|channel| speaker.host_to_sotf(api, channel).unwrap())
                        .collect();
                    permutation.sort_unstable();
                    assert_eq!(permutation, (0..speaker.channels()).collect::<Vec<_>>());
                    if api == PluginApi::Clap && speaker != SpeakerLayout::Stereo {
                        assert_eq!(
                            clap_channel_map(index, is_input, 0).unwrap().len(),
                            speaker.channels()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn same_width_height_and_surround_layouts_use_distinct_vst3_speakers() {
        assert_eq!(
            SpeakerLayout::FiveOneFour.channels(),
            SpeakerLayout::SevenOneTwo.channels()
        );
        assert_ne!(
            SpeakerLayout::FiveOneFour.vst3_arrangement(),
            SpeakerLayout::SevenOneTwo.vst3_arrangement()
        );
        assert_eq!(CLAP_LAYOUTS.len(), 22);
        assert!(CLAP_PAIRS.iter().all(|pair| !matches!(
            pair.output,
            SpeakerLayout::NineOneFour | SpeakerLayout::NineOneSix
        )));
    }
}
