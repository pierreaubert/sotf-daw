//! Fixed native controls for the Crossover wrapper.
//!
//! The engine plugin exposes topology-dependent string parameters. NIH needs a
//! stable parameter list before the selected route is constructed, so this
//! schema keeps native identities separate from the runtime parameter IDs.

use sotf_host::param_specs::ParamSpec;

pub const MODE_LABELS: &[&str] = &["Lowpass", "Highpass", "Both"];
pub const TOPOLOGY_LABELS: &[&str] = &["Bands", "Per-channel"];
pub const BAND_COUNT_LABELS: &[&str] = &["2 bands", "3 bands", "4 bands"];
pub const CHANNEL_MODE_LABELS: &[&str] = &["Lowpass", "Highpass", "Mute", "Passthrough"];

macro_rules! crossover_params {
    ($(($index:literal, $label:literal, $frequency:expr, $mode:expr)),+ $(,)?) => {
        &[
            // Keep this as the first native parameter. Its ID, range, unit,
            // default, and normalized mapping match the pre-AUD142 fallback.
            ParamSpec::float(
                "Frequency",
                "frequency",
                1000.0,
                20.0,
                20000.0,
                1.0,
                "Hz",
                "Crossover",
            ),
            ParamSpec::choice(
                "Family",
                "family",
                0,
                sotf_plugins::param_specs::crossover::CROSSOVER_TYPES,
                "Crossover",
            )
            .structural(),
            ParamSpec::choice("Output", "mode", 0, MODE_LABELS, "Crossover").structural(),
            ParamSpec::int("FIR taps", "fir_taps", 255, 31, 16385, 2, "samples", "FIR")
                .structural(),
            ParamSpec::choice("Topology", "topology", 0, TOPOLOGY_LABELS, "Crossover")
                .structural(),
            ParamSpec::choice(
                "Band count",
                "band_count",
                0,
                BAND_COUNT_LABELS,
                "Crossover",
            )
            .structural(),
            ParamSpec::float(
                "Frequency 2",
                "frequency_2",
                3000.0,
                20.0,
                20000.0,
                1.0,
                "Hz",
                "Bands",
            ),
            ParamSpec::float(
                "Frequency 3",
                "frequency_3",
                8000.0,
                20.0,
                20000.0,
                1.0,
                "Hz",
                "Bands",
            ),
            $(
                ParamSpec::float(
                    concat!("Channel ", $label, " frequency"),
                    concat!("channel_frequency_", stringify!($index)),
                    $frequency,
                    20.0,
                    20000.0,
                    1.0,
                    "Hz",
                    "Per-channel",
                )
                .structural(),
                ParamSpec::choice(
                    concat!("Channel ", $label, " mode"),
                    concat!("channel_mode_", stringify!($index)),
                    $mode,
                    CHANNEL_MODE_LABELS,
                    "Per-channel",
                )
                .structural(),
            )+
        ]
    };
}

pub const PARAMS: &[ParamSpec] = crossover_params!(
    (0, "1", 1000.0, 0),
    (1, "2", 3000.0, 1),
    (2, "3", 1000.0, 0),
    (3, "4", 1000.0, 0),
    (4, "5", 1000.0, 0),
    (5, "6", 1000.0, 0),
    (6, "7", 1000.0, 0),
    (7, "8", 1000.0, 0),
    (8, "9", 1000.0, 0),
    (9, "10", 1000.0, 0),
    (10, "11", 1000.0, 0),
    (11, "12", 1000.0, 0),
    (12, "13", 1000.0, 0),
    (13, "14", 1000.0, 0),
    (14, "15", 1000.0, 0),
    (15, "16", 1000.0, 0),
);

pub fn choice_labels(id: &str) -> Option<&'static [&'static str]> {
    match id {
        "family" => Some(sotf_plugins::param_specs::crossover::CROSSOVER_TYPES),
        "mode" => Some(MODE_LABELS),
        "topology" => Some(TOPOLOGY_LABELS),
        "band_count" => Some(BAND_COUNT_LABELS),
        id if channel_mode_index(id).is_some() => Some(CHANNEL_MODE_LABELS),
        _ => None,
    }
}

pub fn is_structural(id: &str) -> bool {
    matches!(
        id,
        "family"
            | "mode"
            | "fir_taps"
            | "topology"
            | "band_count"
            | "channel_frequency_0"
            | "channel_frequency_1"
    ) || channel_frequency_index(id).is_some()
        || channel_mode_index(id).is_some()
}

fn channel_frequency_index(id: &str) -> Option<usize> {
    id.strip_prefix("channel_frequency_")?
        .parse::<usize>()
        .ok()
        .filter(|index| *index < 16)
}

fn channel_mode_index(id: &str) -> Option<usize> {
    id.strip_prefix("channel_mode_")?
        .parse::<usize>()
        .ok()
        .filter(|index| *index < 16)
}
