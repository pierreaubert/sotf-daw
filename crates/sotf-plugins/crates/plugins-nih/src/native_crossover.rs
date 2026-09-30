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

pub const PARAMS: &[ParamSpec] = &[
    // Keep this as the first native parameter. Its ID, range, unit, default,
    // and linear normalized mapping match the pre-AUD142 native fallback.
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
    ParamSpec::int("FIR taps", "fir_taps", 255, 31, 16385, 2, "samples", "FIR").structural(),
    ParamSpec::choice("Topology", "topology", 0, TOPOLOGY_LABELS, "Crossover").structural(),
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
    ParamSpec::float(
        "Channel 1 frequency",
        "channel_frequency_0",
        1000.0,
        20.0,
        20000.0,
        1.0,
        "Hz",
        "Per-channel",
    )
    .structural(),
    ParamSpec::choice(
        "Channel 1 mode",
        "channel_mode_0",
        0,
        CHANNEL_MODE_LABELS,
        "Per-channel",
    )
    .structural(),
    ParamSpec::float(
        "Channel 2 frequency",
        "channel_frequency_1",
        3000.0,
        20.0,
        20000.0,
        1.0,
        "Hz",
        "Per-channel",
    )
    .structural(),
    ParamSpec::choice(
        "Channel 2 mode",
        "channel_mode_1",
        1,
        CHANNEL_MODE_LABELS,
        "Per-channel",
    )
    .structural(),
];

pub fn choice_labels(id: &str) -> Option<&'static [&'static str]> {
    match id {
        "family" => Some(sotf_plugins::param_specs::crossover::CROSSOVER_TYPES),
        "mode" => Some(MODE_LABELS),
        "topology" => Some(TOPOLOGY_LABELS),
        "band_count" => Some(BAND_COUNT_LABELS),
        "channel_mode_0" | "channel_mode_1" => Some(CHANNEL_MODE_LABELS),
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
            | "channel_mode_0"
            | "channel_mode_1"
    )
}
