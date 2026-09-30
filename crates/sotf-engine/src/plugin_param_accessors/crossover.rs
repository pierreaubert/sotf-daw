use sotf_plugins::BandSplitRecombinationMode;
use sotf_plugins::param_specs::{self};

pub(super) fn band_split_mode_to_index(mode: &BandSplitRecombinationMode) -> f64 {
    mode.index() as f64
}

pub(super) fn band_split_num_bands_to_index(num_bands: &usize) -> f64 {
    num_bands.saturating_sub(2).min(2) as f64
}

pub(super) fn crossover_types() -> &'static [&'static str] {
    param_specs::find_by_key(param_specs::band_split::PARAMS, "type").choice_labels()
}

pub(super) fn crossover_type_to_index(ct: &str) -> f64 {
    crossover_types()
        .iter()
        .position(|&c| c.eq_ignore_ascii_case(ct))
        .unwrap_or(0) as f64
}

pub(super) fn crossover_plugin_type_to_index(ct: &str) -> f64 {
    let canonical = sotf_plugins::plugin_crossover::canonical_crossover_type(ct)
        .unwrap_or("LR24");
    param_specs::crossover::CROSSOVER_TYPES
        .iter()
        .position(|family| family.eq_ignore_ascii_case(canonical))
        .unwrap_or(0) as f64
}

pub(super) fn index_to_crossover_plugin_type(index: f64) -> String {
    param_specs::crossover::CROSSOVER_TYPES
        .get(index as usize)
        .copied()
        .unwrap_or("LR24")
        .to_string()
}

pub(super) fn crossover_output_to_index(output: &str) -> f64 {
    if output.eq_ignore_ascii_case("high") || output.eq_ignore_ascii_case("highpass") {
        1.0
    } else if output.eq_ignore_ascii_case("both") {
        2.0
    } else {
        0.0
    }
}
