use crate::plugins::PluginSettings;
use sotf_plugins::param_specs::{self};
use sotf_plugins::{SpectralTiltCorrection, TiltReferenceFreq};

pub(super) fn spectral_tilt_to_index(stc: &SpectralTiltCorrection) -> f64 {
    match stc {
        SpectralTiltCorrection::None => 0.0,
        SpectralTiltCorrection::ThreeDbPerOctave => 1.0,
        SpectralTiltCorrection::SixDbPerOctave => 2.0,
        SpectralTiltCorrection::Pink => 3.0,
        SpectralTiltCorrection::Custom(_) => 3.0,
    }
}

pub(super) fn tilt_reference_to_index(trf: &TiltReferenceFreq) -> f64 {
    match trf {
        TiltReferenceFreq::Standard => 0.0,
        TiltReferenceFreq::OneKilohertz => 1.0,
        TiltReferenceFreq::TwoKilohertz => 2.0,
        TiltReferenceFreq::MinFreq => 3.0,
    }
}

#[inline]
pub(super) fn b2f(b: bool) -> f64 {
    if b { 1.0 } else { 0.0 }
}

#[inline]
pub(super) fn f2b(f: f64) -> bool {
    f > 0.5
}

fn materialize_band_split_cutoffs(
    frequency: &mut f64,
    frequency_2: &mut f64,
    frequency_3: &mut f64,
    num_bands: &mut usize,
    frequencies: &mut Option<Vec<f64>>,
) {
    let Some(cutoffs) = frequencies.as_ref() else {
        return;
    };
    if !(1..=3).contains(&cutoffs.len())
        || cutoffs
            .iter()
            .any(|cutoff| !cutoff.is_finite() || !(20.0..=20_000.0).contains(cutoff))
        || cutoffs.windows(2).any(|pair| pair[0] >= pair[1])
    {
        // Keep an invalid explicit vector intact so the converter fails instead
        // of silently replacing it with the compatibility scalar fields.
        return;
    }

    *frequency = cutoffs[0];
    if let Some(&cutoff) = cutoffs.get(1) {
        *frequency_2 = cutoff;
    }
    if let Some(&cutoff) = cutoffs.get(2) {
        *frequency_3 = cutoff;
    }
    *num_bands = cutoffs.len() + 1;
    *frequencies = None;
}

impl PluginSettings {
    pub(super) fn band_split_param_value(&self, index: usize) -> Option<f64> {
        let Self::BandSplit {
            frequency,
            crossover_type,
            frequencies,
            recombination_mode,
            num_bands,
            frequency_2,
            frequency_3,
            ..
        } = self
        else {
            return None;
        };

        let effective_count = frequencies
            .as_ref()
            .map(|cuts| cuts.len().saturating_add(1))
            .unwrap_or(*num_bands);
        let effective_frequency = |cutoff_index: usize, fallback: f64| match frequencies {
            // An explicitly empty vector is invalid input and must remain
            // visible as such instead of reading stale compatibility fields.
            Some(cuts) if cuts.is_empty() => None,
            // Active explicit cutoffs win. Compatibility fields remain useful
            // for inactive static cutoff controls, such as Frequency 3 on a
            // three-band instance.
            Some(cuts) => Some(cuts.get(cutoff_index).copied().unwrap_or(fallback)),
            None => Some(fallback),
        };

        match index {
            0 => effective_frequency(0, *frequency),
            1 => Some(super::crossover::crossover_type_to_index(crossover_type)),
            2 => Some(super::band_split_mode_to_index(recombination_mode)),
            3 => Some(super::band_split_num_bands_to_index(&effective_count)),
            4 => effective_frequency(1, *frequency_2),
            5 => effective_frequency(2, *frequency_3),
            _ => None,
        }
    }

    pub(super) fn band_split_set_param_value(&mut self, index: usize, value: f64) {
        let Self::BandSplit {
            frequency,
            crossover_type,
            frequencies,
            recombination_mode,
            num_bands,
            frequency_2,
            frequency_3,
            ..
        } = self
        else {
            return;
        };

        let specs = param_specs::band_split::PARAMS;
        match index {
            0 => {
                materialize_band_split_cutoffs(
                    frequency,
                    frequency_2,
                    frequency_3,
                    num_bands,
                    frequencies,
                );
                *frequency = specs[0].clamp_f64(value);
            }
            1 => {
                let last_index = specs[1].choice_labels().len().saturating_sub(1) as f64;
                *crossover_type =
                    super::index_to_crossover_type(value.round().clamp(0.0, last_index));
            }
            2 => *recombination_mode = super::index_to_band_split_mode(value),
            3 => {
                materialize_band_split_cutoffs(
                    frequency,
                    frequency_2,
                    frequency_3,
                    num_bands,
                    frequencies,
                );
                *num_bands = super::index::index_to_band_split_num_bands(value);
            }
            4 => {
                materialize_band_split_cutoffs(
                    frequency,
                    frequency_2,
                    frequency_3,
                    num_bands,
                    frequencies,
                );
                *frequency_2 = specs[4].clamp_f64(value);
            }
            5 => {
                materialize_band_split_cutoffs(
                    frequency,
                    frequency_2,
                    frequency_3,
                    num_bands,
                    frequencies,
                );
                *frequency_3 = specs[5].clamp_f64(value);
            }
            _ => {}
        }
    }

    /// Get the engine parameter key and value string for zero-dropout updates.
    ///
    /// Returns `None` for structural params, file paths, out-of-range indices,
    /// and plugins with no editable params. For plugins where PARAMS ordering
    /// matches the GPUI param index, this replaces the manual per-plugin mapping.
    pub fn engine_param_at(&self, idx: usize) -> Option<(String, String)> {
        let specs = self.param_specs();
        let spec = specs.get(idx)?;
        if spec.update_mode == param_specs::UpdateMode::Structural {
            return None;
        }
        if matches!(spec.param_type, param_specs::ParamType::FilePath) {
            return None;
        }
        let value = self.param_value_string(idx)?;
        Some((spec.engine_key.to_string(), value))
    }

    /// Format the current value of parameter at `index` as a string for engine communication.
    ///
    /// Unlike `param_value()` which returns f64, this returns the raw string value
    /// suitable for JSON serialization to the plugin engine. String-typed choices
    /// (speaker_config, crossover_type) are returned as their string values.
    pub fn param_value_string(&self, index: usize) -> Option<String> {
        let specs = self.param_specs();
        let spec = specs.get(index)?;

        match spec.param_type {
            param_specs::ParamType::FilePath => {
                const CONVOLUTION_IR_FILE_IDX: usize =
                    param_specs::index_of(param_specs::convolution::PARAMS, "ir_file");
                const BINAURAL_SOFA_FILE_IDX: usize =
                    param_specs::index_of(param_specs::binaural::PARAMS, "sofa_file");
                const AB_PATH_A_CONFIG_IDX: usize =
                    param_specs::index_of(param_specs::ab_compare::PARAMS, "path_a_config");
                const AB_PATH_B_CONFIG_IDX: usize =
                    param_specs::index_of(param_specs::ab_compare::PARAMS, "path_b_config");

                // Return the file path string directly. Indices are derived from
                // PARAMS so param reordering fails fast instead of drifting.
                match self {
                    Self::Convolution { ir_file, .. } if index == CONVOLUTION_IR_FILE_IDX => {
                        Some(ir_file.clone())
                    }
                    Self::BinauralDecoder { sofa_file, .. } if index == BINAURAL_SOFA_FILE_IDX => {
                        Some(sofa_file.clone())
                    }
                    Self::ABCompare { path_a_file, .. } if index == AB_PATH_A_CONFIG_IDX => {
                        Some(path_a_file.clone())
                    }
                    Self::ABCompare { path_b_file, .. } if index == AB_PATH_B_CONFIG_IDX => {
                        Some(path_b_file.clone())
                    }
                    _ => None,
                }
            }
            param_specs::ParamType::Bool { .. } => {
                self.param_value(index).map(|v| format!("{}", f2b(v)))
            }
            param_specs::ParamType::Choice { .. } => {
                const UPMIXER_SPEAKER_CONFIG_IDX: usize =
                    param_specs::index_of(param_specs::upmixer::PARAMS, "speaker_config");
                const AAE_SPEAKER_CONFIG_IDX: usize =
                    param_specs::index_of(param_specs::aae::PARAMS, "speaker_config");
                const AAE_ROOM_PRESET_IDX: usize =
                    param_specs::index_of(param_specs::aae::PARAMS, "room_preset");
                const AMBISONICS_TARGET_LAYOUT_IDX: usize =
                    param_specs::index_of(param_specs::ambisonics::PARAMS, "target_layout");
                const BAND_SPLIT_CROSSOVER_TYPE_IDX: usize =
                    param_specs::index_of(param_specs::band_split::PARAMS, "type");
                const CROSSFEED_MODE_IDX: usize =
                    param_specs::index_of(param_specs::crossfeed::PARAMS, "mode");
                const CROSSFEED_PRESET_IDX: usize =
                    param_specs::index_of(param_specs::crossfeed::PARAMS, "preset");
                const COMPRESSOR_SIDECHAIN_HPF_ORDER_IDX: usize =
                    param_specs::index_of(param_specs::compressor::PARAMS, "sidechain_hpf_order");
                const COMPRESSOR_DETECTION_MODE_IDX: usize =
                    param_specs::index_of(param_specs::compressor::PARAMS, "detection_mode");
                const EXPANDER_DETECTION_MODE_IDX: usize =
                    param_specs::index_of(param_specs::expander::PARAMS, "detection_mode");
                const MULTIBAND_EXPANDER_DETECTION_MODE_IDX: usize = param_specs::index_of(
                    param_specs::multiband_expander::GLOBAL_PARAMS,
                    "detection_mode",
                );
                const MULTIBAND_COMPRESSOR_SIDECHAIN_HPF_ORDER_IDX: usize = param_specs::index_of(
                    param_specs::multiband_compressor::GLOBAL_PARAMS,
                    "sidechain_hpf_order",
                );
                const MULTIBAND_COMPRESSOR_DETECTION_MODE_IDX: usize = param_specs::index_of(
                    param_specs::multiband_compressor::GLOBAL_PARAMS,
                    "detection_mode",
                );

                // String-typed choices need special handling
                match self {
                    Self::Upmixer { speaker_config, .. } if index == UPMIXER_SPEAKER_CONFIG_IDX => {
                        Some(speaker_config.clone())
                    }
                    Self::AAE { speaker_config, .. } if index == AAE_SPEAKER_CONFIG_IDX => {
                        Some(speaker_config.clone())
                    }
                    Self::AAE { room_preset, .. } if index == AAE_ROOM_PRESET_IDX => {
                        Some(room_preset.clone())
                    }
                    Self::AmbisonicsDecoder { target_layout, .. }
                        if index == AMBISONICS_TARGET_LAYOUT_IDX =>
                    {
                        Some(target_layout.clone())
                    }
                    Self::BandSplit { crossover_type, .. }
                        if index == BAND_SPLIT_CROSSOVER_TYPE_IDX =>
                    {
                        Some(crossover_type.clone())
                    }
                    Self::Crossfeed { mode, .. } if index == CROSSFEED_MODE_IDX => Some(format!(
                        "{}",
                        serde_json::to_value(mode).unwrap_or_default()
                    )),
                    Self::Crossfeed { preset, .. } if index == CROSSFEED_PRESET_IDX => Some(
                        format!("{}", serde_json::to_value(preset).unwrap_or_default()),
                    ),
                    Self::Compressor {
                        sidechain_hpf_order,
                        ..
                    } if index == COMPRESSOR_SIDECHAIN_HPF_ORDER_IDX => {
                        Some(sidechain_hpf_order.clone())
                    }
                    Self::Compressor { detection_mode, .. }
                        if index == COMPRESSOR_DETECTION_MODE_IDX =>
                    {
                        Some(detection_mode.clone())
                    }
                    Self::Expander { detection_mode, .. }
                        if index == EXPANDER_DETECTION_MODE_IDX =>
                    {
                        Some(detection_mode.clone())
                    }
                    Self::MultibandExpander { detection_mode, .. }
                        if index == MULTIBAND_EXPANDER_DETECTION_MODE_IDX =>
                    {
                        Some(detection_mode.clone())
                    }
                    Self::MultibandCompressor {
                        sidechain_hpf_order,
                        ..
                    } if index == MULTIBAND_COMPRESSOR_SIDECHAIN_HPF_ORDER_IDX => {
                        Some(sidechain_hpf_order.clone())
                    }
                    Self::MultibandCompressor { detection_mode, .. }
                        if index == MULTIBAND_COMPRESSOR_DETECTION_MODE_IDX =>
                    {
                        Some(detection_mode.clone())
                    }
                    _ => {
                        // Numeric choice: format as integer
                        self.param_value(index).map(|v| format!("{}", v as i64))
                    }
                }
            }
            param_specs::ParamType::Int { .. } => {
                self.param_value(index).map(|v| format!("{}", v as i64))
            }
            param_specs::ParamType::Float { .. } => {
                self.param_value(index).map(|v| spec.engine_value_string(v))
            }
        }
    }
}
