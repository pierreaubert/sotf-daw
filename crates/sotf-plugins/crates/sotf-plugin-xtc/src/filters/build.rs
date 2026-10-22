use super::super::config::XtcPluginParams;
use super::apply::apply_beta_freq_boosts;
use super::compute::compute_beta_condition_number_full;
use super::types::HrtfTransferFunctions;

/// Build condition-number based beta LUT using Woodworth model transfer functions.
///
/// Computes the plant matrix at each frequency bin and uses the condition number
/// to set the regularization strength.
pub(crate) fn build_beta_lut_condition_number(
    num_bins: usize,
    freq_per_bin: f32,
    params: &XtcPluginParams,
    hrtf_data: Option<&HrtfTransferFunctions>,
) -> Vec<f32> {
    if let Some(hrtf) = hrtf_data {
        // HRTF mode: use actual HRTF transfer functions for condition number
        (0..num_bins)
            .map(|bin| {
                let freq = bin as f32 * freq_per_bin;
                compute_beta_condition_number_full(
                    hrtf.h_ll[bin],
                    hrtf.h_lr[bin],
                    hrtf.h_rl[bin],
                    hrtf.h_rr[bin],
                    freq,
                    params,
                )
            })
            .collect()
    } else {
        // Woodworth mode: compute analytical plant matrix per bin
        // We need the geometry cache data, but this is called from contexts
        // that already have it. For a standalone LUT, use the params directly.
        (0..num_bins)
            .map(|bin| {
                let freq = bin as f32 * freq_per_bin;
                // Fallback: just use beta_base with frequency boosts
                apply_beta_freq_boosts(params.beta_base, freq, params)
            })
            .collect()
    }
}
