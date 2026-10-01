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
    let canonical = sotf_plugins::plugin_crossover::canonical_crossover_type(ct).unwrap_or("LR24");
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

pub(super) fn index_to_crossover_output(index: f64) -> String {
    match index.round() as i64 {
        1 => "highpass".to_string(),
        2 => "both".to_string(),
        _ => "lowpass".to_string(),
    }
}

fn inferred_band_count(extra_frequencies: &[f64]) -> usize {
    extra_frequencies.len().saturating_add(2).clamp(2, 4)
}

fn valid_band_cutoff(cutoff: f64) -> bool {
    cutoff.is_finite() && (20.0..=20_000.0).contains(&cutoff)
}

fn valid_core_band_cutoffs(primary_frequency: f64, extra_frequencies: &[f64]) -> bool {
    let mut previous = primary_frequency as f32;
    if !previous.is_finite() {
        return false;
    }
    for cutoff in extra_frequencies {
        let converted = *cutoff as f32;
        if !converted.is_finite() || previous >= converted {
            return false;
        }
        previous = converted;
    }
    true
}

fn materialize_crossover_cutoffs(
    primary_frequency: f64,
    extra_frequencies: &[f64],
    required_count: usize,
) -> Option<Vec<f64>> {
    let required_extra = required_count.checked_sub(2)?;
    if required_extra > 2 || !valid_band_cutoff(primary_frequency) {
        return None;
    }

    let mut candidate = extra_frequencies.to_vec();
    if candidate
        .iter()
        .take(required_extra)
        .any(|cutoff| !valid_band_cutoff(*cutoff))
        || std::iter::once(primary_frequency)
            .chain(candidate.iter().take(required_extra).copied())
            .collect::<Vec<_>>()
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
    {
        return None;
    }

    if candidate.len() < required_extra {
        let defaults = [
            param_specs::find_by_key(param_specs::crossover::PARAMS, "frequency_2").default_f64(),
            param_specs::find_by_key(param_specs::crossover::PARAMS, "frequency_3").default_f64(),
        ];
        let defaults_fit = (candidate.len()..required_extra).all(|index| {
            let previous = if index == 0 {
                primary_frequency
            } else {
                candidate
                    .get(index - 1)
                    .copied()
                    .unwrap_or(defaults[index - 1])
            };
            valid_band_cutoff(defaults[index]) && previous < defaults[index]
        });

        if defaults_fit {
            candidate.extend_from_slice(&defaults[candidate.len()..required_extra]);
        } else {
            let missing = required_extra - candidate.len();
            let previous = candidate.last().copied().unwrap_or(primary_frequency);
            let available = 20_000.0 - previous;
            if !available.is_finite() || available <= 0.0 {
                return None;
            }
            let step = available / (missing + 1) as f64;
            if !step.is_finite() || step <= 0.0 {
                return None;
            }
            for offset in 1..=missing {
                candidate.push(previous + step * offset as f64);
            }
        }
    }

    let active_cutoffs = &candidate[..required_extra];
    if !valid_band_cutoff(primary_frequency)
        || active_cutoffs
            .iter()
            .any(|cutoff| !valid_band_cutoff(*cutoff))
        || std::iter::once(primary_frequency)
            .chain(active_cutoffs.iter().copied())
            .collect::<Vec<_>>()
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        || !valid_core_band_cutoffs(primary_frequency, active_cutoffs)
    {
        return None;
    }

    Some(candidate)
}

impl crate::plugins::PluginSettings {
    pub(super) fn crossover_param_value(&self, index: usize) -> Option<f64> {
        let Self::Crossover {
            crossover_type,
            frequency,
            output,
            fir_taps,
            topology,
            band_count,
            extra_frequencies,
            channel_frequencies_hz,
            ..
        } = self
        else {
            return None;
        };

        let inferred_topology = topology.unwrap_or_else(|| {
            if channel_frequencies_hz
                .as_ref()
                .is_some_and(|frequencies| !frequencies.is_empty())
                && extra_frequencies.is_empty()
            {
                sotf_plugins::plugin_crossover::CrossoverTopology::PerChannel
            } else {
                sotf_plugins::plugin_crossover::CrossoverTopology::Bands
            }
        });
        let default_frequency_2 =
            param_specs::find_by_key(param_specs::crossover::PARAMS, "frequency_2").default_f64();
        let default_frequency_3 =
            param_specs::find_by_key(param_specs::crossover::PARAMS, "frequency_3").default_f64();

        match index {
            0 => Some(crossover_plugin_type_to_index(crossover_type)),
            1 => Some(*frequency),
            2 => Some(crossover_output_to_index(output)),
            3 => Some(*fir_taps as f64),
            4 => Some(
                if inferred_topology
                    == sotf_plugins::plugin_crossover::CrossoverTopology::PerChannel
                {
                    1.0
                } else {
                    0.0
                },
            ),
            5 => Some(
                band_count
                    .unwrap_or_else(|| inferred_band_count(extra_frequencies))
                    .saturating_sub(2)
                    .min(2) as f64,
            ),
            6 => Some(
                extra_frequencies
                    .first()
                    .copied()
                    .unwrap_or(default_frequency_2),
            ),
            7 => Some(
                extra_frequencies
                    .get(1)
                    .copied()
                    .unwrap_or(default_frequency_3),
            ),
            _ => None,
        }
    }

    pub(super) fn crossover_set_param_value(&mut self, index: usize, value: f64) {
        let Self::Crossover {
            crossover_type,
            frequency,
            output,
            fir_taps,
            topology,
            band_count,
            extra_frequencies,
            channel_frequencies_hz,
            ..
        } = self
        else {
            return;
        };

        let specs = param_specs::crossover::PARAMS;
        match index {
            0 => *crossover_type = index_to_crossover_plugin_type(value),
            1 => *frequency = specs[1].clamp_f64(value),
            2 => *output = index_to_crossover_output(value),
            3 => *fir_taps = specs[3].clamp_f64(value) as usize,
            4 => {
                let requested_topology = if value.round() as usize == 1 {
                    sotf_plugins::plugin_crossover::CrossoverTopology::PerChannel
                } else {
                    sotf_plugins::plugin_crossover::CrossoverTopology::Bands
                };
                if requested_topology == sotf_plugins::plugin_crossover::CrossoverTopology::Bands {
                    let inferred_topology = topology.unwrap_or_else(|| {
                        if channel_frequencies_hz
                            .as_ref()
                            .is_some_and(|frequencies| !frequencies.is_empty())
                            && extra_frequencies.is_empty()
                        {
                            sotf_plugins::plugin_crossover::CrossoverTopology::PerChannel
                        } else {
                            sotf_plugins::plugin_crossover::CrossoverTopology::Bands
                        }
                    });
                    if inferred_topology
                        == sotf_plugins::plugin_crossover::CrossoverTopology::PerChannel
                    {
                        let requested_count =
                            band_count.unwrap_or_else(|| inferred_band_count(extra_frequencies));
                        if let Some(materialized) = materialize_crossover_cutoffs(
                            *frequency,
                            extra_frequencies,
                            requested_count,
                        ) {
                            *extra_frequencies = materialized;
                            *band_count = Some(requested_count);
                            *topology = Some(requested_topology);
                        }
                    } else {
                        *topology = Some(requested_topology);
                    }
                } else {
                    *topology = Some(requested_topology);
                }
            }
            5 => {
                let requested_count = value.round().clamp(0.0, 2.0) as usize + 2;
                let current_count =
                    band_count.unwrap_or_else(|| inferred_band_count(extra_frequencies));
                if requested_count > current_count {
                    let inferred_topology = topology.unwrap_or_else(|| {
                        if channel_frequencies_hz
                            .as_ref()
                            .is_some_and(|frequencies| !frequencies.is_empty())
                            && extra_frequencies.is_empty()
                        {
                            sotf_plugins::plugin_crossover::CrossoverTopology::PerChannel
                        } else {
                            sotf_plugins::plugin_crossover::CrossoverTopology::Bands
                        }
                    });
                    if inferred_topology
                        == sotf_plugins::plugin_crossover::CrossoverTopology::PerChannel
                    {
                        // Band count and cutoffs are dormant in this topology.
                        // Defer cutoff creation until Bands is selected so
                        // legacy implicit PerChannel and partial-mode fallback
                        // continue to deserialize with their original meaning.
                        *band_count = Some(requested_count);
                    } else if let Some(materialized) = materialize_crossover_cutoffs(
                        *frequency,
                        extra_frequencies,
                        requested_count,
                    ) {
                        *extra_frequencies = materialized;
                        *band_count = Some(requested_count);
                    }
                } else {
                    // Shrinking only changes the active prefix. Keep all
                    // inactive cutoffs so a later expansion restores them.
                    *band_count = Some(requested_count);
                }
            }
            6 => {
                while extra_frequencies.is_empty() {
                    extra_frequencies
                        .push(param_specs::find_by_key(specs, "frequency_2").default_f64());
                }
                extra_frequencies[0] = specs[6].clamp_f64(value);
            }
            7 => {
                while extra_frequencies.len() < 2 {
                    let default = if extra_frequencies.is_empty() {
                        param_specs::find_by_key(specs, "frequency_2").default_f64()
                    } else {
                        param_specs::find_by_key(specs, "frequency_3").default_f64()
                    };
                    extra_frequencies.push(default);
                }
                extra_frequencies[1] = specs[7].clamp_f64(value);
            }
            _ => {}
        }
    }
}
