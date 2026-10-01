//! Crossover plugin parameter definitions.
//!
//! Static ParamSpec metadata for documentation generation and UI descriptors.
//! Runtime parameter construction in `crossover_plugin.rs` is the source of
//! truth for the audio thread; this module mirrors the user-visible surface.

use sotf_host::param_specs::ParamSpec;
use sotf_host::plugin_layout::*;

pub const CROSSOVER_TYPES: &[&str] = &[
    "LR24",
    "LinearPhase",
    "LR12",
    "LR48",
    "BW6",
    "BW12",
    "BW18",
    "BW24",
    "BW30",
    "BW36",
    "BW42",
    "BW48",
    "Bessel12",
];

pub const PARAMS: &[ParamSpec] = &[
    ParamSpec::choice("Type", "type", 0, CROSSOVER_TYPES, "General")
        .structural()
        .setup()
        .doc("Crossover family: LR12/24/48, Butterworth 6–48 dB/octave, Bessel12, or linear-phase FIR. LR12 high output is polarity-inverted."),
    ParamSpec::float(
        "Frequency",
        "frequency",
        1000.0,
        20.0,
        20000.0,
        1.0,
        "Hz",
        "General",
    )
    .setup()
    .doc("Primary crossover frequency"),
    ParamSpec::choice(
        "Mode",
        "mode",
        0,
        &["Lowpass", "Highpass", "Both"],
        "General",
    )
    .setup()
    .structural()
    .doc("Output mode for the primary crossover; Both changes output channel width"),
    ParamSpec::int("FIR Taps", "fir_taps", 1025, 31, 16385, 2, "", "General")
        .structural()
        .setup()
        .doc("FIR length for linear-phase mode (odd values are rounded up)"),
    ParamSpec::choice("Topology", "topology", 0, &["Bands", "Per Channel"], "General")
        .structural()
        .setup()
        .doc("Selects multi-band cutoffs or independent per-channel crossovers"),
    ParamSpec::choice("Band Count", "band_count", 0, &["2", "3", "4"], "General")
        .structural()
        .setup()
        .doc("Number of output bands when topology is Bands"),
    ParamSpec::float(
        "Frequency 2",
        "frequency_2",
        3000.0,
        20.0,
        20000.0,
        1.0,
        "Hz",
        "General",
    )
    .structural()
    .setup()
    .doc("Second ordered crossover cutoff for 3-way or 4-way mode"),
    ParamSpec::float(
        "Frequency 3",
        "frequency_3",
        8000.0,
        20.0,
        20000.0,
        1.0,
        "Hz",
        "General",
    )
    .structural()
    .setup()
    .doc("Third ordered crossover cutoff for 4-way mode"),
];

/// Crossover: indices 0–3 preserve the original family/frequency/mode/taps order.
pub const LAYOUT: PluginLayout = PluginLayout {
    config: &[
        ControlSpec::button_set(0, CROSSOVER_TYPES),
        ControlSpec::button_set(2, &["Lowpass", "Highpass", "Both"]),
        ControlSpec::button_set(4, &["Bands", "Per Channel"]),
        ControlSpec::button_set(5, &["2", "3", "4"]),
    ],
    main: &[ControlGroup::new(
        "CROSSOVER",
        "CROSSOVER",
        &[
            ControlSpec::knob_large(1),
            ControlSpec::knob(6),
            ControlSpec::knob(7),
        ],
    )
    .with_layout(GroupLayoutHints::inferred().priority(1.0).keep_visible())],
    output: &[],
    tabs: &[TabSpec {
        name: "Linear Phase",
        controls: &[ControlSpec::knob(3)],
    }],
    visualizations: &[],
    column_constraints: &[
        ColumnConstraint::config(170.0, 0.65),
        ColumnConstraint::main(300.0),
    ],
    dynamic_sections: &[],
};
