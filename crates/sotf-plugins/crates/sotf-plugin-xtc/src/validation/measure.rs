//! Analytical filter predictions, separate from independently measured acoustic metrics.

// Rust guideline compliant 2026-02-21
use super::super::config::XtcPluginParams;
use super::super::filters::{
    XtcFilters, analytical_plant, compute_geometry_cache, compute_xtc_filters_full,
};
use super::matrix::{CancellationMetrics, ComplexTransferMatrix, evaluate_cancellation};
use rustfft::num_complex::Complex;

/// An analytical cancellation prediction at one selected RFFT bin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CancellationPrediction {
    /// Selected filter bin, using the historical floor-to-bin convention.
    pub fft_bin: usize,
    /// Actual bin frequency used for both the filter and analytical plant.
    pub frequency_hz: f32,
    /// Left/right input-cue metrics, including intended-signal preservation.
    pub metrics: CancellationMetrics,
}

// The predictor has no prepared HRTF/RoomEQ/room resource snapshot. Refuse these
// modes explicitly instead of presenting an analytical fallback as their response.
pub(crate) fn diagnostic_config_supported(params: &XtcPluginParams) -> bool {
    params.source_mode == "synthetic"
        && params.hrtf_file.is_none()
        && !params.room_reflections_enabled
        && (128..=16384).contains(&params.fft_size)
        && params.fft_size.is_power_of_two()
        && params.head_model <= 1
        && matches!(
            params.itd_modeling.as_str(),
            "phase_only" | "explicit_delay"
        )
        && [
            params.distance_m,
            params.speaker_angle_deg,
            params.head_radius_m,
            params.head_offset_x,
            params.head_offset_z,
            params.head_yaw_deg,
            params.head_shadow_cutoff_hz,
            params.head_shadow_slope_db_per_octave,
            params.beta_base,
            params.beta_low_freq_boost,
            params.beta_high_freq_boost,
            params.kappa_target,
            params.max_gain_db,
        ]
        .iter()
        .all(|v| v.is_finite())
        && params.distance_m + params.head_offset_z > 0.0
        && params.head_radius_m > 0.0
        && params.head_shadow_cutoff_hz > 0.0
        && params.beta_base >= 0.0
        && params.kappa_target > 0.0
}

fn selected_bin(params: &XtcPluginParams, sample_rate: u32, frequency: f32) -> Option<usize> {
    if !diagnostic_config_supported(params)
        || sample_rate == 0
        || !frequency.is_finite()
        || frequency < 0.0
        || frequency > sample_rate as f32 * 0.5
    {
        return None;
    }
    Some(
        ((frequency / (sample_rate as f32 / params.fft_size as f32)) as usize)
            .min(params.fft_size / 2),
    )
}

fn pack(matrix: [[Complex<f32>; 2]; 2]) -> ComplexTransferMatrix {
    matrix.map(|row| row.map(|c| [f64::from(c.re), f64::from(c.im)]))
}

pub(crate) fn prediction_with_filters(
    filters: &XtcFilters,
    params: &XtcPluginParams,
    sample_rate: u32,
    frequency: f32,
    num_bins: usize,
) -> Option<CancellationPrediction> {
    let bin = selected_bin(params, sample_rate, frequency)?;
    if num_bins != params.fft_size / 2 + 1 || filters.speaker_filters.is_some() {
        return None;
    }
    let ll = *filters.filter_ll.get(bin)?;
    let lr = *filters.filter_lr.get(bin)?;
    let rl = *filters
        .filter_rl
        .as_ref()
        .unwrap_or(&filters.filter_lr)
        .get(bin)?;
    let rr = *filters
        .filter_rr
        .as_ref()
        .unwrap_or(&filters.filter_ll)
        .get(bin)?;
    let cache = compute_geometry_cache(params, sample_rate, num_bins);
    let frequency_hz = bin as f32 * cache.freq_per_bin;
    let plant = analytical_plant(params, &cache, frequency_hz);
    Some(CancellationPrediction {
        fft_bin: bin,
        frequency_hz,
        metrics: evaluate_cancellation(pack(plant), pack([[ll, lr], [rl, rr]])),
    })
}

/// Predict analytical filter cancellation at the selected FFT frequency.
///
/// Uses the configured FFT size, head model, yaw, shadow tuning, pinna and LF
/// delay mode. This shares the design's analytical helpers and predicts filter
/// coefficients; it does not measure rendered STFT audio, auto-gain, the limiter,
/// bypass state or a real listener. Use [`evaluate_cancellation`] with independently
/// obtained matrices for those comparisons.
///
/// Returns `None` for invalid frequency/rate/grid/geometry or resource-dependent
/// HRTF, RoomEQ and room-reflection configurations. It never loads those resources
/// or silently substitutes a different plant.
pub fn predict_analytic_cancellation(
    params: &XtcPluginParams,
    sample_rate: u32,
    freq_hz: f32,
) -> Option<CancellationPrediction> {
    selected_bin(params, sample_rate, freq_hz)?;
    let num_bins = params.fft_size / 2 + 1;
    let filters = compute_xtc_filters_full(params, sample_rate, num_bins);
    prediction_with_filters(&filters, params, sample_rate, freq_hz, num_bins)
}

/// Return the worse signed raw prediction across the two intended input cues.
///
/// Retains negative degradation and values beyond the former 0..40 dB display
/// clamp. Returns `NaN` when prediction is unsupported or intended output is
/// absent/undefined. The richer metrics also expose intended gain and normalized
/// separation, which must accompany raw attenuation when assessing quality.
pub(crate) fn measure_cancellation_depth_db_with_filters(
    filters: &XtcFilters,
    params: &XtcPluginParams,
    sample_rate: u32,
    freq_hz: f32,
    num_bins: usize,
) -> f32 {
    let Some(prediction) = prediction_with_filters(filters, params, sample_rate, freq_hz, num_bins)
    else {
        return f32::NAN;
    };
    scalar_depth(prediction.metrics)
}

pub(crate) fn scalar_depth(metrics: CancellationMetrics) -> f32 {
    if metrics.desired_gain_db.iter().any(|v| !v.is_finite())
        || metrics.raw_depth_db.iter().any(|v| v.is_nan())
    {
        return f32::NAN;
    }
    metrics.raw_depth_db[0].min(metrics.raw_depth_db[1]) as f32
}

/// Return the worse signed raw analytical prediction across both input cues.
///
/// This compatibility helper is an uncapped model prediction, not an acoustic
/// measurement. See [`predict_analytic_cancellation`] for frequency/model scope
/// and intended-signal metrics. Invalid or unsupported predictions return `NaN`.
pub fn measure_cancellation_depth_db(
    params: &XtcPluginParams,
    sample_rate: u32,
    freq_hz: f32,
) -> f32 {
    predict_analytic_cancellation(params, sample_rate, freq_hz)
        .map(|p| scalar_depth(p.metrics))
        .unwrap_or(f32::NAN)
}

/// Return signed analytical predictions while reusing one computed filter set.
///
/// Frequencies use the same selection and validity rules as
/// [`predict_analytic_cancellation`]. Unsupported configurations return `NaN`
/// for each requested frequency without loading resources.
pub fn measure_cancellation_depth_spectrum(
    params: &XtcPluginParams,
    sample_rate: u32,
    freq_points: &[f32],
) -> Vec<(f32, f32)> {
    if sample_rate == 0 || !diagnostic_config_supported(params) {
        return freq_points.iter().map(|&f| (f, f32::NAN)).collect();
    }
    let num_bins = params.fft_size / 2 + 1;
    let filters = compute_xtc_filters_full(params, sample_rate, num_bins);
    freq_points
        .iter()
        .map(|&freq| {
            (
                freq,
                measure_cancellation_depth_db_with_filters(
                    &filters,
                    params,
                    sample_rate,
                    freq,
                    num_bins,
                ),
            )
        })
        .collect()
}

#[cfg(test)]
mod source_crossing_tests {
    use super::*;

    #[test]
    fn quarter_turn_spherical_sources_have_equal_transfer_columns() {
        let mut cases = 0;
        let mut failures = 0;
        let mut maximum_error = 0.0_f32;
        for head_model in [0, 1] {
            for yaw in [-90.0, 90.0] {
                for mode in ["phase_only", "explicit_delay"] {
                    let params = XtcPluginParams {
                        head_model,
                        head_yaw_deg: yaw,
                        itd_modeling: mode.into(),
                        pinna_model_enabled: false,
                        ..Default::default()
                    };
                    let cache = compute_geometry_cache(&params, 48_000, 1025);
                    for frequency in [0.0, 93.75, 1000.0, 4000.0, 8000.0] {
                        let plant = analytical_plant(&params, &cache, frequency);
                        // At a quarter turn, both symmetric sources have equal
                        // projection onto the interaural axis and equal range.
                        // An antipodal spherical model without pinna cues must
                        // therefore give equal columns, independently of ear-row
                        // normalization. This is not a measured-human claim.
                        for row in plant {
                            let error = (row[0] - row[1]).norm();
                            maximum_error = maximum_error.max(error);
                            failures += usize::from(error > 2e-5);
                            cases += 1;
                        }
                    }
                }
            }
        }
        println!(
            "quarter_turn cases={cases} failed_values={failures} maximum_error={maximum_error}"
        );
        assert_eq!(failures, 0);
    }
    #[test]
    fn both_ear_paths_converge_at_front_and_rear_source_crossings() {
        let mut cases = 0;
        let mut maximum_ear_error = 0.0_f32;
        let mut maximum_pose_difference = 0.0_f32;
        for head_model in [0, 1] {
            for pinna in [false, true] {
                for mode in ["phase_only", "explicit_delay"] {
                    for (distance, angle, x, z, rear) in [
                        (1.0_f32, 30.0_f32, 0.0, 0.0, false),
                        (1.0, 30.0, 0.1, 0.5, false),
                        (0.5, 60.0, 0.0, 0.5, true),
                    ] {
                        for speaker in [0, 1] {
                            let side = if speaker == 0 { -1.0 } else { 1.0 };
                            let sx = side * distance * angle.to_radians().sin() - x;
                            let sz = distance * angle.to_radians().cos() - z;
                            let azimuth = sx.atan2(sz).to_degrees();
                            let yaw = if rear {
                                azimuth - side * 180.0
                            } else {
                                azimuth
                            };
                            let params = XtcPluginParams {
                                distance_m: distance,
                                speaker_angle_deg: angle,
                                head_offset_x: x,
                                head_offset_z: z,
                                head_yaw_deg: yaw,
                                head_model,
                                pinna_model_enabled: pinna,
                                itd_modeling: mode.into(),
                                ..Default::default()
                            };
                            for frequency in [93.75, 1000.0, 2700.0, 4500.0, 8000.0] {
                                let plant = analytical_plant(
                                    &params,
                                    &compute_geometry_cache(&params, 48_000, 1025),
                                    frequency,
                                );
                                let error = (plant[0][speaker] - plant[1][speaker]).norm();
                                maximum_ear_error = maximum_ear_error.max(error);
                                assert!(
                                    error < 2e-5,
                                    "median ears differ: yaw={yaw} frequency={frequency} error={error}"
                                );
                                let shifted = [-0.00001, 0.00001].map(|delta| {
                                    let p = XtcPluginParams {
                                        head_yaw_deg: yaw + delta,
                                        ..params.clone()
                                    };
                                    analytical_plant(
                                        &p,
                                        &compute_geometry_cache(&p, 48_000, 1025),
                                        frequency,
                                    )
                                });
                                for (before, after) in shifted[0].iter().zip(&shifted[1]) {
                                    let difference = (before[speaker] - after[speaker]).norm();
                                    maximum_pose_difference =
                                        maximum_pose_difference.max(difference);
                                    assert!(
                                        difference < 2e-4,
                                        "source crossing jumps: yaw={yaw} frequency={frequency} difference={difference}"
                                    );
                                }
                                cases += 1;
                            }
                        }
                    }
                }
            }
        }
        println!(
            "source_crossings cases={cases} maximum_ear_error={maximum_ear_error} maximum_pose_difference={maximum_pose_difference}"
        );
    }
}
