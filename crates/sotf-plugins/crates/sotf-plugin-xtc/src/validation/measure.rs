//! Analytical filter predictions, separate from independently measured acoustic metrics.

// Rust guideline compliant 2026-02-21
use super::super::config::XtcPluginParams;
use super::super::filters::{
    GeometryCache, XtcFilters, compute_geometry_cache, compute_xtc_filters_full,
    contralateral_transfer, pinna_resonance, pinna_resonance_contra,
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

fn analytical_plant(
    params: &XtcPluginParams,
    cache: &GeometryCache,
    freq: f32,
) -> [[Complex<f32>; 2]; 2] {
    let ipsi = if params.pinna_model_enabled {
        pinna_resonance(freq)
    } else {
        1.0
    };
    let cross = |angle: f32, radius: f32, amplitude: f32, delay: f32, pinna_angle: f32| {
        let transfer = contralateral_transfer(params, freq, angle, radius, amplitude, delay);
        let pinna = if params.pinna_model_enabled {
            pinna_resonance_contra(freq, pinna_angle)
        } else {
            1.0
        };
        (transfer, pinna)
    };
    if let Some(asym) = &cache.asymmetric {
        let (left, left_pinna) = cross(
            asym.angle_left_contra,
            asym.a,
            asym.amplitude_ratio_left,
            asym.delay_left_contra - asym.delay_left_ipsi,
            asym.theta_right.abs().to_degrees(),
        );
        let (right, right_pinna) = cross(
            asym.angle_right_contra,
            asym.a,
            asym.amplitude_ratio_right,
            asym.delay_right_contra - asym.delay_right_ipsi,
            asym.theta_left.abs().to_degrees(),
        );
        [
            [Complex::new(ipsi, 0.0), left * left_pinna],
            [right * right_pinna, Complex::new(ipsi, 0.0)],
        ]
    } else {
        let sym = &cache.symmetric;
        let delay = sym.delay_contra - sym.delay_ipsi;
        let (contra, pinna) = cross(
            sym.contra_angle,
            sym.a,
            sym.amplitude_ratio,
            delay,
            params.speaker_angle_deg,
        );
        let contra = contra * pinna;
        [
            [Complex::new(ipsi, 0.0), contra],
            [contra, Complex::new(ipsi, 0.0)],
        ]
    }
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
