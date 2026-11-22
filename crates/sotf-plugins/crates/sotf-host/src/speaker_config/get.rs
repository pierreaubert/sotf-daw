use super::consts::CONFIG_1_0;
use super::consts::CONFIG_2_0;
use super::consts::CONFIG_2_1;
use super::consts::CONFIG_5_0;
use super::consts::CONFIG_5_1;
use super::consts::CONFIG_5_1_2;
use super::consts::CONFIG_5_1_4;
use super::consts::CONFIG_5_1_6;
use super::consts::CONFIG_7_0;
use super::consts::CONFIG_7_1;
use super::consts::CONFIG_7_1_2;
use super::consts::CONFIG_7_1_4;
use super::consts::CONFIG_9_1_4;
use super::consts::CONFIG_9_1_6;
use super::consts::CONFIG_COREAUDIO_ATMOS_5_1_2;
use super::consts::CONFIG_COREAUDIO_ATMOS_7_1_2;
use super::consts::CONFIG_COREAUDIO_ATMOS_9_1_6;
use super::types::MeterGroupSpec;
use super::types::SpeakerConfig;

/// Get speaker configuration by ID
pub fn get_speaker_config(id: &str) -> Option<&'static SpeakerConfig> {
    match id {
        "1.0" => Some(&CONFIG_1_0),
        "2.0" => Some(&CONFIG_2_0),
        "2.1" => Some(&CONFIG_2_1),
        "5.0" => Some(&CONFIG_5_0),
        "5.1" => Some(&CONFIG_5_1),
        "7.0" => Some(&CONFIG_7_0),
        "7.1" => Some(&CONFIG_7_1),
        "5.1.2" => Some(&CONFIG_5_1_2),
        "5.1.4" => Some(&CONFIG_5_1_4),
        "5.1.6" => Some(&CONFIG_5_1_6),
        "7.1.2" => Some(&CONFIG_7_1_2),
        "7.1.4" => Some(&CONFIG_7_1_4),
        "9.1.4" => Some(&CONFIG_9_1_4),
        "9.1.6" => Some(&CONFIG_9_1_6),
        "coreaudio_atmos_5.1.2" => Some(&CONFIG_COREAUDIO_ATMOS_5_1_2),
        "coreaudio_atmos_7.1.2" => Some(&CONFIG_COREAUDIO_ATMOS_7_1_2),
        "coreaudio_atmos_9.1.6" => Some(&CONFIG_COREAUDIO_ATMOS_9_1_6),
        _ => None,
    }
}

/// Get all available configuration IDs
pub fn get_available_configs() -> &'static [&'static str] {
    &[
        "1.0", "2.0", "2.1", "5.0", "5.1", "7.0", "7.1", "5.1.2", "5.1.4", "7.1.2", "7.1.4",
        "9.1.4", "9.1.6", "5.1.6",
    ]
}

/// Get speaker configuration by number of channels
/// Returns the most common configuration for the given channel count
pub fn get_speaker_config_by_channels(num_channels: usize) -> Option<&'static SpeakerConfig> {
    match num_channels {
        1 => Some(&CONFIG_1_0),
        2 => Some(&CONFIG_2_0),
        3 => Some(&CONFIG_2_1),
        5 => Some(&CONFIG_5_0),
        6 => Some(&CONFIG_5_1),
        7 => Some(&CONFIG_7_0),
        8 => Some(&CONFIG_7_1),    // Could also be 5.1.2, prefer 7.1
        10 => Some(&CONFIG_5_1_4), // Could also be 7.1.2, prefer 5.1.4
        12 => Some(&CONFIG_7_1_4), // Could also be 5.1.6, prefer 7.1.4
        14 => Some(&CONFIG_9_1_4),
        16 => Some(&CONFIG_9_1_6),
        _ => None,
    }
}

/// Speaker configuration IDs by legacy integer index.
///
/// v1 presets stored `speaker_config` as an integer index into this list.
/// The order is a frozen on-disk contract, not the current choice order
/// shown in parameter metadata, and must not change: every decoder of the
/// legacy numeric form resolves indices through this one table so the
/// plugin, engine settings, and chain layout tracking agree on the layout
/// a numeric preset selects.
pub const LEGACY_SPEAKER_CONFIG_IDS: &[&str] = &[
    "2.0", "5.0", "5.1", "7.0", "7.1", "7.1.2", "7.1.4", "9.1", "9.1.4", "9.1.6", "5.1.6",
];

/// Resolve a legacy numeric `speaker_config` index to its configuration ID.
pub fn speaker_config_id_from_legacy_index(index: u64) -> Option<&'static str> {
    LEGACY_SPEAKER_CONFIG_IDS
        .get(usize::try_from(index).ok()?)
        .copied()
}

/// Get meter groups for a speaker configuration ID, or generate fallback for unknown configs
pub fn get_meter_groups(config_id: &str) -> Option<&'static [MeterGroupSpec]> {
    get_speaker_config(config_id).map(|c| c.meter_groups)
}

/// Get meter groups by channel count, or None if unknown
pub fn get_meter_groups_by_channels(num_channels: usize) -> Option<&'static [MeterGroupSpec]> {
    get_speaker_config_by_channels(num_channels).map(|c| c.meter_groups)
}
