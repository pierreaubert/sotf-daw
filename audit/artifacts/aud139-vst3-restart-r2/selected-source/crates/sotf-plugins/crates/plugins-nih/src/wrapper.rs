//! Macro-based wrapper that generates nih-plug Plugin implementations for SOTF plugins.

use nih_plug::audio_setup::{AudioIOLayout, PortNames, new_nonzero_u32};
use nih_plug::context::PluginApi;

const AMBISONICS_OUTPUT_WIDTHS: [u32; 8] = [6, 8, 8, 10, 10, 12, 14, 16];
const AMBISONICS_LAYOUT_NAMES: [&str; 56] = [
    "Order 1 - 5.1",
    "Order 1 - 7.1",
    "Order 1 - 5.1.2",
    "Order 1 - 7.1.2",
    "Order 1 - 5.1.4",
    "Order 1 - 7.1.4",
    "Order 1 - 9.1.4",
    "Order 1 - 9.1.6",
    "Order 2 - 5.1",
    "Order 2 - 7.1",
    "Order 2 - 5.1.2",
    "Order 2 - 7.1.2",
    "Order 2 - 5.1.4",
    "Order 2 - 7.1.4",
    "Order 2 - 9.1.4",
    "Order 2 - 9.1.6",
    "Order 3 - 5.1",
    "Order 3 - 7.1",
    "Order 3 - 5.1.2",
    "Order 3 - 7.1.2",
    "Order 3 - 5.1.4",
    "Order 3 - 7.1.4",
    "Order 3 - 9.1.4",
    "Order 3 - 9.1.6",
    "Order 4 - 5.1",
    "Order 4 - 7.1",
    "Order 4 - 5.1.2",
    "Order 4 - 7.1.2",
    "Order 4 - 5.1.4",
    "Order 4 - 7.1.4",
    "Order 4 - 9.1.4",
    "Order 4 - 9.1.6",
    "Order 5 - 5.1",
    "Order 5 - 7.1",
    "Order 5 - 5.1.2",
    "Order 5 - 7.1.2",
    "Order 5 - 5.1.4",
    "Order 5 - 7.1.4",
    "Order 5 - 9.1.4",
    "Order 5 - 9.1.6",
    "Order 6 - 5.1",
    "Order 6 - 7.1",
    "Order 6 - 5.1.2",
    "Order 6 - 7.1.2",
    "Order 6 - 5.1.4",
    "Order 6 - 7.1.4",
    "Order 6 - 9.1.4",
    "Order 6 - 9.1.6",
    "Order 7 - 5.1",
    "Order 7 - 7.1",
    "Order 7 - 5.1.2",
    "Order 7 - 7.1.2",
    "Order 7 - 5.1.4",
    "Order 7 - 7.1.4",
    "Order 7 - 9.1.4",
    "Order 7 - 9.1.6",
];
const CLAP_AMBISONICS_LAYOUT_NAMES: [&str; 42] = [
    "Order 1 - 5.1",
    "Order 1 - 7.1",
    "Order 1 - 5.1.2",
    "Order 1 - 7.1.2",
    "Order 1 - 5.1.4",
    "Order 1 - 7.1.4",
    "Order 2 - 5.1",
    "Order 2 - 7.1",
    "Order 2 - 5.1.2",
    "Order 2 - 7.1.2",
    "Order 2 - 5.1.4",
    "Order 2 - 7.1.4",
    "Order 3 - 5.1",
    "Order 3 - 7.1",
    "Order 3 - 5.1.2",
    "Order 3 - 7.1.2",
    "Order 3 - 5.1.4",
    "Order 3 - 7.1.4",
    "Order 4 - 5.1",
    "Order 4 - 7.1",
    "Order 4 - 5.1.2",
    "Order 4 - 7.1.2",
    "Order 4 - 5.1.4",
    "Order 4 - 7.1.4",
    "Order 5 - 5.1",
    "Order 5 - 7.1",
    "Order 5 - 5.1.2",
    "Order 5 - 7.1.2",
    "Order 5 - 5.1.4",
    "Order 5 - 7.1.4",
    "Order 6 - 5.1",
    "Order 6 - 7.1",
    "Order 6 - 5.1.2",
    "Order 6 - 7.1.2",
    "Order 6 - 5.1.4",
    "Order 6 - 7.1.4",
    "Order 7 - 5.1",
    "Order 7 - 7.1",
    "Order 7 - 5.1.2",
    "Order 7 - 7.1.2",
    "Order 7 - 5.1.4",
    "Order 7 - 7.1.4",
];

const fn ambisonics_layout<const TARGETS: usize>(
    index: usize,
    names: &[&'static str],
) -> AudioIOLayout {
    let order = index / TARGETS + 1;
    let target = index % TARGETS;
    AudioIOLayout {
        main_input_channels: Some(new_nonzero_u32(((order + 1) * (order + 1)) as u32)),
        main_output_channels: Some(new_nonzero_u32(AMBISONICS_OUTPUT_WIDTHS[target])),
        aux_input_ports: &[],
        aux_output_ports: &[],
        names: PortNames {
            layout: Some(names[index]),
            main_input: Some("Ambisonics ACN/SN3D"),
            main_output: Some("Decoded Speakers"),
            ..PortNames::const_default()
        },
    }
}

const fn make_ambisonics_layouts<const N: usize, const TARGETS: usize>(
    names: &[&'static str],
) -> [AudioIOLayout; N] {
    let mut layouts = [AudioIOLayout::const_default(); N];
    let mut index = 0;
    while index < N {
        layouts[index] = ambisonics_layout::<TARGETS>(index, names);
        index += 1;
    }
    layouts
}

#[doc(hidden)]
pub static AMBISONICS_VST3_LAYOUTS: [AudioIOLayout; 56] =
    make_ambisonics_layouts::<56, 8>(&AMBISONICS_LAYOUT_NAMES);
#[doc(hidden)]
pub static AMBISONICS_CLAP_LAYOUTS: [AudioIOLayout; 42] =
    make_ambisonics_layouts::<42, 6>(&CLAP_AMBISONICS_LAYOUT_NAMES);

#[doc(hidden)]
pub fn ambisonics_layout_index(layout: &AudioIOLayout, api: PluginApi) -> Option<usize> {
    let layouts: &[AudioIOLayout] = match api {
        PluginApi::Vst3 => &AMBISONICS_VST3_LAYOUTS,
        PluginApi::Clap => &AMBISONICS_CLAP_LAYOUTS,
        PluginApi::Standalone => &AMBISONICS_VST3_LAYOUTS,
    };
    layouts.iter().position(|candidate| candidate == layout)
}

#[doc(hidden)]
pub fn ambisonics_io_channels(order: usize, target_layout: usize) -> Option<(usize, usize)> {
    let target_channels = usize::try_from(*AMBISONICS_OUTPUT_WIDTHS.get(target_layout)?).ok()?;
    let components = order.checked_add(1)?.checked_mul(order.checked_add(1)?)?;
    (1..=7)
        .contains(&order)
        .then_some((components, target_channels))
}

#[doc(hidden)]
pub fn ambisonics_vst3_output_to_sotf(target_layout: usize, channel: usize) -> Option<usize> {
    let map: &[usize] = match target_layout {
        0 => &[0, 1, 2, 3, 4, 5],
        1 => &[0, 1, 2, 3, 6, 7, 4, 5],
        2 => &[0, 1, 2, 3, 4, 5, 6, 7],
        3 => &[0, 1, 2, 3, 6, 7, 4, 5, 8, 9],
        4 => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
        5 => &[0, 1, 2, 3, 6, 7, 4, 5, 8, 9, 10, 11],
        6 => &[0, 1, 2, 3, 6, 7, 4, 5, 10, 11, 12, 13, 8, 9],
        7 => &[0, 1, 2, 3, 6, 7, 4, 5, 10, 11, 12, 13, 14, 15, 8, 9],
        _ => return None,
    };
    map.get(channel).copied()
}

const CLAP_MAP_51: [u8; 6] = [0, 1, 2, 3, 9, 10];
const CLAP_MAP_71: [u8; 8] = [0, 1, 2, 3, 9, 10, 4, 5];
const CLAP_MAP_512: [u8; 8] = [0, 1, 2, 3, 9, 10, 12, 14];
const CLAP_MAP_712: [u8; 10] = [0, 1, 2, 3, 9, 10, 4, 5, 12, 14];
const CLAP_MAP_514: [u8; 10] = [0, 1, 2, 3, 9, 10, 12, 14, 15, 17];
const CLAP_MAP_714: [u8; 12] = [0, 1, 2, 3, 9, 10, 4, 5, 12, 14, 15, 17];

#[doc(hidden)]
pub fn ambisonics_clap_channel_map(target_layout: usize) -> Option<&'static [u8]> {
    Some(match target_layout {
        0 => &CLAP_MAP_51,
        1 => &CLAP_MAP_71,
        2 => &CLAP_MAP_512,
        3 => &CLAP_MAP_712,
        4 => &CLAP_MAP_514,
        5 => &CLAP_MAP_714,
        _ => return None,
    })
}

#[doc(hidden)]
pub fn ambisonics_clap_channel_mask_supported(channel_mask: u64) -> bool {
    (0..6).any(|target_layout| {
        ambisonics_clap_channel_map(target_layout).is_some_and(|channel_map| {
            channel_map
                .iter()
                .fold(0_u64, |mask, channel| mask | (1_u64 << channel))
                == channel_mask
        })
    })
}

#[doc(hidden)]
pub fn ambisonics_vst3_arrangement(layout_index: usize, is_input: bool) -> Option<u64> {
    let order = layout_index / 8 + 1;
    let target_layout = layout_index % 8;
    if !(1..=7).contains(&order) {
        return None;
    }
    if !is_input {
        return Some(match target_layout {
            0 => 0x0000_0000_0000_003f, // 5.1
            1 => 0x0000_0000_0000_063f, // 7.1 Music
            2 => 0x0000_0000_0000_503f, // 5.1.2
            3 => 0x0000_0000_0000_563f, // 7.1.2 front heights
            4 => 0x0000_0000_0002_d03f, // 5.1.4
            5 => 0x0000_0000_0002_d63f, // 7.1.4
            6 => 0x1800_0000_0002_d63f, // 9.1.4 wide
            7 => 0x1800_0000_0302_d63f, // 9.1.6 wide
            _ => return None,
        });
    }

    let channel_count = (order + 1) * (order + 1);
    let arrangement = if order <= 4 {
        let low_acn_channels = channel_count.min(4);
        let mut mask = ((1_u64 << low_acn_channels) - 1) << 20;
        let mut acn = 4;
        while acn < channel_count {
            mask |= 1_u64 << (38 + acn - 4);
            acn += 1;
        }
        mask
    } else if channel_count == 64 {
        u64::MAX
    } else {
        (1_u64 << channel_count) - 1
    };
    Some(arrangement)
}

#[doc(hidden)]
pub mod transport;

#[doc(hidden)]
#[macro_export]
macro_rules! sotf_nih_sample_accurate {
    ("Gain") => {
        true
    };
    ("Gate") => {
        true
    };
    ("EQ") => {
        true
    };
    ("LinearPhaseEQ") => {
        true
    };
    ($other:literal) => {
        false
    };
}

/// DSP channel counts for the fixed layouts exported by the NIH binaries.
#[doc(hidden)]
#[macro_export]
macro_rules! sotf_nih_io_channels {
    ("MonoToStereo", $channels:literal) => {
        (1usize, 2usize)
    };
    ("AmbisonicsDecoder", $channels:literal) => {
        (4usize, 6usize)
    };
    ("Upmixer", $channels:literal) => {
        (2usize, 6usize)
    };
    ("AAE", $channels:literal) => {
        (2usize, 6usize)
    };
    ("BandSplit", $channels:literal) => {
        (2usize, 4usize)
    };
    ("BandMerge", $channels:literal) => {
        (4usize, 2usize)
    };
    ("AEC", $channels:literal) => {
        (2usize, 1usize)
    };
    ("Beamformer", $channels:literal) => {
        (2usize, 1usize)
    };
    ($other:literal, $channels:literal) => {
        ($channels as usize, $channels as usize)
    };
}

/// Default DSP input/output counts for the packaged plugin layouts.
pub fn plugin_io_channels(plugin_type: &str) -> (usize, usize) {
    match plugin_type {
        "MonoToStereo" => (1, 2),
        "AmbisonicsDecoder" => (4, 6),
        "Upmixer" | "AAE" => (2, 6),
        "BandSplit" => (2, 4),
        "BandMerge" => (4, 2),
        "AEC" | "Beamformer" => (2, 1),
        _ => (2, 2),
    }
}

const fn band_split_clap_layout(input_channels: u32, output_channels: u32) -> AudioIOLayout {
    AudioIOLayout {
        main_input_channels: Some(new_nonzero_u32(input_channels)),
        main_output_channels: Some(new_nonzero_u32(output_channels)),
        aux_input_ports: &[],
        aux_output_ports: &[],
        names: PortNames::const_default(),
    }
}

/// CLAP represents BandSplit's outputs as indexed band-major channels. These
/// layouts deliberately omit speaker/surround metadata: output channels are
/// separate frequency bands, not speaker positions.
#[doc(hidden)]
pub static BAND_SPLIT_CLAP_LAYOUTS: [AudioIOLayout; 3] = [
    band_split_clap_layout(2, 4),
    AudioIOLayout {
        names: PortNames {
            layout: Some("BandSplit 3-band indexed output"),
            main_output: Some("Band-major channels"),
            ..PortNames::const_default()
        },
        ..band_split_clap_layout(2, 6)
    },
    AudioIOLayout {
        names: PortNames {
            layout: Some("BandSplit 4-band indexed output"),
            main_output: Some("Band-major channels"),
            ..PortNames::const_default()
        },
        ..band_split_clap_layout(2, 8)
    },
];

const BAND_SPLIT_AUX_OUTPUTS_4: [std::num::NonZeroU32; 3] =
    [new_nonzero_u32(2), new_nonzero_u32(2), new_nonzero_u32(2)];

/// The old VST3 configuration exposed two stereo bands packed into a single
/// four-channel main bus. Keep it as a migration layout for sessions that
/// explicitly restore that old arrangement.
const BAND_SPLIT_VST3_LEGACY_PACKED: AudioIOLayout = AudioIOLayout {
    main_input_channels: Some(new_nonzero_u32(2)),
    main_output_channels: Some(new_nonzero_u32(4)),
    aux_input_ports: &[],
    aux_output_ports: &BAND_SPLIT_AUX_OUTPUTS_4,
    names: PortNames {
        layout: Some("BandSplit legacy packed two-band output"),
        main_input: Some("Stereo Input"),
        main_output: Some("Bands 1+2 (legacy packed)"),
        aux_inputs: &[],
        aux_outputs: &["Band 3", "Band 4", "Legacy compatibility slot"],
    },
};

/// Fresh VST3 instances discover a fixed maximum of four stereo output buses.
/// Bus activation selects how many bands the DSP computes; each bus independently
/// determines whether the host consumes that band's stereo pair.
const BAND_SPLIT_VST3_MAX_BUSES: AudioIOLayout = AudioIOLayout {
    main_input_channels: Some(new_nonzero_u32(2)),
    main_output_channels: Some(new_nonzero_u32(2)),
    aux_input_ports: &[],
    aux_output_ports: &BAND_SPLIT_AUX_OUTPUTS_4,
    names: PortNames {
        layout: Some("BandSplit four stereo band buses"),
        main_input: Some("Stereo Input"),
        main_output: Some("Band 1"),
        aux_outputs: &["Band 2", "Band 3", "Band 4"],
        ..PortNames::const_default()
    },
};

#[doc(hidden)]
pub static BAND_SPLIT_VST3_LAYOUTS: [AudioIOLayout; 2] =
    [BAND_SPLIT_VST3_LEGACY_PACKED, BAND_SPLIT_VST3_MAX_BUSES];

const CROSSOVER_VST3_AUX_OUTPUTS: [std::num::NonZeroU32; 3] =
    [new_nonzero_u32(2), new_nonzero_u32(2), new_nonzero_u32(2)];
const CROSSOVER_VST3_AUX_OUTPUT_NAMES: [&str; 3] = ["Band 2", "Band 3", "Band 4"];

const fn crossover_vst3_single_output(
    layout_name: &'static str,
    output_name: &'static str,
) -> AudioIOLayout {
    AudioIOLayout {
        main_input_channels: Some(new_nonzero_u32(2)),
        main_output_channels: Some(new_nonzero_u32(2)),
        aux_input_ports: &[],
        aux_output_ports: &[],
        names: PortNames {
            layout: Some(layout_name),
            main_input: Some("Stereo Input"),
            main_output: Some(output_name),
            ..PortNames::const_default()
        },
    }
}

const fn crossover_vst3_both_outputs() -> AudioIOLayout {
    AudioIOLayout {
        main_input_channels: Some(new_nonzero_u32(2)),
        main_output_channels: Some(new_nonzero_u32(2)),
        aux_input_ports: &[],
        aux_output_ports: &CROSSOVER_VST3_AUX_OUTPUTS,
        names: PortNames {
            layout: Some("Crossover both bands"),
            main_input: Some("Stereo Input"),
            main_output: Some("Band 1"),
            aux_outputs: &CROSSOVER_VST3_AUX_OUTPUT_NAMES,
            ..PortNames::const_default()
        },
    }
}

#[doc(hidden)]
pub static CROSSOVER_VST3_LAYOUTS: [AudioIOLayout; 4] = [
    crossover_vst3_single_output("Crossover lowpass", "Low Only"),
    crossover_vst3_single_output("Crossover highpass", "High Only"),
    crossover_vst3_single_output("Crossover per-channel", "Per-channel"),
    crossover_vst3_both_outputs(),
];

const fn crossover_clap_layout(output_channels: u32, layout_name: &'static str) -> AudioIOLayout {
    AudioIOLayout {
        main_input_channels: Some(new_nonzero_u32(2)),
        main_output_channels: Some(new_nonzero_u32(output_channels)),
        aux_input_ports: &[],
        aux_output_ports: &[],
        names: PortNames {
            layout: Some(layout_name),
            main_input: Some("Stereo Input"),
            main_output: Some(if output_channels == 2 {
                "Stereo Output"
            } else {
                "Band-major indexed output"
            }),
            ..PortNames::const_default()
        },
    }
}

#[doc(hidden)]
pub static CROSSOVER_CLAP_LAYOUTS: [AudioIOLayout; 4] = [
    crossover_clap_layout(2, "Crossover stereo output"),
    crossover_clap_layout(4, "Crossover 2-band indexed output"),
    crossover_clap_layout(6, "Crossover 3-band indexed output"),
    crossover_clap_layout(8, "Crossover 4-band indexed output"),
];

#[doc(hidden)]
pub fn crossover_clap_output_channels(layout: &AudioIOLayout) -> Option<usize> {
    CROSSOVER_CLAP_LAYOUTS
        .iter()
        .position(|candidate| candidate == layout)
        .map(|index| if index == 0 { 2 } else { (index + 1) * 2 })
}

#[doc(hidden)]
pub fn crossover_vst3_both_band_count(active_buses: u64) -> Option<usize> {
    (2..=4).find(|bands| active_buses == (1_u64 << bands) - 1)
}

#[doc(hidden)]
pub fn crossover_vst3_bus_arrangement(
    layout_index: usize,
    is_input: bool,
    bus_index: usize,
) -> Option<u64> {
    if !(0..CROSSOVER_VST3_LAYOUTS.len()).contains(&layout_index) {
        return None;
    }
    if is_input {
        return (bus_index == 0).then_some(band_split_vst3_stereo_arrangement());
    }
    match (layout_index, bus_index) {
        (0..=2, 0) | (3, 0..=3) => Some(band_split_vst3_stereo_arrangement()),
        _ => None,
    }
}

#[doc(hidden)]
pub fn band_split_num_bands_for_clap_layout(layout: &AudioIOLayout) -> Option<usize> {
    BAND_SPLIT_CLAP_LAYOUTS
        .iter()
        .position(|candidate| candidate == layout)
        .map(|index| index + 2)
}

#[doc(hidden)]
pub fn band_split_num_bands_for_vst3_layout(layout: &AudioIOLayout) -> Option<usize> {
    (layout == &BAND_SPLIT_VST3_LEGACY_PACKED).then_some(2)
}

#[doc(hidden)]
pub fn band_split_vst3_is_max_bus_layout(layout: &AudioIOLayout) -> bool {
    layout == &BAND_SPLIT_VST3_MAX_BUSES
}

#[doc(hidden)]
pub fn band_split_vst3_is_legacy_packed_layout(layout: &AudioIOLayout) -> bool {
    layout == &BAND_SPLIT_VST3_LEGACY_PACKED
}

#[doc(hidden)]
pub fn band_split_vst3_num_bands_from_active_buses(
    layout: &AudioIOLayout,
    active_buses: u64,
) -> Option<usize> {
    if band_split_vst3_is_legacy_packed_layout(layout) {
        Some(if active_buses & 0b0100 != 0 {
            4
        } else if active_buses & 0b0010 != 0 {
            3
        } else {
            2
        })
    } else if band_split_vst3_is_max_bus_layout(layout) {
        let highest_active_bus = (0..4)
            .rev()
            .find(|bus| active_buses & (1_u64 << bus) != 0)
            .unwrap_or(1);
        Some((highest_active_bus + 1).clamp(2, 4))
    } else {
        None
    }
}

#[doc(hidden)]
pub fn band_split_vst3_active_buses_support_band_count(
    layout: &AudioIOLayout,
    active_buses: u64,
    num_bands: usize,
) -> bool {
    if active_buses & 0b0001 == 0 || active_buses & !0b1111 != 0 {
        return false;
    }

    if band_split_vst3_is_legacy_packed_layout(layout) {
        (active_buses & 0b0010 == 0 || num_bands >= 3)
            && (active_buses & 0b0100 == 0 || num_bands >= 4)
    } else if band_split_vst3_is_max_bus_layout(layout) {
        (active_buses & 0b0010 == 0 || num_bands >= 2)
            && (active_buses & 0b0100 == 0 || num_bands >= 3)
            && (active_buses & 0b1000 == 0 || num_bands >= 4)
    } else {
        false
    }
}

#[doc(hidden)]
pub fn band_split_vst3_is_legacy_reserved_bus(bus_index: usize) -> bool {
    bus_index == 3
}

#[doc(hidden)]
pub fn band_split_vst3_output_channel_offset(
    legacy_packed: bool,
    bus_index: usize,
) -> Option<usize> {
    if legacy_packed {
        match bus_index {
            1 => Some(4),
            2 => Some(6),
            _ => None,
        }
    } else if (1..=3).contains(&bus_index) {
        Some(bus_index * 2)
    } else {
        None
    }
}

#[doc(hidden)]
pub fn band_split_vst3_stereo_arrangement() -> u64 {
    // VST3 speaker bits: front-left bit 0, front-right bit 1.
    0b11
}

#[doc(hidden)]
pub fn band_split_vst3_legacy_packed_arrangement() -> u64 {
    // The historical main bus carried Band 1 and Band 2 as four indexed channels.
    0b1111
}

#[doc(hidden)]
pub fn band_split_vst3_bus_arrangement(
    layout_index: usize,
    is_input: bool,
    bus_index: usize,
) -> Option<u64> {
    match (layout_index, is_input, bus_index) {
        (0, true, 0) | (1, true, 0) => Some(band_split_vst3_stereo_arrangement()),
        (0, false, 0) => Some(band_split_vst3_legacy_packed_arrangement()),
        (0, false, 1..=3) | (1, false, 0..=3) => Some(band_split_vst3_stereo_arrangement()),
        _ => None,
    }
}

/// Migrate legacy NIH states while keeping BandSplit's structural fields
/// consistent with the selected CLAP layout.
#[doc(hidden)]
pub fn migrate_band_split_state(state: &mut nih_plug::wrapper::state::PluginState) {
    let has_band_split_state = state.params.keys().any(|id| {
        matches!(
            id.as_str(),
            "frequency" | "type" | "crossover_type" | "frequency_2" | "frequency_3" | "num_bands"
        )
    });
    if !has_band_split_state {
        return;
    }

    // The old two-band DSP used the legacy cascade. Preserve that behavior for
    // presets written before the recombination choice existed.
    state
        .params
        .entry("recombination_mode".to_string())
        .or_insert(nih_plug::wrapper::state::ParamValue::I32(0));

    // Before structural band count was serialized, every BandSplit state was
    // a two-band state. Do not inherit a wider count from the instance that
    // receives that old preset.
    state
        .params
        .entry("num_bands".to_string())
        .or_insert(nih_plug::wrapper::state::ParamValue::I32(0));
    state.fields.insert(
        crate::params::BAND_SPLIT_LAYOUT_RESTORE_MARKER.to_string(),
        "1".to_string(),
    );
}

/// Migrate complete Crossover NIH states while preserving the legacy
/// frequency-only preset contract. Newly introduced controls receive the old
/// native route defaults so restoring over a populated instance cannot retain
/// its newer family, topology, mode, or band count.
#[doc(hidden)]
pub fn migrate_crossover_state(state: &mut nih_plug::wrapper::state::PluginState) {
    use nih_plug::wrapper::state::ParamValue;

    let has_crossover_state = state.params.keys().any(|id| {
        matches!(
            id.as_str(),
            "frequency"
                | "family"
                | "mode"
                | "fir_taps"
                | "topology"
                | "band_count"
                | "frequency_2"
                | "frequency_3"
                | "channel_frequency_0"
                | "channel_mode_0"
                | "channel_frequency_1"
                | "channel_mode_1"
        )
    });
    if !has_crossover_state {
        return;
    }

    // Before AUD142 the only native control was `frequency`. Those presets
    // represented LR24, bands topology, two bands, and lowpass output.
    for (id, value) in [
        ("family", ParamValue::I32(0)),
        ("mode", ParamValue::I32(0)),
        ("fir_taps", ParamValue::I32(255)),
        ("topology", ParamValue::I32(0)),
        ("band_count", ParamValue::I32(0)),
        ("frequency_2", ParamValue::F32(3000.0)),
        ("frequency_3", ParamValue::F32(8000.0)),
        ("channel_frequency_0", ParamValue::F32(1000.0)),
        ("channel_mode_0", ParamValue::I32(0)),
        ("channel_frequency_1", ParamValue::F32(3000.0)),
        ("channel_mode_1", ParamValue::I32(1)),
    ] {
        state.params.entry(id.to_string()).or_insert(value);
    }
    state.fields.insert(
        crate::params::CROSSOVER_STATE_RESTORE_MARKER.to_string(),
        "1".to_string(),
    );
}

/// Append Gate's key bus without renumbering existing CLAP configurations.
#[doc(hidden)]
#[macro_export]
macro_rules! sotf_nih_layouts {
    ("AmbisonicsDecoder", $default:expr) => {
        &$crate::wrapper::AMBISONICS_VST3_LAYOUTS
    };
    ("BandSplit", $default:expr) => {
        &$crate::wrapper::BAND_SPLIT_VST3_LAYOUTS
    };
    ("Gate", $default:expr) => {{
        const DEFAULT: nih_plug::prelude::AudioIOLayout = $default;
        &[
            DEFAULT,
            nih_plug::prelude::AudioIOLayout {
                aux_input_ports: &[nih_plug::audio_setup::new_nonzero_u32(2)],
                names: nih_plug::audio_setup::PortNames {
                    layout: Some("Stereo + Sidechain"),
                    aux_inputs: &["Sidechain"],
                    ..DEFAULT.names
                },
                ..DEFAULT
            },
        ]
    }};
    ($other:literal, $default:expr) => {
        &[$default]
    };
}

/// Compile-time specialization for wrapper features that are only meaningful
/// for the Ambisonics decoder. This literal-token match is needed in
/// associated constants.
#[doc(hidden)]
#[macro_export]
macro_rules! sotf_nih_is_ambisonics {
    ("AmbisonicsDecoder") => {
        true
    };
    ($other:literal) => {
        false
    };
}

/// Factory channel argument. BandMerge historically takes the output width.
pub fn plugin_constructor_channels(plugin_type: &str) -> usize {
    match plugin_type {
        "MonoToStereo" => 1,
        "AmbisonicsDecoder" => 4,
        _ => 2,
    }
}

/// Generate a complete nih-plug plugin struct from SOTF plugin metadata.
///
/// This macro creates a struct that:
/// - Implements `nih_plug::Plugin`, `Vst3Plugin`, and `ClapPlugin`
/// - Creates the SOTF plugin via `plugins_bridge::create_plugin()`
/// - Handles interleave/deinterleave for buffer conversion
/// - Syncs nih-plug parameters to SOTF plugin parameters
#[macro_export]
macro_rules! sotf_nih_plugin {
    (
        $struct_name:ident,
        plugin_type: $plugin_type:tt,
        name: $name:literal,
        clap_id: $clap_id:literal,
        vst3_class_id: $vst3_id:expr,
        channels: $channels:literal
    ) => {
        pub struct $struct_name {
            params: std::sync::Arc<$crate::params::DynamicParams>,
            inner: Option<Box<dyn sotf_host::plugin::Plugin>>,
            bridge: $crate::PluginBridgeWrapper,
            interleaved_in: Vec<f32>,
            interleaved_out: Vec<f32>,
            max_frames: usize,
            main_input_channels: usize,
            main_output_channels: usize,
            aux_output_channels: [usize; 3],
            aux_output_count: usize,
            ambisonics_target_layout: usize,
            band_split_active_output_buses: u64,
            band_split_last_vst3_output_buses: Option<(usize, u64)>,
            band_split_vst3_legacy_packed: bool,
            sample_rate: u32,
            structural_fingerprint: u64,
            non_restartable_structural_fingerprint: u64,
            transport: $crate::wrapper::transport::TransportTracker,
        }

        impl Default for $struct_name {
            fn default() -> Self {
                let specs = $crate::wrapper::get_param_specs($plugin_type);
                let bridge_inner = plugins_bridge::param_bridge::ParamBridge::new(specs);

                // Build param infos for DynamicParams
                let mut infos = Vec::new();
                for i in 0..bridge_inner.count() {
                    if let Some(info) = bridge_inner.info(i) {
                        infos.push(info);
                    }
                }

                // If no ParamSpec params, or when LinearPhaseEQ needs its
                // dynamic band schema in addition to global specs, inspect a
                // temporary uninitialized instance on the control thread.
                if (infos.is_empty() || matches!($plugin_type, "LinearPhaseEQ" | "EQ"))
                    && let Ok(plugin) = plugins_bridge::create_plugin(
                        $plugin_type,
                        $crate::wrapper::plugin_constructor_channels($plugin_type),
                        48000,
                        &$crate::wrapper::default_plugin_config($plugin_type),
                    )
                {
                    for param in plugin.parameters() {
                        if infos.iter().any(|info| info.id == param.id.as_str()) {
                            continue;
                        }
                        if let Some(info) = $crate::wrapper::bridged_info_from_parameter(&param) {
                            infos.push(info);
                        }
                    }
                }

                // Expose pre-migration ids to DAW hosts; internal sync and
                // construction translate back to canonical keys.
                for info in &mut infos {
                    info.id = $crate::wrapper::legacy_external_param_id($plugin_type, &info.id)
                        .to_string();
                }

                Self {
                    params: $crate::params::DynamicParams::from_infos_for_plugin(
                        $plugin_type,
                        &infos,
                    ),
                    inner: None,
                    bridge: $crate::PluginBridgeWrapper::new(bridge_inner),
                    interleaved_in: Vec::new(),
                    interleaved_out: Vec::new(),
                    max_frames: 0,
                    main_input_channels: 0,
                    main_output_channels: 0,
                    aux_output_channels: [0; 3],
                    aux_output_count: 0,
                    ambisonics_target_layout: 0,
                    band_split_active_output_buses: 0,
                    band_split_last_vst3_output_buses: None,
                    band_split_vst3_legacy_packed: false,
                    sample_rate: 48000,
                    structural_fingerprint: 0,
                    non_restartable_structural_fingerprint: 0,
                    transport: $crate::wrapper::transport::TransportTracker::default(),
                }
            }
        }

        impl nih_plug::prelude::Plugin for $struct_name {
            const NAME: &'static str = $name;
            const VENDOR: &'static str = "SOTF / Spinorama";
            const URL: &'static str = "https://spinorama.org";
            const EMAIL: &'static str = "";
            const VERSION: &'static str = env!("CARGO_PKG_VERSION");
            // nih-plug splits process buffers at host automation boundaries
            // when this is enabled. The per-slice sync below therefore stamps
            // AsyncTimelinePlugin events at the exact absolute frame instead
            // of collapsing automation to the original callback start.
            const SAMPLE_ACCURATE_AUTOMATION: bool =
                $crate::sotf_nih_sample_accurate!($plugin_type);
            fn tail_length(&self) -> Option<u32> {
                Some($crate::wrapper::native_tail_samples(
                    self.inner.as_ref().map_or(sotf_host::TailLength::Unknown, |plugin| plugin.tail_length()),
                ))
            }
            const AUDIO_IO_LAYOUTS: &'static [nih_plug::prelude::AudioIOLayout] =
                $crate::sotf_nih_layouts!($plugin_type, nih_plug::prelude::AudioIOLayout {
                    main_input_channels: std::num::NonZeroU32::new({
                        let (inputs, outputs) =
                            $crate::sotf_nih_io_channels!($plugin_type, $channels);
                        if inputs < outputs {
                            inputs as u32
                        } else {
                            outputs as u32
                        }
                    }),
                    main_output_channels: std::num::NonZeroU32::new(
                        $crate::sotf_nih_io_channels!($plugin_type, $channels).1 as u32,
                    ),
                    // NIH's main Buffer contains only output channels. Put
                    // additional input channels on a separate bus so none are lost.
                    aux_input_ports: {
                        const IO: (usize, usize) =
                            $crate::sotf_nih_io_channels!($plugin_type, $channels);
                        if IO.0 > IO.1 {
                            &[nih_plug::audio_setup::new_nonzero_u32((IO.0 - IO.1) as u32)]
                        } else {
                            &[]
                        }
                    },
                    aux_output_ports: &[],
                    ..nih_plug::prelude::AudioIOLayout::const_default()
                });

            type SysExMessage = ();
            type BackgroundTask = ();

            fn params(&self) -> std::sync::Arc<dyn nih_plug::prelude::Params> {
                self.params.clone()
            }

            fn filter_state(state: &mut nih_plug::wrapper::state::PluginState) {
                if matches!($plugin_type, "BandSplit") {
                    $crate::wrapper::migrate_band_split_state(state);
                } else if matches!($plugin_type, "Crossover") {
                    $crate::wrapper::migrate_crossover_state(state);
                }
            }

            fn initialize(
                &mut self,
                audio_io_layout: &nih_plug::prelude::AudioIOLayout,
                buffer_config: &nih_plug::prelude::BufferConfig,
                context: &mut impl nih_plug::prelude::InitContext<Self>,
            ) -> bool {
                let mut band_split_vst3_output_buses = None;
                let mut band_split_vst3_layout_index = None;
                let mut band_split_vst3_legacy_packed = false;
                let band_split_num_bands = if matches!($plugin_type, "BandSplit") {
                    if context.plugin_api() == nih_plug::context::PluginApi::Clap {
                        $crate::wrapper::band_split_num_bands_for_clap_layout(audio_io_layout)
                    } else if context.plugin_api() == nih_plug::context::PluginApi::Vst3 {
                        let Some(layout_index) = Self::AUDIO_IO_LAYOUTS
                            .iter()
                            .position(|layout| layout == audio_io_layout)
                        else {
                            return false;
                        };
                        band_split_vst3_layout_index = Some(layout_index);
                        band_split_vst3_legacy_packed =
                            $crate::wrapper::band_split_vst3_is_legacy_packed_layout(
                                audio_io_layout,
                            );
                        let default_buses = if band_split_vst3_legacy_packed {
                            0b1
                        } else {
                            0b11
                        };
                        let active_buses = context
                            .vst3_active_audio_output_buses()
                            .unwrap_or(default_buses);
                        band_split_vst3_output_buses = Some(active_buses);
                        let unchanged_bus_selection = self
                            .band_split_last_vst3_output_buses
                            .is_some_and(|previous| previous == (layout_index, active_buses));
                        let imported_state = self.params.band_split_layout_restore_is_pending();
                        if imported_state || unchanged_bus_selection {
                            self.params.band_split_num_bands()
                        } else {
                            $crate::wrapper::band_split_vst3_num_bands_from_active_buses(
                                audio_io_layout,
                                active_buses,
                            )
                        }
                    } else if Self::AUDIO_IO_LAYOUTS.contains(audio_io_layout) {
                        Some(2)
                    } else {
                        None
                    }
                } else {
                    None
                };
                if !Self::AUDIO_IO_LAYOUTS.contains(audio_io_layout)
                    && band_split_num_bands.is_none()
                {
                    return false;
                }
                if let Some(num_bands) = band_split_num_bands
                    && self.params.select_band_split_layout(num_bands).is_err()
                {
                    return false;
                }
                if matches!($plugin_type, "AmbisonicsDecoder") {
                    let Some(layout_index) = $crate::wrapper::ambisonics_layout_index(
                        audio_io_layout,
                        context.plugin_api(),
                    ) else {
                        return false;
                    };
                    let target_count = if context.plugin_api()
                        == nih_plug::context::PluginApi::Vst3
                    {
                        8
                    } else {
                        6
                    };
                    let order = layout_index / target_count + 1;
                    let target_layout = layout_index % target_count;
                    if self
                        .params
                        .validate_restored_ambisonics_layout(order, target_layout)
                        .is_err()
                    {
                        return false;
                    }
                    if self
                        .params
                        .set_ambisonics_layout(order, target_layout)
                        .is_err()
                    {
                        return false;
                    }
                    self.ambisonics_target_layout = target_layout;
                }
                self.main_input_channels = audio_io_layout
                    .main_input_channels
                    .map_or(0, |channels| channels.get() as usize);
                let main_output_channels = audio_io_layout
                    .main_output_channels
                    .map_or(0, |channels| channels.get() as usize);
                if audio_io_layout.aux_output_ports.len() > self.aux_output_channels.len() {
                    return false;
                }
                let mut aux_output_channels = [0; 3];
                for (index, channels) in audio_io_layout.aux_output_ports.iter().enumerate() {
                    aux_output_channels[index] = channels.get() as usize;
                }
                let total_declared_outputs = main_output_channels
                    + aux_output_channels.iter().sum::<usize>();
                self.sample_rate = buffer_config.sample_rate as u32;
                let max_frames = buffer_config.max_buffer_size as usize;

                match $crate::params::configuration::create_plugin(
                    $plugin_type,
                    self.sample_rate,
                    &self.params,
                ) {
                    Ok(mut plugin) => {
                        let input_channels = plugin.input_channels();
                        let output_channels = plugin.output_channels();
                        let main_inputs = audio_io_layout.main_input_channels
                            .map_or(0, |channels| channels.get() as usize);
                        let total_inputs = main_inputs + audio_io_layout.aux_input_ports
                            .iter().map(|channels| channels.get() as usize).sum::<usize>();
                        let uses_optional_band_buses = matches!($plugin_type, "BandSplit")
                            && context.plugin_api() == nih_plug::context::PluginApi::Vst3
                            && ($crate::wrapper::band_split_vst3_is_max_bus_layout(
                                audio_io_layout,
                            ) || band_split_vst3_legacy_packed);
                        let output_width_matches = if uses_optional_band_buses {
                            band_split_num_bands
                                .is_some_and(|bands| {
                                    output_channels == bands * 2
                                        && output_channels
                                            >= audio_io_layout.main_output_channels.map_or(0, |v| v.get() as usize)
                                })
                                && output_channels <= total_declared_outputs
                        } else {
                            output_channels == total_declared_outputs
                        };
                        let active_band_buses_match = band_split_num_bands.is_some_and(|bands| {
                            $crate::wrapper::band_split_vst3_active_buses_support_band_count(
                                audio_io_layout,
                                band_split_vst3_output_buses.unwrap_or_default(),
                                bands,
                            )
                        });
                        // Gate's internal detector ignores the optional key bus.
                        // External detection requires that the host selected it.
                        let ignores_key_bus = matches!($plugin_type, "Gate")
                            && input_channels == main_inputs;
                        if (input_channels != total_inputs && !ignores_key_bus)
                            || !output_width_matches
                            || (uses_optional_band_buses && !active_band_buses_match)
                        {
                            log::error!(
                                "{} DSP channels do not match the declared host layout",
                                $plugin_type
                            );
                            return false;
                        }

                        plugin = match plugins_bridge::prepare_standalone_plugin(plugin, max_frames) {
                            Ok(plugin) => plugin,
                            Err(error) => {
                                log::error!("Failed to prepare {}: {error}", $plugin_type);
                                return false;
                            }
                        };
                        if matches!($plugin_type, "LinearPhaseEQ") {
                            plugin = match sotf_host::AsyncTimelinePlugin::new(
                                plugin,
                                self.sample_rate,
                                max_frames,
                            ) {
                                Ok(adapter) => Box::new(adapter),
                                Err(e) => {
                                    log::error!(
                                        "Failed to initialize {} adapter: {e}",
                                        $plugin_type
                                    );
                                    return false;
                                }
                            };
                        } else if let Err(e) = plugin.initialize(self.sample_rate) {
                            log::error!("Failed to initialize {}: {e}", $plugin_type);
                            return false;
                        }

                        // Validate the complete saved state on the control thread.
                        // Realtime values may depend on structural settings, such
                        // as the limiter requiring a fully wet mix in ISP mode.
                        if let Err(error) = self.params.sync_to_plugin(plugin.as_mut()) {
                            log::error!("Failed to restore {} parameters: {error}", $plugin_type);
                            return false;
                        }

                        let latency = match u32::try_from(plugin.latency_samples()) {
                            Ok(latency) => latency,
                            Err(_) => {
                                log::error!("{} latency does not fit the host ABI", $plugin_type);
                                return false;
                            }
                        };
                        context.set_latency_samples(latency);

                        self.interleaved_in = vec![0.0; max_frames * input_channels];
                        self.interleaved_out = vec![0.0; max_frames * output_channels];
                        self.max_frames = max_frames;
                        self.main_output_channels = main_output_channels;
                        self.aux_output_channels = aux_output_channels;
                        self.aux_output_count = audio_io_layout.aux_output_ports.len();
                        if let Some(active_buses) = band_split_vst3_output_buses {
                            self.band_split_active_output_buses = active_buses;
                            self.band_split_last_vst3_output_buses = Some((
                                band_split_vst3_layout_index.unwrap_or_default(),
                                active_buses,
                            ));
                        }
                        self.band_split_vst3_legacy_packed = band_split_vst3_legacy_packed;
                        self.structural_fingerprint = self.params.structural_fingerprint();
                        self.non_restartable_structural_fingerprint = self
                            .params
                            .non_restartable_structural_fingerprint();
                        self.inner = Some(plugin);
                        if band_split_num_bands.is_some() {
                            self.params.complete_band_split_layout_restore();
                        }
                        self.transport = $crate::wrapper::transport::TransportTracker::default();
                        true
                    }
                    Err(e) => {
                        log::error!("Failed to create {}: {e}", $plugin_type);
                        false
                    }
                }
            }

            fn process(
                &mut self,
                buffer: &mut nih_plug::prelude::Buffer,
                aux: &mut nih_plug::prelude::AuxiliaryBuffers,
                context: &mut impl nih_plug::prelude::ProcessContext<Self>,
            ) -> nih_plug::prelude::ProcessStatus {
                self.process_with_api(
                    buffer,
                    aux,
                    context.plugin_api(),
                    context.transport().into(),
                )
            }

            fn reset(&mut self) {
                self.transport = $crate::wrapper::transport::TransportTracker::default();
                if let Some(plugin) = self.inner.as_mut() {
                    plugin.reset();
                }
            }
        }

        impl $struct_name {
            // Only the shared unit tests need the transport-only entry point.
            #[cfg(test)]
            #[allow(dead_code)]
            fn process_with_transport(
                &mut self,
                buffer: &mut nih_plug::prelude::Buffer,
                aux: &mut nih_plug::prelude::AuxiliaryBuffers,
                transport: $crate::wrapper::transport::NativeTransport,
            ) -> nih_plug::prelude::ProcessStatus {
                self.process_with_api(
                    buffer,
                    aux,
                    nih_plug::context::PluginApi::Clap,
                    transport,
                )
            }

            fn process_with_api(
                &mut self,
                buffer: &mut nih_plug::prelude::Buffer,
                aux: &mut nih_plug::prelude::AuxiliaryBuffers,
                plugin_api: nih_plug::context::PluginApi,
                transport: $crate::wrapper::transport::NativeTransport,
            ) -> nih_plug::prelude::ProcessStatus {
                let plugin = match self.inner.as_mut() {
                    Some(p) => p,
                    None => return nih_plug::prelude::ProcessStatus::Error("Not initialized"),
                };

                let num_frames = buffer.samples();
                let num_channels = buffer.channels();
                let input_channels = plugin.input_channels();
                let output_channels = plugin.output_channels();
                let ambisonics = matches!($plugin_type, "AmbisonicsDecoder");
                let expected_channels = self.main_input_channels.max(self.main_output_channels);

                // A misbehaving host may hand us a block larger than the
                // negotiated `max_buffer_size` or with an unexpected channel
                // count. The scratch vectors are sized from `initialize()`,
                // so reject the block with silence instead of indexing them
                // out of bounds (mirrors the FFI crate's BufferTooSmall path).
                let _buffer_sample_count = match $crate::wrapper::check_host_block(
                    num_frames,
                    num_channels,
                    self.max_frames,
                    expected_channels,
                ) {
                    Some(needed) => needed,
                    None => {
                        $crate::wrapper::silence_host_outputs(buffer, aux);
                        return nih_plug::prelude::ProcessStatus::Error(
                            "Host block exceeds the negotiated maximum",
                        );
                    }
                };
                let input_sample_count = num_frames
                    .checked_mul(input_channels)
                    .filter(|samples| *samples <= self.interleaved_in.len());
                let output_sample_count = num_frames
                    .checked_mul(output_channels)
                    .filter(|samples| *samples <= self.interleaved_out.len());
                let optional_band_buses = matches!($plugin_type, "BandSplit")
                    && plugin_api == nih_plug::context::PluginApi::Vst3
                    && self.aux_output_count == 3;
                let invalid_aux_output = aux.outputs.len() != self.aux_output_count
                    || aux.outputs.iter().enumerate().any(|(index, output_bus)| {
                        if optional_band_buses {
                            let slices = output_bus.as_slice_immutable();
                            slices.len() != self.aux_output_channels[index]
                                || !(slices.iter().all(|slice| slice.len() == num_frames)
                                    || slices.iter().all(|slice| slice.is_empty()))
                        } else {
                            output_bus.channels() != self.aux_output_channels[index]
                                || output_bus.samples() != num_frames
                        }
                    })
                    || (optional_band_buses
                        && (self.band_split_active_output_buses & 1 == 0
                            || buffer
                                .as_slice_immutable()
                                .iter()
                                .take(self.main_output_channels)
                                .any(|slice| slice.len() != num_frames)
                            || aux.outputs.iter().enumerate().any(|(index, output_bus)| {
                                let bus_index = index + 1;
                                let active =
                                    self.band_split_active_output_buses & (1_u64 << bus_index) != 0;
                                active
                                    && (output_bus
                                        .as_slice_immutable()
                                        .iter()
                                        .any(|slice| slice.len() != num_frames)
                                        || $crate::wrapper::band_split_vst3_output_channel_offset(
                                            self.band_split_vst3_legacy_packed,
                                            bus_index,
                                        )
                                        .is_some_and(|offset| offset + self.aux_output_channels[index] > output_channels))
                            })));
                if input_sample_count.is_none()
                    || output_sample_count.is_none()
                    || invalid_aux_output
                    || (ambisonics && !buffer.has_all_main_input_channels())
                    || (!ambisonics && input_channels > output_channels
                        && (aux.inputs.len() != 1
                            || aux.inputs[0].channels() != input_channels - output_channels
                            || aux.inputs[0].samples() != num_frames))
                {
                    $crate::wrapper::silence_host_outputs(buffer, aux);
                    return nih_plug::prelude::ProcessStatus::Error(
                        "Host block does not match the negotiated bus geometry",
                    );
                }

                let structural_state_changed =
                    self.params.structural_fingerprint() != self.structural_fingerprint;
                if structural_state_changed {
                    let non_restartable_state_changed =
                        !matches!($plugin_type, "DynamicEQ")
                            || self.params.non_restartable_structural_fingerprint()
                                != self.non_restartable_structural_fingerprint;
                    if non_restartable_state_changed {
                        // Construction-sized state is reconstructed by initialize().
                        // Only DynamicEQ's visible, non-automatable shelf controls may
                        // change while the old prepared instance continues processing;
                        // all other structural edits still fail rather than rebuilding
                        // or destroying plugin resources on the render thread.
                        $crate::wrapper::silence_host_outputs(buffer, aux);
                        return nih_plug::prelude::ProcessStatus::Error(
                            "Structural parameter state changed; reactivate plugin",
                        );
                    }
                }

                // Sync nih-plug params → SOTF plugin
                if self
                    .bridge
                    .sync_params_to_plugin(&self.params, plugin.as_mut())
                    .is_err()
                {
                    $crate::wrapper::silence_host_outputs(buffer, aux);
                    return nih_plug::prelude::ProcessStatus::Error("Parameter update failed");
                }

                // Interleave main inputs and any additional input bus.
                let channel_slices = buffer.as_slice();
                for frame in 0..num_frames {
                    if ambisonics {
                        for ch in 0..input_channels {
                            self.interleaved_in[frame * input_channels + ch] =
                                channel_slices[ch][frame];
                        }
                    } else {
                        for ch in 0..input_channels.min(output_channels) {
                            self.interleaved_in[frame * input_channels + ch] =
                                channel_slices[ch][frame];
                        }
                    }
                    if !ambisonics && input_channels > output_channels {
                        for ch in 0..input_channels - output_channels {
                            self.interleaved_in[frame * input_channels + output_channels + ch] =
                                aux.inputs[0].as_slice_immutable()[ch][frame];
                        }
                    }
                }

                // Process
                let ctx = self.transport.context(transport, self.sample_rate, num_frames);
                let produced = plugin.process(
                    &self.interleaved_in[..num_frames * input_channels],
                    &mut self.interleaved_out[..output_sample_count.unwrap()],
                    &ctx,
                );
                if !matches!(produced, Ok(frames) if frames == num_frames) {
                    $crate::wrapper::silence_host_outputs(buffer, aux);
                    return nih_plug::prelude::ProcessStatus::Error("Processing failed or returned an incomplete block");
                }

                // Deinterleave the packed DSP output into the negotiated main
                // bus followed by each auxiliary output bus.
                let channel_slices = buffer.as_slice();
                for frame in 0..num_frames {
                    for ch in 0..self.main_output_channels {
                        let source_channel = if ambisonics
                            && plugin_api == nih_plug::context::PluginApi::Vst3
                        {
                            match $crate::wrapper::ambisonics_vst3_output_to_sotf(
                                self.ambisonics_target_layout,
                                ch,
                            ) {
                                Some(source_channel) => source_channel,
                                None => {
                                    for channel in channel_slices.iter_mut() {
                                        channel.fill(0.0);
                                    }
                                    for output_bus in aux.outputs.iter_mut() {
                                        for channel in output_bus.as_slice() {
                                            channel.fill(0.0);
                                        }
                                    }
                                    return nih_plug::prelude::ProcessStatus::Error(
                                        "Unsupported Ambisonics output mapping",
                                    );
                                }
                            }
                        } else {
                            ch
                        };
                        channel_slices[ch][frame] =
                            self.interleaved_out[frame * output_channels + source_channel];
                    }
                }
                let mut output_channel_offset = self.main_output_channels;
                for (bus_index, output_bus) in aux.outputs.iter_mut().enumerate() {
                    let bus_channels = self.aux_output_channels[bus_index];
                    let bus_index_vst = bus_index + 1;
                    if optional_band_buses {
                        let active = self.band_split_active_output_buses & (1_u64 << bus_index_vst)
                            != 0;
                        if !active {
                            continue;
                        }
                        let bus_slices = output_bus.as_slice();
                        if let Some(source_offset) =
                            $crate::wrapper::band_split_vst3_output_channel_offset(
                                self.band_split_vst3_legacy_packed,
                                bus_index_vst,
                            )
                        {
                            for frame in 0..num_frames {
                                for channel in 0..bus_channels {
                                    bus_slices[channel][frame] = self.interleaved_out
                                        [frame * output_channels + source_offset + channel];
                                }
                            }
                        } else {
                            // The final legacy bus preserves the old host bus count but has no
                            // corresponding BandSplit output. It is valid, stable, and silent.
                            for channel in bus_slices.iter_mut() {
                                channel.fill(0.0);
                            }
                        }
                    } else {
                        let bus_slices = output_bus.as_slice();
                        for frame in 0..num_frames {
                            for channel in 0..bus_channels {
                                bus_slices[channel][frame] = self.interleaved_out
                                    [frame * output_channels + output_channel_offset + channel];
                            }
                        }
                    }
                    output_channel_offset += bus_channels;
                }
                if !optional_band_buses && output_channel_offset != output_channels {
                    for channel in channel_slices.iter_mut() {
                        channel.fill(0.0);
                    }
                    for output_bus in aux.outputs.iter_mut() {
                        for channel in output_bus.as_slice() {
                            channel.fill(0.0);
                        }
                    }
                    return nih_plug::prelude::ProcessStatus::Error(
                        "Negotiated output buses do not cover the plugin output",
                    );
                }

                $crate::wrapper::process_status_for_tail(plugin.tail_length())
            }
        }

        impl nih_plug::prelude::Vst3Plugin for $struct_name {
            const VST3_CLASS_ID: [u8; 16] = $vst3_id;
            const VST3_SUBCATEGORIES: &'static [nih_plug::prelude::Vst3SubCategory] =
                &[nih_plug::prelude::Vst3SubCategory::Fx];

            fn vst3_restart_component_on_required_parameter_change() -> bool {
                matches!($plugin_type, "DynamicEQ")
            }

            fn default_audio_io_layout() -> nih_plug::prelude::AudioIOLayout {
                // VST3 discovers buses from its initial layout. CLAP retains
                // its legacy stereo config ID and explicitly selects keys.
                let layouts = <Self as nih_plug::prelude::Plugin>::AUDIO_IO_LAYOUTS;
                if matches!($plugin_type, "BandSplit") {
                    layouts[1]
                } else {
                    layouts[if matches!($plugin_type, "Gate") { 1 } else { 0 }]
                }
            }

            fn vst3_bus_arrangement(
                layout_index: usize,
                is_input: bool,
                bus_index: usize,
            ) -> Option<u64> {
                if matches!($plugin_type, "AmbisonicsDecoder") && bus_index == 0 {
                    $crate::wrapper::ambisonics_vst3_arrangement(layout_index, is_input)
                } else if matches!($plugin_type, "BandSplit") {
                    $crate::wrapper::band_split_vst3_bus_arrangement(
                        layout_index,
                        is_input,
                        bus_index,
                    )
                } else {
                    None
                }
            }

            fn vst3_audio_bus_default_active(is_input: bool, bus_index: usize) -> bool {
                !matches!($plugin_type, "BandSplit") || is_input || bus_index < 2
            }

            fn vst3_compatibility_layout_index(
                input_arrangements: &[u64],
                output_arrangements: &[u64],
            ) -> Option<usize> {
                if matches!($plugin_type, "BandSplit")
                    && input_arrangements == [$crate::wrapper::band_split_vst3_stereo_arrangement()]
                    && output_arrangements
                        == [$crate::wrapper::band_split_vst3_legacy_packed_arrangement()]
                {
                    Some(0)
                } else {
                    None
                }
            }

            fn vst3_audio_bus_default_active_for_layout(
                layout_index: usize,
                is_input: bool,
                bus_index: usize,
            ) -> bool {
                if matches!($plugin_type, "BandSplit") {
                    if is_input {
                        return layout_index <= 1 && bus_index == 0;
                    }
                    match (layout_index, bus_index) {
                        (0, 0) => true,
                        (1, 0 | 1) => true,
                        _ => false,
                    }
                } else {
                    Self::vst3_audio_bus_default_active(is_input, bus_index)
                }
            }

            fn vst3_allows_inactive_audio_output_buses() -> bool {
                matches!($plugin_type, "BandSplit")
            }

        }

        impl nih_plug::prelude::ClapPlugin for $struct_name {
            const CLAP_ID: &'static str = $clap_id;
            const CLAP_FEATURES: &'static [nih_plug::prelude::ClapFeature] =
                &[nih_plug::prelude::ClapFeature::AudioEffect];
            const CLAP_DESCRIPTION: Option<&'static str> = None;
            const CLAP_MANUAL_URL: Option<&'static str> = None;
            const CLAP_SUPPORT_URL: Option<&'static str> = None;
            const CLAP_SUPPORTS_AMBISONIC: bool =
                $crate::sotf_nih_is_ambisonics!($plugin_type);
            const CLAP_SUPPORTS_SURROUND: bool =
                $crate::sotf_nih_is_ambisonics!($plugin_type);

            fn clap_audio_io_layouts() -> &'static [nih_plug::prelude::AudioIOLayout] {
                if matches!($plugin_type, "AmbisonicsDecoder") {
                    &$crate::wrapper::AMBISONICS_CLAP_LAYOUTS
                } else if matches!($plugin_type, "BandSplit") {
                    &$crate::wrapper::BAND_SPLIT_CLAP_LAYOUTS
                } else {
                    <Self as nih_plug::prelude::Plugin>::AUDIO_IO_LAYOUTS
                }
            }

            fn clap_audio_port_type(
                _layout_index: usize,
                is_input: bool,
                port_index: usize,
            ) -> Option<&'static std::ffi::CStr> {
                if !matches!($plugin_type, "AmbisonicsDecoder") || port_index != 0 {
                    return None;
                }
                Some(if is_input {
                    clap_sys::ext::ambisonic::CLAP_PORT_AMBISONIC
                } else {
                    clap_sys::ext::surround::CLAP_PORT_SURROUND
                })
            }

            fn clap_ambisonic_config(
                is_input: bool,
                port_index: usize,
            ) -> Option<clap_sys::ext::ambisonic::clap_ambisonic_config> {
                if matches!($plugin_type, "AmbisonicsDecoder") && is_input && port_index == 0 {
                    Some(clap_sys::ext::ambisonic::clap_ambisonic_config {
                        ordering: clap_sys::ext::ambisonic::CLAP_AMBISONIC_ORDERING_ACN,
                        normalization: clap_sys::ext::ambisonic::CLAP_AMBISONIC_NORMALIZATION_SN3D,
                    })
                } else {
                    None
                }
            }

            fn clap_surround_channel_map(
                layout_index: usize,
                is_input: bool,
                port_index: usize,
            ) -> Option<&'static [u8]> {
                if matches!($plugin_type, "AmbisonicsDecoder") && !is_input && port_index == 0 {
                    $crate::wrapper::ambisonics_clap_channel_map(layout_index % 6)
                } else {
                    None
                }
            }

            fn clap_surround_channel_mask_supported(channel_mask: u64) -> bool {
                matches!($plugin_type, "AmbisonicsDecoder")
                    && $crate::wrapper::ambisonics_clap_channel_mask_supported(channel_mask)
            }
        }
    };
}

/// Convert an output-clock tail bound to the common native representation.
#[doc(hidden)]
pub fn native_tail_samples(tail: sotf_host::TailLength) -> u32 {
    match tail {
        sotf_host::TailLength::Finite(frames) if frames < i32::MAX as u64 => frames as u32,
        _ => u32::MAX,
    }
}

/// Keep processing until the declared zero-input response can be emitted.
#[doc(hidden)]
pub fn process_status_for_tail(tail: sotf_host::TailLength) -> nih_plug::prelude::ProcessStatus {
    match native_tail_samples(tail) {
        0 => nih_plug::prelude::ProcessStatus::Normal,
        u32::MAX => nih_plug::prelude::ProcessStatus::KeepAlive,
        frames => nih_plug::prelude::ProcessStatus::Tail(frames),
    }
}

/// Stable DAW parameter count. Neutral peaking bands preserve pass-through audio
/// while making every supported EQ band available before the host scans params.
pub const NIH_EQ_BANDS: usize = 20;

/// Default construction used for parameter discovery and host initialization.
pub fn default_plugin_config(plugin_type: &str) -> String {
    match plugin_type {
        "EQ" => eq_config_json(|_| None),
        "LinearPhaseEQ" => r#"{"num_filters":10}"#.to_string(),
        _ => "{}".to_string(),
    }
}

/// Restore the fixed EQ band schema from DAW state before starting audio.
pub fn eq_config_json(
    value: impl Fn(&str) -> Option<sotf_host::parameters::ParameterValue>,
) -> String {
    use sotf_host::parameters::ParameterValue;
    let types = [
        "Peak",
        "Lowshelf",
        "Highshelf",
        "Lowpass",
        "Highpass",
        "Bandpass",
        "Notch",
        "AllPass",
    ];
    let filters: Vec<_> = (0..NIH_EQ_BANDS)
        .map(|band| {
            let float = |field, default| {
                value(&format!("band_{band}_{field}"))
                    .and_then(|value| value.as_float())
                    .unwrap_or(default)
            };
            let integer = |field, default| {
                value(&format!("band_{band}_{field}"))
                    .and_then(|value| value.as_int())
                    .unwrap_or(default)
            };
            serde_json::json!({
                "filter_type": types.get(integer("filter_type", 0) as usize).unwrap_or(&"Peak"),
                "freq": float("freq", 1000.0),
                "q": float("q", 1.0),
                "db_gain": float("gain", 0.0),
                "order": integer("order", 2),
            })
        })
        .collect();
    let enabled = matches!(value("auto_gain_enabled"), Some(ParameterValue::Bool(true)));
    serde_json::json!({ "filters": filters, "auto_gain": { "enabled": enabled } }).to_string()
}

/// Apply structural controls on the control thread. Their host representation
/// remains the historical choice index while the DSP accepts an actual factor.
pub(crate) fn apply_eq_structural(
    plugin: &mut dyn sotf_host::plugin::Plugin,
    value: impl Fn(&str) -> Option<sotf_host::parameters::ParameterValue>,
) -> Result<(), String> {
    use sotf_host::parameters::{ParameterId, ParameterValue};
    for id in ["max_filters", "oversampling", "topology"] {
        if let Some(mut current) = value(id) {
            if id == "oversampling" {
                current = ParameterValue::Int(match current.as_int().unwrap_or(0) {
                    1 => 2,
                    2 => 4,
                    _ => 1,
                });
            }
            plugin.set_parameter(ParameterId::from(id), current)?;
        }
    }
    Ok(())
}

/// Convert runtime plugin metadata to NIH's format without erasing integer,
/// boolean, or structural/realtime semantics.
pub fn bridged_info_from_parameter(
    parameter: &sotf_host::parameters::Parameter,
) -> Option<plugins_bridge::param_bridge::BridgedParamInfo> {
    use plugins_bridge::param_bridge::BridgedParamKind;
    use sotf_host::param_specs::UpdateMode;
    use sotf_host::parameters::ParameterValue;

    let (min_value, max_value, default_value, steps) = match (
        &parameter.min_value,
        &parameter.max_value,
        &parameter.default_value,
    ) {
        (
            Some(ParameterValue::Float(min)),
            Some(ParameterValue::Float(max)),
            ParameterValue::Float(default),
        ) => (*min as f64, *max as f64, *default as f64, 0),
        (
            Some(ParameterValue::Int(min)),
            Some(ParameterValue::Int(max)),
            ParameterValue::Int(default),
        ) => (
            *min as f64,
            *max as f64,
            *default as f64,
            u32::try_from(i64::from(*max) - i64::from(*min) + 1).ok()?,
        ),
        (None, None, ParameterValue::Bool(default)) => {
            (0.0, 1.0, if *default { 1.0 } else { 0.0 }, 1)
        }
        _ => return None,
    };
    Some(plugins_bridge::param_bridge::BridgedParamInfo {
        id: parameter.id.to_string(),
        name: parameter.name.clone(),
        unit: parameter.unit.clone(),
        min_value,
        max_value,
        default_value,
        kind: match parameter.default_value {
            ParameterValue::Float(_) => BridgedParamKind::Float,
            ParameterValue::Int(_) => BridgedParamKind::Int,
            ParameterValue::Bool(_) => BridgedParamKind::Bool,
            ParameterValue::String(_) => return None,
        },
        steps,
        logarithmic: parameter.logarithmic,
        realtime: parameter.update_mode == UpdateMode::Realtime,
        group: parameter.group.clone(),
    })
}

/// Validate a host-supplied block against the bounds negotiated in `initialize()`.
///
/// Returns the required interleaved sample count when `num_frames` fits within
/// `max_frames` and `num_channels` matches the plugin's declared layout;
/// returns `None` otherwise (oversized block, channel mismatch, or sample
/// count overflow). Callers must fill the host buffer with silence and return
/// `ProcessStatus::Error` on `None`, mirroring the FFI crate's
/// `BufferTooSmall` behavior.
pub fn check_host_block(
    num_frames: usize,
    num_channels: usize,
    max_frames: usize,
    expected_channels: usize,
) -> Option<usize> {
    if num_channels != expected_channels || num_frames > max_frames {
        return None;
    }
    num_frames.checked_mul(num_channels)
}

#[doc(hidden)]
pub fn silence_host_outputs(
    buffer: &mut nih_plug::prelude::Buffer,
    aux: &mut nih_plug::prelude::AuxiliaryBuffers,
) {
    for channel in buffer.as_slice() {
        channel.fill(0.0);
    }
    for output_bus in aux.outputs.iter_mut() {
        for channel in output_bus.as_slice() {
            channel.fill(0.0);
        }
    }
}

/// DAW-facing parameter id for a canonical spec key.
///
/// CLAP/VST3 hosts persist parameter ids across sessions, so the four
/// choice parameters renamed to `*_index`/`mode`/`preset`/`type` keep their
/// pre-migration ids on this boundary. The internal engine, toolbar, and
/// factory all use the canonical keys; the NIH map translates back when
/// building host-visible state (see `linear_phase_eq_config_json`).
/// Scoped per plugin type so untouched plugins sharing a name (e.g. the
/// upmixer's own `fft_size`) are unaffected.
pub fn legacy_external_param_id<'a>(
    plugin_type: &str,
    canonical: &'a str,
) -> std::borrow::Cow<'a, str> {
    use std::borrow::Cow;
    let linear_phase_eq = matches!(plugin_type, "LinearPhaseEQ" | "linear_phase_eq");
    let crossfeed = matches!(plugin_type, "Crossfeed" | "crossfeed");
    let spectral = matches!(plugin_type, "SpectralCompressor" | "spectral_compressor");
    let band_split = matches!(plugin_type, "BandSplit" | "band_split");
    match canonical {
        "fir_length_index" if linear_phase_eq => Cow::Borrowed("fir_length"),
        "phase_mode_index" if linear_phase_eq => Cow::Borrowed("phase_mode"),
        "mode" if crossfeed => Cow::Borrowed("crossfeed_mode"),
        "preset" if crossfeed => Cow::Borrowed("crossfeed_preset"),
        "fft_size_index" if spectral => Cow::Borrowed("fft_size"),
        "type" if band_split => Cow::Borrowed("crossover_type"),
        _ => Cow::Borrowed(canonical),
    }
}

/// Get ParamSpec array for a plugin type.
pub fn get_param_specs(plugin_type: &str) -> &'static [sotf_host::param_specs::ParamSpec] {
    use sotf_plugins::param_specs::*;

    match plugin_type {
        "EQ" => eq::GLOBAL_PARAMS,
        "Compressor" => compressor::PARAMS,
        "Limiter" => limiter::PARAMS,
        "Gate" => gate::PARAMS,
        "Gain" => gain::PARAMS,
        "Saturation" => saturation::PARAMS,
        "Delay" => delay::PARAMS,
        "Expander" => expander::PARAMS,
        "Crossfeed" => crossfeed::PARAMS,
        "FletcherMunson" => loudness_compensation::PARAMS,
        "LoudnessCompensation" => loudness_compensation::PARAMS,
        "MultibandCompressor" => multiband_compressor::GLOBAL_PARAMS,
        "MultibandExpander" => multiband_expander::GLOBAL_PARAMS,
        "Upmixer" => upmixer::PARAMS,
        "AAE" => aae::PARAMS,
        "XTC" => xtc::PARAMS,
        "Binaural" => binaural::PARAMS,
        "Denoiser" => denoiser::PARAMS,
        "SpeechDenoiser" => speech_denoiser::PARAMS,
        "HissReducer" => hiss_reducer::PARAMS,
        "Declick" => declick::PARAMS,
        "BandSplit" => band_split::PARAMS,
        "BandMerge" => band_merge::PARAMS,
        "Crossover" => crate::native_crossover::PARAMS,
        "AEC" => aec::PARAMS,
        "Beamformer" => beamformer::PARAMS,
        "LinearPhaseEQ" => linear_phase_eq::PARAMS,
        "SpectralCompressor" => spectral_compressor::PARAMS,
        "AmbisonicsDecoder" => ambisonics::PARAMS,
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::check_host_block;

    #[test]
    fn fitting_block_returns_interleaved_sample_count() {
        assert_eq!(check_host_block(64, 2, 128, 2), Some(128));
        assert_eq!(check_host_block(128, 2, 128, 2), Some(256));
        assert_eq!(check_host_block(0, 2, 128, 2), Some(0));
    }

    #[test]
    fn oversized_block_is_rejected() {
        assert_eq!(check_host_block(129, 2, 128, 2), None);
        assert_eq!(check_host_block(1024, 2, 128, 2), None);
    }

    #[test]
    fn channel_mismatch_is_rejected() {
        assert_eq!(check_host_block(64, 1, 128, 2), None);
        assert_eq!(check_host_block(64, 4, 128, 2), None);
    }

    #[test]
    fn overflowing_sample_count_is_rejected() {
        assert_eq!(
            check_host_block(usize::MAX, usize::MAX, usize::MAX, usize::MAX),
            None
        );
    }
}

#[cfg(test)]
#[path = "wrapper_tests.rs"]
mod process_tests;
