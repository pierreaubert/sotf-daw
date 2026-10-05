//! Macro-based wrapper that generates nih-plug Plugin implementations for SOTF plugins.

use nih_plug::audio_setup::{AudioIOLayout, PortNames, new_nonzero_u32};
use nih_plug::context::PluginApi;
use sotf_host::external_plugin::{
    MAX_AMBISONICS_CUSTOM_SPEAKERS, NativeCrossoverInputLayout as CrossoverInputLayout,
};

#[cfg(feature = "convolution")]
pub(crate) mod native_convolution_editor;

#[cfg(feature = "convolution")]
#[doc(hidden)]
pub type NativeConvolutionBackgroundTask = native_convolution_editor::BackgroundTask;

#[cfg(not(feature = "convolution"))]
#[doc(hidden)]
pub type NativeConvolutionBackgroundTask = ();

#[doc(hidden)]
#[macro_export]
macro_rules! sotf_nih_background_task_type {
    ("Convolution") => {
        $crate::wrapper::NativeConvolutionBackgroundTask
    };
    ($other:literal) => {
        ()
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! sotf_nih_task_executor {
    ("Convolution", $params:expr, $service:expr) => {{
        let params = ($params).clone();
        let service = ($service).clone();
        Box::new(move |task| {
            $crate::wrapper::native_convolution_editor::handle_background_task(
                &params, &service, task,
            );
        })
    }};
    ($other:literal, $params:expr, $service:expr) => {{ Box::new(|_task| {}) }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! sotf_nih_create_editor {
    ("Convolution", $plugin:ty, $params:expr, $service:expr, $executor:expr) => {{
        #[cfg(feature = "convolution")]
        {
            $crate::wrapper::native_convolution_editor::create_editor::<$plugin>(
                ($params).clone(),
                ($service).clone(),
                $executor,
            )
        }
        #[cfg(not(feature = "convolution"))]
        {
            let _ = $executor;
            None
        }
    }};
    ($other:literal, $plugin:ty, $params:expr, $service:expr, $executor:expr) => {{
        let _ = $executor;
        None
    }};
}

const AMBISONICS_OUTPUT_WIDTHS: [u32; 8] = [6, 8, 8, 10, 10, 12, 14, 16];
// Canonical DSP slot order per TARGET_LAYOUTS (5.1.4 at 3, 7.1.2 at 4).
const AMBISONICS_LAYOUT_NAMES: [&str; 56] = [
    "Order 1 - 5.1",
    "Order 1 - 7.1",
    "Order 1 - 5.1.2",
    "Order 1 - 5.1.4",
    "Order 1 - 7.1.2",
    "Order 1 - 7.1.4",
    "Order 1 - 9.1.4",
    "Order 1 - 9.1.6",
    "Order 2 - 5.1",
    "Order 2 - 7.1",
    "Order 2 - 5.1.2",
    "Order 2 - 5.1.4",
    "Order 2 - 7.1.2",
    "Order 2 - 7.1.4",
    "Order 2 - 9.1.4",
    "Order 2 - 9.1.6",
    "Order 3 - 5.1",
    "Order 3 - 7.1",
    "Order 3 - 5.1.2",
    "Order 3 - 5.1.4",
    "Order 3 - 7.1.2",
    "Order 3 - 7.1.4",
    "Order 3 - 9.1.4",
    "Order 3 - 9.1.6",
    "Order 4 - 5.1",
    "Order 4 - 7.1",
    "Order 4 - 5.1.2",
    "Order 4 - 5.1.4",
    "Order 4 - 7.1.2",
    "Order 4 - 7.1.4",
    "Order 4 - 9.1.4",
    "Order 4 - 9.1.6",
    "Order 5 - 5.1",
    "Order 5 - 7.1",
    "Order 5 - 5.1.2",
    "Order 5 - 5.1.4",
    "Order 5 - 7.1.2",
    "Order 5 - 7.1.4",
    "Order 5 - 9.1.4",
    "Order 5 - 9.1.6",
    "Order 6 - 5.1",
    "Order 6 - 7.1",
    "Order 6 - 5.1.2",
    "Order 6 - 5.1.4",
    "Order 6 - 7.1.2",
    "Order 6 - 7.1.4",
    "Order 6 - 9.1.4",
    "Order 6 - 9.1.6",
    "Order 7 - 5.1",
    "Order 7 - 7.1",
    "Order 7 - 5.1.2",
    "Order 7 - 5.1.4",
    "Order 7 - 7.1.2",
    "Order 7 - 7.1.4",
    "Order 7 - 9.1.4",
    "Order 7 - 9.1.6",
];
// Canonical DSP slot order per TARGET_LAYOUTS (5.1.4 at 3, 7.1.2 at 4).
const CLAP_AMBISONICS_LAYOUT_NAMES: [&str; 42] = [
    "Order 1 - 5.1",
    "Order 1 - 7.1",
    "Order 1 - 5.1.2",
    "Order 1 - 5.1.4",
    "Order 1 - 7.1.2",
    "Order 1 - 7.1.4",
    "Order 2 - 5.1",
    "Order 2 - 7.1",
    "Order 2 - 5.1.2",
    "Order 2 - 5.1.4",
    "Order 2 - 7.1.2",
    "Order 2 - 7.1.4",
    "Order 3 - 5.1",
    "Order 3 - 7.1",
    "Order 3 - 5.1.2",
    "Order 3 - 5.1.4",
    "Order 3 - 7.1.2",
    "Order 3 - 7.1.4",
    "Order 4 - 5.1",
    "Order 4 - 7.1",
    "Order 4 - 5.1.2",
    "Order 4 - 5.1.4",
    "Order 4 - 7.1.2",
    "Order 4 - 7.1.4",
    "Order 5 - 5.1",
    "Order 5 - 7.1",
    "Order 5 - 5.1.2",
    "Order 5 - 5.1.4",
    "Order 5 - 7.1.2",
    "Order 5 - 7.1.4",
    "Order 6 - 5.1",
    "Order 6 - 7.1",
    "Order 6 - 5.1.2",
    "Order 6 - 5.1.4",
    "Order 6 - 7.1.2",
    "Order 6 - 7.1.4",
    "Order 7 - 5.1",
    "Order 7 - 7.1",
    "Order 7 - 5.1.2",
    "Order 7 - 5.1.4",
    "Order 7 - 7.1.2",
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
        // Slot 3 is 5.1.4 (identity: SOTF order already matches VST3
        // bit order); slot 4 is 7.1.2 (sides before backs in SOTF).
        3 => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
        4 => &[0, 1, 2, 3, 6, 7, 4, 5, 8, 9],
        5 => &[0, 1, 2, 3, 6, 7, 4, 5, 8, 9, 10, 11],
        6 => &[0, 1, 2, 3, 6, 7, 4, 5, 10, 11, 12, 13, 8, 9],
        7 => &[0, 1, 2, 3, 6, 7, 4, 5, 10, 11, 12, 13, 14, 15, 8, 9],
        _ => return None,
    };
    map.get(channel).copied()
}

/// Map one VST3 output bus channel to its SOTF source channel.
///
/// Named targets use the static tables; target 8 reads the negotiated
/// custom permutation installed during initialization. Returns `None`
/// for unmapped channels; the process callback silences and reports.
#[doc(hidden)]
pub fn ambisonics_vst3_output_to_sotf_custom(
    target_layout: usize,
    custom_channels: usize,
    custom_map: &[usize; MAX_AMBISONICS_CUSTOM_SPEAKERS],
    channel: usize,
) -> Option<usize> {
    if target_layout == crate::params::ambisonics_custom::AMBISONICS_CUSTOM_TARGET_INDEX {
        custom_map
            .get(channel)
            .copied()
            .filter(|_| channel < custom_channels)
    } else {
        ambisonics_vst3_output_to_sotf(target_layout, channel)
    }
}

/// Negotiate a custom-geometry layout during NIH initialization.
///
/// Validates the staged-or-committed geometry against the negotiated
/// order, bus width, and the exact wire bytes the format callbacks
/// will report for the selected layout, then records target 8 in the
/// structural parameters. Returns the VST3 bus-to-SOTF permutation
/// for the audio path, or `None` when the layout is unmapped, the
/// geometry disagrees with the negotiated bus or wire bytes, or the
/// structural parameters cannot record target 8. Control thread only.
#[doc(hidden)]
pub fn negotiate_ambisonics_custom_layout(
    params: &crate::params::DynamicParams,
    order: usize,
    target_slot: usize,
    layout_index: usize,
    output_channels: usize,
    api: nih_plug::context::PluginApi,
) -> Option<Vec<usize>> {
    let expected_clap_map = if api == nih_plug::context::PluginApi::Clap {
        Some(ambisonics_clap_channel_map(target_slot)?)
    } else {
        None
    };
    let expected_vst3_mask = if api == nih_plug::context::PluginApi::Clap {
        None
    } else {
        ambisonics_vst3_arrangement(layout_index, false)
    };
    let permutation = params
        .validate_restored_ambisonics_custom_layout(
            order,
            output_channels,
            expected_clap_map,
            expected_vst3_mask,
        )
        .ok()?;
    params
        .set_ambisonics_layout(
            order,
            crate::params::ambisonics_custom::AMBISONICS_CUSTOM_TARGET_INDEX,
        )
        .ok()?;
    Some(permutation)
}

const CLAP_MAP_51: [u8; 6] = [0, 1, 2, 3, 9, 10];
const CLAP_MAP_71: [u8; 8] = [0, 1, 2, 3, 9, 10, 4, 5];
const CLAP_MAP_512: [u8; 8] = [0, 1, 2, 3, 9, 10, 12, 14];
const CLAP_MAP_712: [u8; 10] = [0, 1, 2, 3, 9, 10, 4, 5, 12, 14];
const CLAP_MAP_514: [u8; 10] = [0, 1, 2, 3, 9, 10, 12, 14, 15, 17];
const CLAP_MAP_714: [u8; 12] = [0, 1, 2, 3, 9, 10, 4, 5, 12, 14, 15, 17];
const CROSSOVER_CLAP_MAP_MONO: [u8; 1] = [clap_sys::ext::surround::CLAP_SURROUND_FC as u8];
const CROSSOVER_CLAP_MAP_STEREO: [u8; 2] = [
    clap_sys::ext::surround::CLAP_SURROUND_FL as u8,
    clap_sys::ext::surround::CLAP_SURROUND_FR as u8,
];
const CROSSOVER_CLAP_MAP_QUAD: [u8; 4] = [
    clap_sys::ext::surround::CLAP_SURROUND_FL as u8,
    clap_sys::ext::surround::CLAP_SURROUND_FR as u8,
    clap_sys::ext::surround::CLAP_SURROUND_BL as u8,
    clap_sys::ext::surround::CLAP_SURROUND_BR as u8,
];
const CROSSOVER_CLAP_MAP_914: [u8; 14] = [0, 1, 2, 3, 9, 10, 4, 5, 15, 17, 6, 7, 12, 14];
const CROSSOVER_CLAP_MAP_916_WIDE: [u8; 16] =
    [0, 1, 2, 3, 9, 10, 4, 5, 6, 7, 12, 14, 13, 16, 15, 17];

#[doc(hidden)]
pub fn crossover_clap_channel_map(
    input_layout_index: usize,
    is_input: bool,
    port_index: usize,
) -> Option<&'static [u8]> {
    if !is_input || port_index != 0 {
        return None;
    }
    Some(match CROSSOVER_INPUT_LAYOUTS.get(input_layout_index)? {
        CrossoverInputLayout::Mono => &CROSSOVER_CLAP_MAP_MONO,
        CrossoverInputLayout::Stereo => &CROSSOVER_CLAP_MAP_STEREO,
        CrossoverInputLayout::Quad => &CROSSOVER_CLAP_MAP_QUAD,
        CrossoverInputLayout::FiveOne => &CLAP_MAP_51,
        CrossoverInputLayout::SevenOne => &CLAP_MAP_71,
        CrossoverInputLayout::FiveOneTwo => &CLAP_MAP_512,
        CrossoverInputLayout::FiveOneFour => &CLAP_MAP_514,
        CrossoverInputLayout::SevenOneTwo => &CLAP_MAP_712,
        CrossoverInputLayout::SevenOneFour => &CLAP_MAP_714,
        CrossoverInputLayout::NineOneFour => &CROSSOVER_CLAP_MAP_914,
        CrossoverInputLayout::NineOneSixWide => &CROSSOVER_CLAP_MAP_916_WIDE,
    })
}

#[doc(hidden)]
pub fn crossover_clap_channel_mask_supported(channel_mask: u64) -> bool {
    (0..CROSSOVER_INPUT_LAYOUTS.len()).any(|layout_index| {
        crossover_clap_channel_map(layout_index, true, 0).is_some_and(|channel_map| {
            channel_map
                .iter()
                .fold(0_u64, |mask, channel| mask | (1_u64 << channel))
                == channel_mask
        })
    })
}

#[doc(hidden)]
pub fn ambisonics_clap_channel_map(target_layout: usize) -> Option<&'static [u8]> {
    Some(match target_layout {
        0 => &CLAP_MAP_51,
        1 => &CLAP_MAP_71,
        2 => &CLAP_MAP_512,
        3 => &CLAP_MAP_514,
        4 => &CLAP_MAP_712,
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
            3 => 0x0000_0000_0002_d03f, // 5.1.4
            4 => 0x0000_0000_0000_563f, // 7.1.2 front heights
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

const CROSSOVER_INPUT_LAYOUTS: [CrossoverInputLayout; 11] = [
    CrossoverInputLayout::Stereo,
    CrossoverInputLayout::Mono,
    CrossoverInputLayout::Quad,
    CrossoverInputLayout::FiveOne,
    CrossoverInputLayout::SevenOne,
    CrossoverInputLayout::FiveOneTwo,
    CrossoverInputLayout::FiveOneFour,
    CrossoverInputLayout::SevenOneTwo,
    CrossoverInputLayout::SevenOneFour,
    CrossoverInputLayout::NineOneFour,
    CrossoverInputLayout::NineOneSixWide,
];

const CROSSOVER_CLAP_LAYOUT_NAMES: [&str; 44] = [
    "Crossover Stereo 1x",
    "Crossover Stereo 2x",
    "Crossover Stereo 3x",
    "Crossover Stereo 4x",
    "Crossover Mono 1x",
    "Crossover Mono 2x",
    "Crossover Mono 3x",
    "Crossover Mono 4x",
    "Crossover Quad 1x",
    "Crossover Quad 2x",
    "Crossover Quad 3x",
    "Crossover Quad 4x",
    "Crossover 5.1 1x",
    "Crossover 5.1 2x",
    "Crossover 5.1 3x",
    "Crossover 5.1 4x",
    "Crossover 7.1 1x",
    "Crossover 7.1 2x",
    "Crossover 7.1 3x",
    "Crossover 7.1 4x",
    "Crossover 5.1.2 1x",
    "Crossover 5.1.2 2x",
    "Crossover 5.1.2 3x",
    "Crossover 5.1.2 4x",
    "Crossover 5.1.4 1x",
    "Crossover 5.1.4 2x",
    "Crossover 5.1.4 3x",
    "Crossover 5.1.4 4x",
    "Crossover 7.1.2 1x",
    "Crossover 7.1.2 2x",
    "Crossover 7.1.2 3x",
    "Crossover 7.1.2 4x",
    "Crossover 7.1.4 1x",
    "Crossover 7.1.4 2x",
    "Crossover 7.1.4 3x",
    "Crossover 7.1.4 4x",
    "Crossover 9.1.4 1x",
    "Crossover 9.1.4 2x",
    "Crossover 9.1.4 3x",
    "Crossover 9.1.4 4x",
    "Crossover 9.1.6 wide 1x",
    "Crossover 9.1.6 wide 2x",
    "Crossover 9.1.6 wide 3x",
    "Crossover 9.1.6 wide 4x",
];

const CROSSOVER_INPUT_LAYOUT_NAMES: [&str; 11] = [
    "Stereo Input",
    "Mono Input",
    "Quad Input",
    "5.1 Input",
    "7.1 Input",
    "5.1.2 Input",
    "5.1.4 Input",
    "7.1.2 Input",
    "7.1.4 Input",
    "9.1.4 Input",
    "9.1.6 Wide Input",
];

const EQ_NATIVE_LAYOUT_NAMES: [&str; 11] = [
    "EQ Stereo",
    "EQ Mono",
    "EQ Quad",
    "EQ 5.1",
    "EQ 7.1",
    "EQ 5.1.2",
    "EQ 5.1.4",
    "EQ 7.1.2",
    "EQ 7.1.4",
    "EQ 9.1.4",
    "EQ 9.1.6 Wide",
];

const fn eq_native_layout(index: usize) -> AudioIOLayout {
    let width = CROSSOVER_INPUT_LAYOUTS[index].channel_count() as u32;
    AudioIOLayout {
        main_input_channels: Some(new_nonzero_u32(width)),
        main_output_channels: Some(new_nonzero_u32(width)),
        aux_input_ports: &[],
        aux_output_ports: &[],
        names: PortNames {
            layout: Some(EQ_NATIVE_LAYOUT_NAMES[index]),
            main_input: Some(CROSSOVER_INPUT_LAYOUT_NAMES[index]),
            main_output: Some("Speaker outputs"),
            ..PortNames::const_default()
        },
    }
}

const fn make_eq_native_layouts() -> [AudioIOLayout; 11] {
    let mut layouts = [AudioIOLayout::const_default(); 11];
    let mut index = 0;
    while index < layouts.len() {
        layouts[index] = eq_native_layout(index);
        index += 1;
    }
    layouts
}

/// Equal-width native EQ layouts for the supported named speaker arrangements.
#[doc(hidden)]
pub static EQ_NATIVE_LAYOUTS: [AudioIOLayout; 11] = make_eq_native_layouts();

/// Return the EQ layout index selected by a host API's native layout.
#[doc(hidden)]
pub fn eq_native_layout_index(layout: &AudioIOLayout, api: PluginApi) -> Option<usize> {
    let _ = api;
    EQ_NATIVE_LAYOUTS
        .iter()
        .position(|candidate| candidate == layout)
}

/// Map a host EQ channel index to the canonical SOTF speaker order.
#[doc(hidden)]
pub fn eq_native_channel_to_sotf(
    layout_index: usize,
    api: PluginApi,
    channel: usize,
) -> Option<usize> {
    crossover_channel_to_sotf(layout_index, api, channel)
}

const fn crossover_clap_layout(
    input_channels: u32,
    output_channels: u32,
    input_name: &'static str,
    layout_name: &'static str,
) -> AudioIOLayout {
    AudioIOLayout {
        main_input_channels: Some(new_nonzero_u32(input_channels)),
        main_output_channels: Some(new_nonzero_u32(output_channels)),
        aux_input_ports: &[],
        aux_output_ports: &[],
        names: PortNames {
            layout: Some(layout_name),
            main_input: Some(input_name),
            main_output: Some("Band-major indexed output"),
            ..PortNames::const_default()
        },
    }
}

const fn make_crossover_clap_layouts() -> [AudioIOLayout; 44] {
    let mut layouts = [AudioIOLayout::const_default(); 44];
    let mut layout_index = 0;
    while layout_index < CROSSOVER_INPUT_LAYOUTS.len() {
        let input_channels = CROSSOVER_INPUT_LAYOUTS[layout_index].channel_count() as u32;
        let mut output_variant = 0;
        while output_variant < 4 {
            let index = layout_index * 4 + output_variant;
            let output_channels = input_channels * (output_variant as u32 + 1);
            layouts[index] = crossover_clap_layout(
                input_channels,
                output_channels,
                CROSSOVER_INPUT_LAYOUT_NAMES[layout_index],
                CROSSOVER_CLAP_LAYOUT_NAMES[index],
            );
            output_variant += 1;
        }
        layout_index += 1;
    }
    layouts
}

#[doc(hidden)]
pub static CROSSOVER_CLAP_LAYOUTS: [AudioIOLayout; 44] = make_crossover_clap_layouts();

const CROSSOVER_VST3_AUX_OUTPUTS_1: [std::num::NonZeroU32; 3] =
    [new_nonzero_u32(1), new_nonzero_u32(1), new_nonzero_u32(1)];
const CROSSOVER_VST3_AUX_OUTPUTS_2: [std::num::NonZeroU32; 3] =
    [new_nonzero_u32(2), new_nonzero_u32(2), new_nonzero_u32(2)];
const CROSSOVER_VST3_AUX_OUTPUTS_4: [std::num::NonZeroU32; 3] =
    [new_nonzero_u32(4), new_nonzero_u32(4), new_nonzero_u32(4)];
const CROSSOVER_VST3_AUX_OUTPUTS_6: [std::num::NonZeroU32; 3] =
    [new_nonzero_u32(6), new_nonzero_u32(6), new_nonzero_u32(6)];
const CROSSOVER_VST3_AUX_OUTPUTS_8: [std::num::NonZeroU32; 3] =
    [new_nonzero_u32(8), new_nonzero_u32(8), new_nonzero_u32(8)];
const CROSSOVER_VST3_AUX_OUTPUTS_10: [std::num::NonZeroU32; 3] = [
    new_nonzero_u32(10),
    new_nonzero_u32(10),
    new_nonzero_u32(10),
];
const CROSSOVER_VST3_AUX_OUTPUTS_12: [std::num::NonZeroU32; 3] = [
    new_nonzero_u32(12),
    new_nonzero_u32(12),
    new_nonzero_u32(12),
];
const CROSSOVER_VST3_AUX_OUTPUTS_14: [std::num::NonZeroU32; 3] = [
    new_nonzero_u32(14),
    new_nonzero_u32(14),
    new_nonzero_u32(14),
];
const CROSSOVER_VST3_AUX_OUTPUTS_16: [std::num::NonZeroU32; 3] = [
    new_nonzero_u32(16),
    new_nonzero_u32(16),
    new_nonzero_u32(16),
];
const CROSSOVER_VST3_AUX_OUTPUT_NAMES: [&str; 3] = ["Band 2", "Band 3", "Band 4"];
const CROSSOVER_VST3_LAYOUT_NAMES: [&str; 11] = [
    "Crossover stereo buses",
    "Crossover mono buses",
    "Crossover quad buses",
    "Crossover 5.1 buses",
    "Crossover 7.1 buses",
    "Crossover 5.1.2 buses",
    "Crossover 5.1.4 buses",
    "Crossover 7.1.2 buses",
    "Crossover 7.1.4 buses",
    "Crossover 9.1.4 buses",
    "Crossover 9.1.6 wide buses",
];

const fn crossover_vst3_layout(
    index: usize,
    input_layout: CrossoverInputLayout,
    aux_output_ports: &'static [std::num::NonZeroU32; 3],
) -> AudioIOLayout {
    let width = input_layout.channel_count() as u32;
    AudioIOLayout {
        main_input_channels: Some(new_nonzero_u32(width)),
        main_output_channels: Some(new_nonzero_u32(width)),
        aux_input_ports: &[],
        aux_output_ports,
        names: PortNames {
            layout: Some(CROSSOVER_VST3_LAYOUT_NAMES[index]),
            main_input: Some(CROSSOVER_INPUT_LAYOUT_NAMES[index]),
            main_output: Some("Band 1"),
            aux_outputs: &CROSSOVER_VST3_AUX_OUTPUT_NAMES,
            ..PortNames::const_default()
        },
    }
}

#[doc(hidden)]
pub static CROSSOVER_VST3_LAYOUTS: [AudioIOLayout; 11] = [
    crossover_vst3_layout(0, CROSSOVER_INPUT_LAYOUTS[0], &CROSSOVER_VST3_AUX_OUTPUTS_2),
    crossover_vst3_layout(1, CROSSOVER_INPUT_LAYOUTS[1], &CROSSOVER_VST3_AUX_OUTPUTS_1),
    crossover_vst3_layout(2, CROSSOVER_INPUT_LAYOUTS[2], &CROSSOVER_VST3_AUX_OUTPUTS_4),
    crossover_vst3_layout(3, CROSSOVER_INPUT_LAYOUTS[3], &CROSSOVER_VST3_AUX_OUTPUTS_6),
    crossover_vst3_layout(4, CROSSOVER_INPUT_LAYOUTS[4], &CROSSOVER_VST3_AUX_OUTPUTS_8),
    crossover_vst3_layout(5, CROSSOVER_INPUT_LAYOUTS[5], &CROSSOVER_VST3_AUX_OUTPUTS_8),
    crossover_vst3_layout(
        6,
        CROSSOVER_INPUT_LAYOUTS[6],
        &CROSSOVER_VST3_AUX_OUTPUTS_10,
    ),
    crossover_vst3_layout(
        7,
        CROSSOVER_INPUT_LAYOUTS[7],
        &CROSSOVER_VST3_AUX_OUTPUTS_10,
    ),
    crossover_vst3_layout(
        8,
        CROSSOVER_INPUT_LAYOUTS[8],
        &CROSSOVER_VST3_AUX_OUTPUTS_12,
    ),
    crossover_vst3_layout(
        9,
        CROSSOVER_INPUT_LAYOUTS[9],
        &CROSSOVER_VST3_AUX_OUTPUTS_14,
    ),
    crossover_vst3_layout(
        10,
        CROSSOVER_INPUT_LAYOUTS[10],
        &CROSSOVER_VST3_AUX_OUTPUTS_16,
    ),
];

#[doc(hidden)]
pub fn crossover_clap_output_channels(layout: &AudioIOLayout) -> Option<usize> {
    CROSSOVER_CLAP_LAYOUTS
        .iter()
        .position(|candidate| candidate == layout)
        .and_then(|_| {
            layout
                .main_output_channels
                .map(|channels| channels.get() as usize)
        })
}

#[doc(hidden)]
pub fn crossover_input_layout_index(layout: &AudioIOLayout, api: PluginApi) -> Option<usize> {
    match api {
        PluginApi::Clap => CROSSOVER_CLAP_LAYOUTS
            .iter()
            .position(|candidate| candidate == layout)
            .map(|index| index / 4),
        PluginApi::Vst3 | PluginApi::Standalone => CROSSOVER_VST3_LAYOUTS
            .iter()
            .position(|candidate| candidate == layout),
    }
}

#[doc(hidden)]
pub fn crossover_input_layout(index: usize) -> Option<CrossoverInputLayout> {
    CROSSOVER_INPUT_LAYOUTS.get(index).copied()
}

#[doc(hidden)]
pub fn crossover_channel_to_sotf(
    input_layout_index: usize,
    api: PluginApi,
    channel: usize,
) -> Option<usize> {
    let layout = *CROSSOVER_INPUT_LAYOUTS.get(input_layout_index)?;
    if api == PluginApi::Clap {
        return (channel < layout.channel_count()).then_some(channel);
    }
    layout.vst3_bus_to_sotf_permutation().get(channel).copied()
}

#[doc(hidden)]
pub fn crossover_input_layout_count() -> usize {
    CROSSOVER_INPUT_LAYOUTS.len()
}

#[doc(hidden)]
pub fn crossover_configured_output_bands(params: &crate::params::DynamicParams) -> Option<usize> {
    let mode = params.value("mode")?.as_int()?;
    let topology = params.value("topology")?.as_int()?;
    let band_count = params.value("band_count")?.as_int()?;
    crossover_output_band_count(mode, topology, band_count)
}

#[doc(hidden)]
pub fn crossover_output_band_count(
    mode_index: i32,
    topology_index: i32,
    band_count_index: i32,
) -> Option<usize> {
    if !(0..=2).contains(&mode_index) || !(0..=1).contains(&topology_index) {
        return None;
    }
    if mode_index == 2 && topology_index == 0 {
        usize::try_from(band_count_index)
            .ok()
            .and_then(|index| index.checked_add(2))
            .filter(|bands| (2..=4).contains(bands))
    } else {
        Some(1)
    }
}

#[doc(hidden)]
pub fn crossover_vst3_active_bus_mask(output_bands: usize) -> Option<u64> {
    (1..=4)
        .contains(&output_bands)
        .then(|| (1_u64 << output_bands) - 1)
}

#[doc(hidden)]
pub fn crossover_vst3_active_buses_support_structure(
    active_buses: u64,
    mode_index: i32,
    topology_index: i32,
    band_count_index: i32,
) -> bool {
    if active_buses & 1 == 0 || active_buses & !0b1111 != 0 {
        return false;
    }
    crossover_output_band_count(mode_index, topology_index, band_count_index)
        .and_then(crossover_vst3_active_bus_mask)
        .is_some_and(|expected_buses| active_buses == expected_buses)
}

#[doc(hidden)]
pub fn crossover_vst3_active_buses_match_params(
    params: &crate::params::DynamicParams,
    active_buses: u64,
) -> bool {
    let Some(mode) = params.value("mode").and_then(|value| value.as_int()) else {
        return false;
    };
    let Some(topology) = params.value("topology").and_then(|value| value.as_int()) else {
        return false;
    };
    let Some(band_count) = params.value("band_count").and_then(|value| value.as_int()) else {
        return false;
    };
    crossover_vst3_active_buses_support_structure(active_buses, mode, topology, band_count)
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
    if layout_index >= CROSSOVER_VST3_LAYOUTS.len() || (is_input && bus_index != 0) {
        return None;
    }
    if !is_input && bus_index >= 4 {
        return None;
    }
    let layout = *CROSSOVER_INPUT_LAYOUTS.get(layout_index)?;
    Some(match layout.vst3_speaker_arrangement() {
        Some(arrangement) => arrangement,
        None => match layout.channel_count() {
            1 => 0b1,
            2 => band_split_vst3_stereo_arrangement(),
            4 => 0b1111,
            _ => return None,
        },
    })
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
    ("EQ", $default:expr) => {
        &$crate::wrapper::EQ_NATIVE_LAYOUTS
    };
    ("BandSplit", $default:expr) => {
        &$crate::wrapper::BAND_SPLIT_VST3_LAYOUTS
    };
    ("Crossover", $default:expr) => {
        &$crate::wrapper::CROSSOVER_VST3_LAYOUTS
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
    ("DeEsser", $default:expr) => {{
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

#[doc(hidden)]
#[macro_export]
macro_rules! sotf_nih_supports_surround {
    ("AmbisonicsDecoder") => {
        true
    };
    ("EQ") => {
        true
    };
    ("Crossover") => {
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

#[cfg(feature = "convolution")]
pub(crate) fn convolution_reactivation_is_compatible(
    prepared_geometry: Option<native_convolution_editor::Geometry>,
    requested_geometry: native_convolution_editor::Geometry,
    prepared_structural_fingerprint: u64,
    requested_structural_fingerprint: u64,
    prepared_non_restartable_fingerprint: u64,
    requested_non_restartable_fingerprint: u64,
) -> bool {
    prepared_geometry == Some(requested_geometry)
        && prepared_structural_fingerprint == requested_structural_fingerprint
        && prepared_non_restartable_fingerprint == requested_non_restartable_fingerprint
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
            ambisonics_custom_vst3_to_sotf:
                [usize; sotf_host::external_plugin::MAX_AMBISONICS_CUSTOM_SPEAKERS],
            ambisonics_custom_vst3_channels: usize,
            band_split_active_output_buses: u64,
            band_split_last_vst3_output_buses: Option<(usize, u64)>,
            band_split_vst3_legacy_packed: bool,
            crossover_input_layout_index: usize,
            crossover_active_output_buses: u64,
            crossover_host_to_sotf: [usize; 16],
            eq_native_host_to_sotf: [usize; 16],
            sample_rate: f64,
            structural_fingerprint: u64,
            non_restartable_structural_fingerprint: u64,
            hiss_momentary: $crate::params::hiss_profile::HissMomentaryLatch,
            transport: $crate::wrapper::transport::TransportTracker,
            #[cfg(feature = "convolution")]
            convolution_editor_service:
                std::sync::Arc<$crate::wrapper::native_convolution_editor::ConvolutionEditorService>,
            #[cfg(feature = "convolution")]
            convolution_prepared_geometry:
                Option<$crate::wrapper::native_convolution_editor::Geometry>,
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
                        48000.0,
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

                if matches!($plugin_type, "EQ") {
                    for info in $crate::params::native_eq_pair_route_param_infos() {
                        if !infos.iter().any(|existing| existing.id == info.id) {
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
                    ambisonics_custom_vst3_to_sotf:
                        [0; sotf_host::external_plugin::MAX_AMBISONICS_CUSTOM_SPEAKERS],
                    ambisonics_custom_vst3_channels: 0,
                    band_split_active_output_buses: 0,
                    band_split_last_vst3_output_buses: None,
                    band_split_vst3_legacy_packed: false,
                    crossover_input_layout_index: 0,
                    crossover_active_output_buses: 1,
                    crossover_host_to_sotf: [0; 16],
                    eq_native_host_to_sotf: [0; 16],
                    sample_rate: 48000.0,
                    structural_fingerprint: 0,
                    non_restartable_structural_fingerprint: 0,
                    hiss_momentary: $crate::params::hiss_profile::HissMomentaryLatch::default(),
                    transport: $crate::wrapper::transport::TransportTracker::default(),
                    #[cfg(feature = "convolution")]
                    convolution_editor_service: std::sync::Arc::default(),
                    #[cfg(feature = "convolution")]
                    convolution_prepared_geometry: None,
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
            type BackgroundTask = $crate::sotf_nih_background_task_type!($plugin_type);

            fn params(&self) -> std::sync::Arc<dyn nih_plug::prelude::Params> {
                self.params.clone()
            }

            #[cfg(feature = "convolution")]
            fn task_executor(&mut self) -> nih_plug::plugin::TaskExecutor<Self> {
                $crate::sotf_nih_task_executor!(
                    $plugin_type,
                    self.params,
                    self.convolution_editor_service
                )
            }

            fn editor(
                &mut self,
                async_executor: nih_plug::prelude::AsyncExecutor<Self>,
            ) -> Option<Box<dyn nih_plug::prelude::Editor>> {
                $crate::sotf_nih_create_editor!(
                    $plugin_type,
                    Self,
                    self.params,
                    self.convolution_editor_service,
                    async_executor
                )
            }

            fn filter_state(state: &mut nih_plug::wrapper::state::PluginState) {
                if matches!($plugin_type, "BandSplit") {
                    $crate::wrapper::migrate_band_split_state(state);
                } else if matches!($plugin_type, "Crossover") {
                    $crate::wrapper::migrate_crossover_state(state);
                } else if matches!($plugin_type, "EQ") {
                    $crate::params::migrate_eq_native_state(state);
                } else if matches!($plugin_type, "HissReducer") {
                    $crate::params::scrub_hiss_momentary_state(state);
                }
            }

            fn state_restore_allows_audio_thread(
                state: &nih_plug::wrapper::state::PluginState,
            ) -> bool {
                if matches!($plugin_type, "EQ") {
                    return $crate::params::eq_state_restore_allows_audio_thread(state);
                }
                if matches!($plugin_type, "SpeechDenoiser") {
                    return $crate::params::speech_state_restore_allows_audio_thread(state);
                }
                true
            }

            fn initialize(
                &mut self,
                audio_io_layout: &nih_plug::prelude::AudioIOLayout,
                buffer_config: &nih_plug::prelude::BufferConfig,
                context: &mut impl nih_plug::prelude::InitContext<Self>,
            ) -> bool {
                let mut eq_pair_apply_attempt = matches!($plugin_type, "EQ")
                    .then(|| $crate::params::EqPairApplyAttempt::new(self.params.clone()));
                let candidate_sample_rate = f64::from(buffer_config.sample_rate);
                if !candidate_sample_rate.is_finite() || candidate_sample_rate <= 0.0 {
                    return false;
                }
                if !matches!($plugin_type, "EQ") {
                    self.sample_rate = candidate_sample_rate;
                }
                #[cfg(feature = "convolution")]
                let convolution_editor_generation = if matches!($plugin_type, "Convolution") {
                    self.params.convolution_pending_editor_generation()
                } else {
                    None
                };
                #[cfg(not(feature = "convolution"))]
                let convolution_editor_generation: Option<u64> = None;
                let mut convolution_restore_attempt =
                    (matches!($plugin_type, "Convolution")
                        && convolution_editor_generation.is_none())
                    .then(|| {
                        $crate::params::ConvolutionRestoreAttempt::new(self.params.clone())
                    });
                let mut hiss_restore_attempt = matches!($plugin_type, "HissReducer")
                    .then(|| $crate::params::HissProfileRestoreAttempt::new(self.params.clone()));
                let mut ambisonics_custom_restore_attempt =
                    matches!($plugin_type, "AmbisonicsDecoder").then(|| {
                        $crate::params::AmbisonicsCustomRestoreAttempt::new(self.params.clone())
                    });
                let mut band_split_vst3_output_buses = None;
                let mut band_split_vst3_layout_index = None;
                let mut band_split_vst3_legacy_packed = false;
                let crossover_input_layout_index = if matches!($plugin_type, "Crossover") {
                    $crate::wrapper::crossover_input_layout_index(
                        audio_io_layout,
                        context.plugin_api(),
                    )
                } else {
                    None
                };
                let eq_native_input_layout_index = if matches!($plugin_type, "EQ") {
                    $crate::wrapper::eq_native_layout_index(audio_io_layout, context.plugin_api())
                } else {
                    None
                };
                if matches!($plugin_type, "EQ") && eq_native_input_layout_index.is_none() {
                    return false;
                }
                if matches!($plugin_type, "Crossover") && crossover_input_layout_index.is_none() {
                    return false;
                }
                let crossover_output_bands = if matches!($plugin_type, "Crossover") {
                    let Some(output_bands) =
                        $crate::wrapper::crossover_configured_output_bands(&self.params)
                    else {
                        return false;
                    };
                    Some(output_bands)
                } else {
                    None
                };
                let mut crossover_active_output_buses = None;
                if matches!($plugin_type, "Crossover") {
                    let input_channels = audio_io_layout
                        .main_input_channels
                        .map_or(0, |channels| channels.get() as usize);
                    let Some(output_bands) = crossover_output_bands else {
                        return false;
                    };
                    match context.plugin_api() {
                        nih_plug::context::PluginApi::Clap => {
                            let Some(layout_output_channels) =
                                $crate::wrapper::crossover_clap_output_channels(audio_io_layout)
                            else {
                                return false;
                            };
                            if layout_output_channels != input_channels * output_bands {
                                return false;
                            }
                        }
                        nih_plug::context::PluginApi::Vst3
                        | nih_plug::context::PluginApi::Standalone => {
                            let Some(expected_buses) =
                                $crate::wrapper::crossover_vst3_active_bus_mask(output_bands)
                            else {
                                return false;
                            };
                            let active_buses = context
                                .vst3_active_audio_output_buses()
                                .unwrap_or(expected_buses);
                            if !$crate::wrapper::crossover_vst3_active_buses_match_params(
                                &self.params,
                                active_buses,
                            ) {
                                return false;
                            }
                            crossover_active_output_buses = Some(active_buses);
                        }
                    }
                }
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
                    && crossover_input_layout_index.is_none()
                    && eq_native_input_layout_index.is_none()
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
                    // VST3 and Standalone both resolve layouts against
                    // the 8-target VST3 table; only CLAP uses 6 targets.
                    let target_count = if context.plugin_api()
                        == nih_plug::context::PluginApi::Clap
                    {
                        6
                    } else {
                        8
                    };
                    let order = layout_index / target_count + 1;
                    let target_layout = layout_index % target_count;
                    if self.params.value("target_layout")
                        == Some(sotf_host::parameters::ParameterValue::Int(
                            $crate::params::ambisonics_custom::AMBISONICS_CUSTOM_TARGET_INDEX as i32,
                        ))
                    {
                        let output_channels = audio_io_layout
                            .main_output_channels
                            .map_or(0, |channels| channels.get() as usize);
                        let Some(permutation) =
                            $crate::wrapper::negotiate_ambisonics_custom_layout(
                                &self.params,
                                order,
                                target_layout,
                                layout_index,
                                output_channels,
                                context.plugin_api(),
                            )
                        else {
                            return false;
                        };
                        if permutation.len() != output_channels
                            || output_channels
                                > sotf_host::external_plugin::MAX_AMBISONICS_CUSTOM_SPEAKERS
                        {
                            return false;
                        }
                        self.ambisonics_custom_vst3_to_sotf[..output_channels]
                            .copy_from_slice(&permutation);
                        self.ambisonics_custom_vst3_channels = output_channels;
                        self.ambisonics_target_layout =
                            $crate::params::ambisonics_custom::AMBISONICS_CUSTOM_TARGET_INDEX;
                    } else {
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
                }
                let candidate_main_input_channels = audio_io_layout
                    .main_input_channels
                    .map_or(0, |channels| channels.get() as usize);
                if !matches!($plugin_type, "EQ") {
                    self.main_input_channels = candidate_main_input_channels;
                }
                if matches!($plugin_type, "EQ")
                    && (audio_io_layout.main_output_channels.map_or(0, |channels| channels.get() as usize)
                        != candidate_main_input_channels
                        || !audio_io_layout.aux_input_ports.is_empty()
                        || !audio_io_layout.aux_output_ports.is_empty()
                        || !(1..=16).contains(&candidate_main_input_channels))
                {
                    return false;
                }
                let mut crossover_host_to_sotf = [0; 16];
                if let Some(layout_index) = crossover_input_layout_index {
                    if self.main_input_channels > crossover_host_to_sotf.len() {
                        return false;
                    }
                    for host_channel in 0..self.main_input_channels {
                        let Some(sotf_channel) = $crate::wrapper::crossover_channel_to_sotf(
                            layout_index,
                            context.plugin_api(),
                            host_channel,
                        ) else {
                            return false;
                        };
                        crossover_host_to_sotf[host_channel] = sotf_channel;
                    }
                }
                let mut eq_native_host_to_sotf = [0; 16];
                if let Some(layout_index) = eq_native_input_layout_index {
                    if candidate_main_input_channels > eq_native_host_to_sotf.len() {
                        return false;
                    }
                    for host_channel in 0..candidate_main_input_channels {
                        let Some(sotf_channel) = $crate::wrapper::eq_native_channel_to_sotf(
                            layout_index,
                            context.plugin_api(),
                            host_channel,
                        ) else {
                            return false;
                        };
                        eq_native_host_to_sotf[host_channel] = sotf_channel;
                    }
                }
                let eq_pair_route = if matches!($plugin_type, "EQ") {
                    self.params
                        .eq_pair_route_for_initialization(candidate_main_input_channels)
                        .ok()
                } else {
                    None
                };
                if matches!($plugin_type, "EQ") && eq_pair_route.is_none() {
                    return false;
                }
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
                let total_inputs = audio_io_layout
                    .main_input_channels
                    .map_or(0, |channels| channels.get() as usize)
                    + audio_io_layout
                        .aux_input_ports
                        .iter()
                        .map(|channels| channels.get() as usize)
                        .sum::<usize>();
                let max_frames = buffer_config.max_buffer_size as usize;

                #[cfg(feature = "convolution")]
                let convolution_geometry =
                    $crate::wrapper::native_convolution_editor::Geometry {
                        sample_rate: candidate_sample_rate,
                        max_frames,
                        input_channels: total_inputs,
                        output_channels: total_declared_outputs,
                    };

                // VST3 reactivation normally rebuilds the native DSP. For an
                // unchanged Convolution instance, retain its prepared IR so
                // reset does not reopen the user's source file. A failed or
                // pending resource restore, topology change, or host geometry
                // change must still take the detached-candidate path below.
                #[cfg(feature = "convolution")]
                if matches!($plugin_type, "Convolution")
                    && convolution_editor_generation.is_none()
                    && $crate::wrapper::convolution_reactivation_is_compatible(
                        self.convolution_prepared_geometry,
                        convolution_geometry,
                        self.structural_fingerprint,
                        self.params.structural_fingerprint(),
                        self.non_restartable_structural_fingerprint,
                        self.params.non_restartable_structural_fingerprint(),
                    )
                    && let Some(plugin) = self.inner.as_mut()
                    && plugin.input_channels() == total_inputs
                    && plugin.output_channels() == total_declared_outputs
                    && self
                        .params
                        .convolution_prepared_resource_matches(plugin.as_ref())
                {
                    if let Err(error) = self
                        .params
                        .validate_convolution_realtime_values(plugin.as_ref())
                    {
                        log::error!(
                            "Failed to validate Convolution reactivation parameters: {error}"
                        );
                        return false;
                    }
                    let latency = match u32::try_from(plugin.latency_samples()) {
                        Ok(latency) => latency,
                        Err(_) => {
                            log::error!("Convolution latency does not fit the host ABI");
                            return false;
                        }
                    };
                    if let Err(error) = plugin.initialize(candidate_sample_rate) {
                        log::error!("Failed to reinitialize prepared Convolution: {error}");
                        return false;
                    }
                    if let Err(error) = self.params.sync_to_plugin(plugin.as_mut()) {
                        // Every scalar value was validated above. This branch
                        // protects the host contract if a future Convolution
                        // setter adds a new state-dependent rejection.
                        log::error!("Failed to sync Convolution reactivation parameters: {error}");
                        return false;
                    }
                    context.set_latency_samples(latency);
                    self.max_frames = max_frames;
                    self.main_output_channels = main_output_channels;
                    self.aux_output_channels = aux_output_channels;
                    self.aux_output_count = audio_io_layout.aux_output_ports.len();
                    if let Some(attempt) = convolution_restore_attempt.as_mut() {
                        attempt.commit(candidate_sample_rate);
                    }
                    self.convolution_editor_service
                        .complete_initialization(convolution_geometry, None);
                    self.structural_fingerprint = self.params.structural_fingerprint();
                    self.non_restartable_structural_fingerprint = self
                        .params
                        .non_restartable_structural_fingerprint();
                    self.transport = $crate::wrapper::transport::TransportTracker::default();
                    return true;
                }

                #[cfg(feature = "convolution")]
                let plugin_result = if let Some(generation) = convolution_editor_generation {
                    self.convolution_editor_service
                        .take_candidate(
                            generation,
                            $crate::wrapper::native_convolution_editor::Geometry {
                                sample_rate: candidate_sample_rate,
                                max_frames,
                                input_channels: total_inputs,
                                output_channels: total_declared_outputs,
                            },
                            self.params.convolution_editor_topology_fingerprint(),
                        )
                        .map(|plugin| (plugin, true))
                } else {
                    if matches!($plugin_type, "EQ") {
                        let Some((route, _)) = eq_pair_route.as_ref() else {
                            return false;
                        };
                        $crate::params::configuration::create_native_eq_plugin(
                            candidate_sample_rate,
                            &self.params,
                            candidate_main_input_channels,
                            route,
                        )
                    } else if matches!($plugin_type, "Crossover") {
                        $crate::params::configuration::create_plugin_with_input_channels(
                            $plugin_type,
                            candidate_sample_rate,
                            &self.params,
                            candidate_main_input_channels,
                        )
                    } else {
                        $crate::params::configuration::create_plugin(
                            $plugin_type,
                            candidate_sample_rate,
                            &self.params,
                        )
                    }
                    .map(|plugin| (plugin, false))
                };
                #[cfg(not(feature = "convolution"))]
                let plugin_result = if matches!($plugin_type, "EQ") {
                    let Some((route, _)) = eq_pair_route.as_ref() else {
                        return false;
                    };
                    $crate::params::configuration::create_native_eq_plugin(
                        candidate_sample_rate,
                        &self.params,
                        candidate_main_input_channels,
                        route,
                    )
                } else if matches!($plugin_type, "Crossover") {
                    $crate::params::configuration::create_plugin_with_input_channels(
                        $plugin_type,
                        candidate_sample_rate,
                        &self.params,
                        candidate_main_input_channels,
                    )
                } else {
                    $crate::params::configuration::create_plugin(
                        $plugin_type,
                        candidate_sample_rate,
                        &self.params,
                    )
                }
                .map(|plugin| (plugin, false));

                match plugin_result {
                    Ok((mut plugin, editor_candidate)) => {
                        let input_channels = plugin.input_channels();
                        let output_channels = plugin.output_channels();
                        let main_inputs = audio_io_layout.main_input_channels
                            .map_or(0, |channels| channels.get() as usize);
                        let uses_optional_band_buses = matches!($plugin_type, "BandSplit")
                            && context.plugin_api() == nih_plug::context::PluginApi::Vst3
                            && ($crate::wrapper::band_split_vst3_is_max_bus_layout(
                                audio_io_layout,
                            ) || band_split_vst3_legacy_packed);
                        let uses_optional_crossover_buses = matches!($plugin_type, "Crossover")
                            && context.plugin_api() == nih_plug::context::PluginApi::Vst3;
                        let output_width_matches = if uses_optional_band_buses {
                            band_split_num_bands
                                .is_some_and(|bands| {
                                    output_channels == bands * 2
                                        && output_channels
                                            >= audio_io_layout.main_output_channels.map_or(0, |v| v.get() as usize)
                                })
                                && output_channels <= total_declared_outputs
                        } else if uses_optional_crossover_buses {
                            crossover_output_bands
                                .is_some_and(|bands| output_channels == input_channels * bands)
                                && output_channels
                                    >= audio_io_layout.main_output_channels.map_or(0, |v| v.get() as usize)
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
                        let active_crossover_buses_match = crossover_output_bands
                            .and_then($crate::wrapper::crossover_vst3_active_bus_mask)
                            .is_some_and(|expected| {
                                crossover_active_output_buses == Some(expected)
                            });
                        // Gate and De-esser internal detectors ignore the optional key bus.
                        // External detection requires that the host selected it.
                        let ignores_key_bus = matches!($plugin_type, "Gate" | "DeEsser")
                            && input_channels == main_inputs;
                        if (input_channels != total_inputs && !ignores_key_bus)
                            || !output_width_matches
                            || (uses_optional_band_buses && !active_band_buses_match)
                            || (uses_optional_crossover_buses && !active_crossover_buses_match)
                        {
                            log::error!(
                                "{} DSP channels do not match the declared host layout",
                                $plugin_type
                            );
                            return false;
                        }

                        if !editor_candidate {
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
                                    candidate_sample_rate,
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
                            } else if let Err(e) = plugin.initialize(candidate_sample_rate) {
                                log::error!("Failed to initialize {}: {e}", $plugin_type);
                                return false;
                            }
                        }

                        // Validate the complete saved state on the control thread.
                        // Realtime values may depend on structural settings, such
                        // as the limiter requiring a fully wet mix in ISP mode.
                        if let Err(error) = self
                            .params
                            .sync_to_plugin_for_activation(plugin.as_mut(), candidate_sample_rate)
                        {
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
                        let interleaved_in = vec![0.0; max_frames * input_channels];
                        let interleaved_out = vec![0.0; max_frames * output_channels];
                        if let Some((route, publish_draft)) = &eq_pair_route
                            && let Err(error) = self
                                .params
                                .complete_eq_pair_route(route.clone(), *publish_draft)
                        {
                            log::error!("Failed to commit native EQ stereo-pair route: {error}");
                            return false;
                        }

                        context.set_latency_samples(latency);
                        self.interleaved_in = interleaved_in;
                        self.interleaved_out = interleaved_out;
                        self.max_frames = max_frames;
                        self.sample_rate = candidate_sample_rate;
                        self.main_input_channels = candidate_main_input_channels;
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
                        if let Some(layout_index) = crossover_input_layout_index {
                            self.crossover_input_layout_index = layout_index;
                            self.crossover_host_to_sotf = crossover_host_to_sotf;
                        }
                        if eq_native_input_layout_index.is_some() {
                            self.eq_native_host_to_sotf = eq_native_host_to_sotf;
                        }
                        if let Some(active_buses) = crossover_active_output_buses {
                            self.crossover_active_output_buses = active_buses;
                        }
                        if convolution_editor_generation.is_some() {
                            self.params
                                .complete_convolution_state_restore(candidate_sample_rate);
                            #[cfg(feature = "convolution")]
                            if let Some(generation) = convolution_editor_generation {
                                self.convolution_editor_service.mark_applied(generation);
                            }
                        } else if let Some(attempt) = convolution_restore_attempt.as_mut() {
                            attempt.commit(candidate_sample_rate);
                        }
                        if let Some(attempt) = hiss_restore_attempt.as_mut() {
                            attempt.commit();
                        }
                        if let Some(attempt) = ambisonics_custom_restore_attempt.as_mut() {
                            attempt.commit();
                        }
                        #[cfg(feature = "convolution")]
                        if matches!($plugin_type, "Convolution") {
                            self.convolution_editor_service.complete_initialization(
                                convolution_geometry,
                                self.params.convolution_pending_editor_generation(),
                            );
                            self.convolution_prepared_geometry = Some(convolution_geometry);
                        }
                        self.structural_fingerprint = self.params.structural_fingerprint();
                        self.non_restartable_structural_fingerprint = self
                            .params
                            .non_restartable_structural_fingerprint();
                        self.inner = Some(plugin);
                        if matches!($plugin_type, "HissReducer")
                            && let Some(inner) = self.inner.as_ref()
                            && let Some(snapshot) =
                                $crate::params::hiss_profile::hiss_snapshot(inner.as_ref())
                        {
                            self.params.install_hiss_snapshot(snapshot);
                        }
                        if matches!($plugin_type, "HissReducer")
                            && let Some(inner) = self.inner.as_ref()
                        {
                            let learn_id =
                                sotf_host::parameters::ParameterId::from("learn_noise");
                            let clear_id =
                                sotf_host::parameters::ParameterId::from("clear_profile");
                            self.hiss_momentary.learn_immediate = inner
                                .supports_immediate_momentary_control(&learn_id);
                            self.hiss_momentary.clear_immediate = inner
                                .supports_immediate_momentary_control(&clear_id);
                        }
                        if let Some(attempt) = eq_pair_apply_attempt.as_mut() {
                            attempt.commit();
                        }
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
                let optional_crossover_buses = matches!($plugin_type, "Crossover")
                    && plugin_api == nih_plug::context::PluginApi::Vst3
                    && self.aux_output_count == 3;
                let optional_output_buses = optional_band_buses || optional_crossover_buses;
                let active_output_buses = if optional_crossover_buses {
                    self.crossover_active_output_buses
                } else {
                    self.band_split_active_output_buses
                };
                let invalid_aux_output = aux.outputs.len() != self.aux_output_count
                    || aux.outputs.iter().enumerate().any(|(index, output_bus)| {
                        if optional_output_buses {
                            let slices = output_bus.as_slice_immutable();
                            slices.len() != self.aux_output_channels[index]
                                || !(slices.iter().all(|slice| slice.len() == num_frames)
                                    || slices.iter().all(|slice| slice.is_empty()))
                        } else {
                            output_bus.channels() != self.aux_output_channels[index]
                                || output_bus.samples() != num_frames
                        }
                    })
                    || (optional_output_buses
                        && (active_output_buses & 1 == 0
                            || buffer
                                .as_slice_immutable()
                                .iter()
                                .take(self.main_output_channels)
                                .any(|slice| slice.len() != num_frames)
                            || aux.outputs.iter().enumerate().any(|(index, output_bus)| {
                                let bus_index = index + 1;
                                let active = active_output_buses & (1_u64 << bus_index) != 0;
                                let source_offset = if optional_crossover_buses {
                                    Some(bus_index * self.aux_output_channels[index])
                                } else {
                                    $crate::wrapper::band_split_vst3_output_channel_offset(
                                        self.band_split_vst3_legacy_packed,
                                        bus_index,
                                    )
                                };
                                let legacy_reserved_bus = optional_band_buses
                                    && self.band_split_vst3_legacy_packed
                                    && $crate::wrapper::band_split_vst3_is_legacy_reserved_bus(
                                        bus_index,
                                    );
                                active
                                    && (output_bus
                                        .as_slice_immutable()
                                        .iter()
                                        .any(|slice| slice.len() != num_frames)
                                        || (!legacy_reserved_bus
                                            && source_offset.map_or(true, |offset| {
                                                offset + self.aux_output_channels[index]
                                                    > output_channels
                                            })))
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
                        if matches!($plugin_type, "DynamicEQ") {
                            self.params.non_restartable_structural_fingerprint()
                                != self.non_restartable_structural_fingerprint
                        } else if matches!($plugin_type, "Convolution") {
                            #[cfg(feature = "convolution")]
                            {
                                !self
                                    .convolution_editor_service
                                    .allows_old_prepared_audio_for(
                                        self.params.structural_fingerprint(),
                                    )
                            }
                            #[cfg(not(feature = "convolution"))]
                            {
                                true
                            }
                        } else {
                            true
                        };
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

                // Hiss host actions: edge-consume the visible momentary
                // controls into the DSP immediate setters. No allocation,
                // lock, or rebuild; a rejected forward fails this block.
                if matches!($plugin_type, "HissReducer")
                    && self
                        .params
                        .forward_hiss_momentary_edges(plugin.as_mut(), &mut self.hiss_momentary)
                        .is_err()
                {
                    $crate::wrapper::silence_host_outputs(buffer, aux);
                    return nih_plug::prelude::ProcessStatus::Error("Hiss capture control failed");
                }

                // Interleave main inputs and any additional input bus.
                let channel_slices = buffer.as_slice();
                for frame in 0..num_frames {
                    if matches!($plugin_type, "Crossover" | "EQ") {
                        for host_channel in 0..input_channels {
                            let sotf_channel = if matches!($plugin_type, "Crossover") {
                                self.crossover_host_to_sotf[host_channel]
                            } else {
                                self.eq_native_host_to_sotf[host_channel]
                            };
                            self.interleaved_in[frame * input_channels + sotf_channel] =
                                channel_slices[host_channel][frame];
                        }
                    } else if ambisonics {
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
                        let source_channel = if matches!($plugin_type, "Crossover") {
                            let width = self.main_input_channels;
                            let host_channel = ch % width;
                            let band = ch / width;
                            band * width + self.crossover_host_to_sotf[host_channel]
                        } else if matches!($plugin_type, "EQ") {
                            self.eq_native_host_to_sotf[ch]
                        } else if ambisonics
                            && plugin_api == nih_plug::context::PluginApi::Vst3
                        {
                            match $crate::wrapper::ambisonics_vst3_output_to_sotf_custom(
                                self.ambisonics_target_layout,
                                self.ambisonics_custom_vst3_channels,
                                &self.ambisonics_custom_vst3_to_sotf,
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
                    if optional_output_buses {
                        let active = active_output_buses & (1_u64 << bus_index_vst) != 0;
                        if !active {
                            continue;
                        }
                        let bus_slices = output_bus.as_slice();
                        let source_offset = if optional_crossover_buses {
                            Some(bus_index_vst * bus_channels)
                        } else {
                            $crate::wrapper::band_split_vst3_output_channel_offset(
                                self.band_split_vst3_legacy_packed,
                                bus_index_vst,
                            )
                        };
                        if let Some(source_offset) = source_offset {
                            for frame in 0..num_frames {
                                for channel in 0..bus_channels {
                                    let source_channel = if optional_crossover_buses {
                                        self.crossover_host_to_sotf[channel]
                                    } else {
                                        channel
                                    };
                                    bus_slices[channel][frame] = self.interleaved_out
                                        [frame * output_channels + source_offset + source_channel];
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
                if !optional_output_buses && output_channel_offset != output_channels {
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
                matches!(
                    $plugin_type,
                    "DynamicEQ" | "Convolution" | "EQ" | "DeEsser"
                )
            }

            fn default_audio_io_layout() -> nih_plug::prelude::AudioIOLayout {
                // VST3 discovers buses from its initial layout. CLAP retains
                // its legacy stereo config ID and explicitly selects keys.
                let layouts = <Self as nih_plug::prelude::Plugin>::AUDIO_IO_LAYOUTS;
                if matches!($plugin_type, "BandSplit") {
                    layouts[1]
                } else if matches!($plugin_type, "Crossover") {
                    layouts[0]
                } else {
                    layouts[if matches!($plugin_type, "Gate" | "DeEsser") {
                        1
                    } else {
                        0
                    }]
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
                } else if matches!($plugin_type, "Crossover") {
                    $crate::wrapper::crossover_vst3_bus_arrangement(
                        layout_index,
                        is_input,
                        bus_index,
                    )
                } else if matches!($plugin_type, "EQ") {
                    $crate::wrapper::crossover_vst3_bus_arrangement(
                        layout_index,
                        is_input,
                        bus_index,
                    )
                } else {
                    None
                }
            }

            fn vst3_audio_bus_default_active(is_input: bool, bus_index: usize) -> bool {
                if matches!($plugin_type, "Crossover") {
                    is_input || bus_index == 0
                } else {
                    !matches!($plugin_type, "BandSplit") || is_input || bus_index < 2
                }
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
                } else if matches!($plugin_type, "Crossover") {
                    if layout_index >= $crate::wrapper::crossover_input_layout_count() {
                        return false;
                    }
                    if is_input {
                        bus_index == 0
                    } else {
                        bus_index == 0
                    }
                } else {
                    Self::vst3_audio_bus_default_active(is_input, bus_index)
                }
            }

            fn vst3_allows_inactive_audio_output_buses() -> bool {
                matches!($plugin_type, "BandSplit" | "Crossover")
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
                $crate::sotf_nih_supports_surround!($plugin_type);

            fn clap_audio_io_layouts() -> &'static [nih_plug::prelude::AudioIOLayout] {
                if matches!($plugin_type, "AmbisonicsDecoder") {
                    &$crate::wrapper::AMBISONICS_CLAP_LAYOUTS
                } else if matches!($plugin_type, "BandSplit") {
                    &$crate::wrapper::BAND_SPLIT_CLAP_LAYOUTS
                } else if matches!($plugin_type, "Crossover") {
                    &$crate::wrapper::CROSSOVER_CLAP_LAYOUTS
                } else {
                    <Self as nih_plug::prelude::Plugin>::AUDIO_IO_LAYOUTS
                }
            }

            fn clap_audio_port_type(
                layout_index: usize,
                is_input: bool,
                port_index: usize,
            ) -> Option<&'static std::ffi::CStr> {
                if port_index != 0 {
                    return None;
                }
                if matches!($plugin_type, "AmbisonicsDecoder") {
                    Some(if is_input {
                        clap_sys::ext::ambisonic::CLAP_PORT_AMBISONIC
                    } else {
                        clap_sys::ext::surround::CLAP_PORT_SURROUND
                    })
                } else if matches!($plugin_type, "EQ")
                    && Self::clap_audio_io_layouts()
                        .get(layout_index)
                        .and_then(|layout| {
                            if is_input {
                                layout.main_input_channels
                            } else {
                                layout.main_output_channels
                            }
                        })
                        .is_some_and(|channels| channels.get() > 2)
                {
                    Some(clap_sys::ext::surround::CLAP_PORT_SURROUND)
                } else if matches!($plugin_type, "Crossover")
                    && is_input
                    && Self::clap_audio_io_layouts()
                        .get(layout_index)
                        .and_then(|layout| layout.main_input_channels)
                        .is_some_and(|channels| channels.get() > 2)
                {
                    Some(clap_sys::ext::surround::CLAP_PORT_SURROUND)
                } else {
                    None
                }
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
                } else if matches!($plugin_type, "Crossover") {
                    $crate::wrapper::crossover_clap_channel_map(
                        layout_index / 4,
                        is_input,
                        port_index,
                    )
                } else if matches!($plugin_type, "EQ") {
                    $crate::wrapper::crossover_clap_channel_map(
                        layout_index,
                        true,
                        port_index,
                    )
                } else {
                    None
                }
            }

            fn clap_surround_channel_mask_supported(channel_mask: u64) -> bool {
                matches!($plugin_type, "AmbisonicsDecoder")
                    && $crate::wrapper::ambisonics_clap_channel_mask_supported(channel_mask)
                    || (matches!($plugin_type, "Crossover")
                        && $crate::wrapper::crossover_clap_channel_mask_supported(channel_mask))
                    || (matches!($plugin_type, "EQ")
                        && $crate::wrapper::crossover_clap_channel_mask_supported(channel_mask))
            }
        }
    };
}

#[cfg(all(test, feature = "convolution"))]
mod convolution_reactivation_tests {
    use super::{convolution_reactivation_is_compatible, native_convolution_editor::Geometry};

    const GEOMETRY: Geometry = Geometry {
        sample_rate: 48_000.0,
        max_frames: 256,
        input_channels: 2,
        output_channels: 2,
    };

    #[test]
    fn prepared_convolution_reuse_rejects_geometry_and_structural_changes() {
        assert!(convolution_reactivation_is_compatible(
            Some(GEOMETRY),
            GEOMETRY,
            7,
            7,
            11,
            11,
        ));

        for changed in [
            Geometry {
                sample_rate: 44_100.0,
                ..GEOMETRY
            },
            Geometry {
                max_frames: 128,
                ..GEOMETRY
            },
            Geometry {
                input_channels: 1,
                ..GEOMETRY
            },
            Geometry {
                output_channels: 1,
                ..GEOMETRY
            },
        ] {
            assert!(!convolution_reactivation_is_compatible(
                Some(GEOMETRY),
                changed,
                7,
                7,
                11,
                11,
            ));
        }

        assert!(!convolution_reactivation_is_compatible(
            Some(GEOMETRY),
            GEOMETRY,
            7,
            8,
            11,
            11,
        ));
        assert!(!convolution_reactivation_is_compatible(
            Some(GEOMETRY),
            GEOMETRY,
            7,
            7,
            11,
            12,
        ));
        assert!(!convolution_reactivation_is_compatible(
            None, GEOMETRY, 7, 7, 11, 11,
        ));
    }
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

/// Build an EQ constructor config with committed native placement and pair
/// routing. An Inherit placement and a disabled pair route stay absent so old
/// presets retain their legacy channel behavior.
#[doc(hidden)]
pub fn eq_config_json_with_native_route(
    value: impl Fn(&str) -> Option<sotf_host::parameters::ParameterValue>,
    route: &crate::params::EqPairRoute,
) -> Result<String, String> {
    use sotf_host::parameters::ParameterValue;

    let mut config: serde_json::Value = serde_json::from_str(&eq_config_json(|id| value(id)))
        .map_err(|error| format!("EQ native configuration is invalid: {error}"))?;
    let filters = config
        .get_mut("filters")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| "EQ native configuration is missing its filter array".to_string())?;
    for (band, filter) in filters.iter_mut().enumerate() {
        let placement_id = format!("filter_{band}_placement");
        let placement = match value(&placement_id) {
            Some(ParameterValue::Int(index)) => index,
            None => 0,
            Some(_) => {
                return Err(format!(
                    "EQ placement parameter '{placement_id}' is not an integer"
                ));
            }
        };
        let placement_name = match placement {
            0 => None,
            1 => Some("stereo"),
            2 => Some("left"),
            3 => Some("right"),
            4 => Some("mid"),
            5 => Some("side"),
            index => {
                return Err(format!(
                    "EQ placement choice {index} is outside 0..=5 for band {band}"
                ));
            }
        };
        let object = filter
            .as_object_mut()
            .ok_or_else(|| format!("EQ filter {band} is not an object"))?;
        if let Some(name) = placement_name {
            object.insert(
                "placement".to_string(),
                serde_json::Value::String(name.to_string()),
            );
        } else {
            object.remove("placement");
        }
    }
    let object = config
        .as_object_mut()
        .ok_or_else(|| "EQ native configuration is not an object".to_string())?;
    if route.enabled {
        if route.pairs.is_empty() || route.pairs.len() > 8 {
            return Err("EQ native stereo-pair route must contain 1..=8 pairs".to_string());
        }
        let mut used = 0_u16;
        for [first, second] in &route.pairs {
            if *first >= 16 || *second >= 16 || first == second {
                return Err(format!("EQ stereo pair [{first}, {second}] is invalid"));
            }
            let pair_mask = (1_u16 << first) | (1_u16 << second);
            if used & pair_mask != 0 {
                return Err(format!(
                    "EQ stereo pair [{first}, {second}] overlaps another pair"
                ));
            }
            used |= pair_mask;
        }
        object.insert(
            "stereo_pairs".to_string(),
            serde_json::to_value(&route.pairs)
                .map_err(|error| format!("EQ stereo-pair route: {error}"))?,
        );
    } else {
        object.remove("stereo_pairs");
    }
    serde_json::to_string(&config)
        .map_err(|error| format!("EQ native configuration is invalid: {error}"))
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
        // DeEsser choice controls (mode, split topology) are String-typed at
        // runtime; specs provide the stable integer Choice metadata.
        "DeEsser" => de_esser::PARAMS,
        "AnalogLimiter" => analog_limiter::PARAMS,
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
