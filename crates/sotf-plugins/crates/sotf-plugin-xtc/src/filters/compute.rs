use super::super::config::XtcPluginParams;
use super::super::reflections::{
    RoomReflectionData, build_reflection_data_image_source, build_reflection_data_ir,
};
use super::apply::apply_beta_freq_boosts;
use super::apply::apply_effort_constraint;
use super::build::build_beta_lut_condition_number;
use super::head::head_shadowing_complex;
use super::misc::SPEED_OF_SOUND;
use super::misc::condition_number_2x2;
use super::misc::sanitize_filter;
use super::misc::sigmoid_smooth;
use super::misc::{soft_limit_column_f64, soft_limit_filter_column};
use super::types::GeometryCache;
use super::types::HrtfTransferFunctions;
use super::xtc_filters::XtcFilters;
use rustfft::num_complex::Complex;
use std::f32::consts::PI;
use std::sync::Arc;

#[cfg(test)]
#[path = "transfer_audit_tests.rs"]
mod transfer_audit_tests;

#[cfg(test)]
#[path = "lateral_pose_tests.rs"]
mod lateral_pose_tests;

/// Return the head-shadow magnitude used by the plant model.
///
/// Geometric path length supplies the relative delay for every analytical
/// model. Brown–Duda's public helper also reports that delay as a phase (which
/// is useful to callers comparing the standalone model), but applying both
/// phases would double the ITD in the composed plant.
#[inline]
pub(crate) fn head_shadowing_for_plant(
    freq: f32,
    angle_rad: f32,
    head_radius: f32,
    head_model: usize,
    cutoff_hz: f32,
    slope_db_per_octave: f32,
) -> Complex<f32> {
    let base = match head_model {
        1 => Complex::new(
            super::head::head_shadowing_brown_duda(freq, angle_rad, head_radius).0,
            0.0,
        ),
        _ => head_shadowing_complex(freq, angle_rad, head_radius, head_model),
    };
    if freq <= 0.0 || cutoff_hz <= 0.0 {
        return base;
    }

    // The analytical models have a fixed diffraction roll-off. These controls
    // specify a correction relative to the historical 4 kHz / 6 dB-octave
    // tuning, preserving the default response while making both controls
    // acoustically effective and continuously adjustable.
    let requested_octaves = (freq / cutoff_hz).log2().max(0.0);
    let reference_octaves = (freq / 4_000.0).log2().max(0.0);
    let correction_db = -slope_db_per_octave * requested_octaves + 6.0 * reference_octaves;
    let angular_weight = (angle_rad.abs() * 0.5).sin().powi(2);
    let correction = 10.0_f32
        .powf(correction_db * angular_weight / 20.0)
        .clamp(0.125, 8.0);
    base * correction
}

/// Build a continuous four-path relative analytical plant.
///
/// Rows are ears; columns are speakers. Each source's near-ear magnitude is
/// the reference for both of its ear paths. At either median plane both ear
/// angles coincide, so their shadow and pinna ratios converge to one rather
/// than switching between unity and an attenuated fixed contralateral label.
/// Both sources share one range/time reference; lateral range differences stay
/// in the matrix. This retains the center-range/extra-arc approximation rather
/// than claiming exact finite-distance ear-surface acoustics.
pub(crate) fn analytical_plant(
    params: &XtcPluginParams,
    cache: &GeometryCache,
    freq: f32,
) -> [[Complex<f32>; 2]; 2] {
    let column = |source: &super::types::SourceGeometry| {
        let shadow = |angle| {
            head_shadowing_for_plant(
                freq,
                angle,
                params.head_radius_m,
                params.head_model,
                params.head_shadow_cutoff_hz,
                params.head_shadow_slope_db_per_octave,
            )
            .re
        };
        let near_shadow = shadow(source.near_angle);
        let pinna = |angle: f32| {
            if params.pinna_model_enabled {
                super::pinna_resonance_contra(freq, angle.to_degrees() - 90.0)
            } else {
                1.0
            }
        };
        let near_pinna = pinna(source.near_angle);
        let reference_pinna = if params.pinna_model_enabled {
            super::pinna_resonance(freq)
        } else {
            1.0
        };
        std::array::from_fn::<_, 2, _>(|ear| {
            let shadow_ratio =
                shadow(source.ear_angles[ear]) / near_shadow * source.amplitudes[ear];
            let magnitude = if params.itd_modeling == "explicit_delay" {
                let blend = 1.0 / (1.0 + ((freq - 300.0) / 50.0).exp());
                blend + shadow_ratio * (1.0 - blend)
            } else {
                shadow_ratio
            };
            let amplitude =
                magnitude * (pinna(source.ear_angles[ear]) / near_pinna) * reference_pinna;
            let phase = -2.0 * PI * freq * source.delays[ear];
            Complex::new(phase.cos(), phase.sin()) * amplitude
        })
    };
    let left = column(&cache.sources[0]);
    let right = column(&cache.sources[1]);
    [[left[0], right[0]], [left[1], right[1]]]
}

/// Compute geometry cache for the given parameters.
///
/// Pre-computes all path lengths, angles, and ratios that don't change per bin.
pub(crate) fn compute_geometry_cache(
    params: &XtcPluginParams,
    sample_rate: impl Into<f64>,
    num_bins: usize,
) -> GeometryCache {
    let sample_rate = sample_rate.into();
    let freq_per_bin = sample_rate as f32 / (2.0 * (num_bins - 1) as f32);

    // Fixed world speakers at (+/-d*sin(theta), d*cos(theta)), matching
    // reflections. Positive listener Z moves toward them. Translation changes
    // source range/incidence; yaw rotates incidence without moving the center.
    let d = f64::from(params.distance_m);
    let speaker_theta = f64::from(params.speaker_angle_deg).to_radians();
    let speaker_x = d * speaker_theta.sin();
    let forward = d * speaker_theta.cos() - f64::from(params.head_offset_z);
    let a = f64::from(params.head_radius_m);
    let x_offset = f64::from(params.head_offset_x);
    let yaw_rad = f64::from(params.head_yaw_deg).to_radians();
    let left_x = -speaker_x - x_offset;
    let right_x = speaker_x - x_offset;
    let ranges = [left_x.hypot(forward), right_x.hypot(forward)];
    // The mean is a common smooth reference under lateral mirroring. Using
    // min(range_L, range_R) created a cusp and a first-order common phase shift
    // at the centered pose, despite smooth physical source motion.
    let reference_range = (ranges[0] + ranges[1]) * 0.5;
    let (sin_yaw, cos_yaw) = yaw_rad.sin_cos();
    let sources = std::array::from_fn(|speaker| {
        let x = [left_x, right_x][speaker];
        let projection = ((x * cos_yaw - forward * sin_yaw) / ranges[speaker]).clamp(-1.0, 1.0);
        let near_angle = projection.abs().acos();
        let extra = a * (projection.abs().asin() + projection.abs());
        let ear_angles = [(-projection).acos(), projection.acos()];
        let extras = [
            if projection > 0.0 { extra } else { 0.0 },
            if projection < 0.0 { extra } else { 0.0 },
        ];
        super::types::SourceGeometry {
            near_angle: near_angle as f32,
            ear_angles: ear_angles.map(|angle| angle as f32),
            amplitudes: std::array::from_fn(|ear| {
                (reference_range / (ranges[speaker] + extras[ear])) as f32
            }),
            delays: std::array::from_fn(|ear| {
                (((ranges[speaker] - reference_range) + extras[ear]) / f64::from(SPEED_OF_SOUND))
                    as f32
            }),
        }
    });
    GeometryCache {
        sources,
        freq_per_bin,
        is_symmetric: params.head_yaw_deg == 0.0 && params.head_offset_x == 0.0,
    }
}

/// Compute crosstalk cancellation filters in frequency domain
///
/// This is the main filter computation function that handles:
/// - Symmetric case (yaw = 0): returns None for filter_rl/filter_rr
/// - Asymmetric case (yaw != 0): returns full 4-filter matrix
///
/// Uses improved Woodworth head shadowing model for better accuracy.
///
/// Optimization 3: Uses pre-computed geometry cache to avoid redundant calculations.
pub(crate) fn compute_xtc_filters_full(
    params: &XtcPluginParams,
    sample_rate: impl Into<f64>,
    num_bins: usize,
) -> XtcFilters {
    let sample_rate = sample_rate.into();
    // Pre-compute geometry cache (Optimization 3)
    let cache = compute_geometry_cache(params, sample_rate, num_bins);

    // Compute room reflection data if enabled
    let room_data: Option<Arc<RoomReflectionData>> = if params.room_reflections_enabled {
        if let Some(ref ir_path) = params.room_ir_file {
            // No pre-planned FFT available here; build_reflection_data_ir will create one.
            build_reflection_data_ir(ir_path, sample_rate, num_bins, None)
                .ok()
                .map(Arc::new)
        } else {
            Some(Arc::new(build_reflection_data_image_source(
                params,
                sample_rate,
                num_bins,
            )))
        }
    } else {
        None
    };

    compute_xtc_filters_full_with_cache(params, sample_rate, num_bins, &cache, room_data)
}

/// Internal filter computation with pre-computed geometry cache.
///
/// Optimization 4: Accepts optional pre-computed room reflection data to avoid
/// redundant computation when parameters haven't changed.
pub(crate) fn compute_xtc_filters_full_with_cache(
    params: &XtcPluginParams,
    _sample_rate: impl Into<f64>,
    num_bins: usize,
    cache: &GeometryCache,
    room_data: Option<Arc<RoomReflectionData>>,
) -> XtcFilters {
    compute_xtc_filters_full_with_cache_and_hrtf(
        params,
        _sample_rate.into(),
        num_bins,
        cache,
        room_data,
        None,
    )
}

/// Internal filter computation with pre-computed geometry cache and optional HRTF data.
pub(crate) fn compute_xtc_filters_full_with_cache_and_hrtf(
    params: &XtcPluginParams,
    _sample_rate: impl Into<f64>,
    num_bins: usize,
    cache: &GeometryCache,
    room_data: Option<Arc<RoomReflectionData>>,
    hrtf_data: Option<&HrtfTransferFunctions>,
) -> XtcFilters {
    let _sample_rate = _sample_rate.into();
    let is_symmetric = cache.is_symmetric && hrtf_data.is_none();
    let room_data_ref: Option<&RoomReflectionData> = room_data
        .as_ref()
        .map(|r: &Arc<RoomReflectionData>| r.as_ref());

    let mut filters = if let Some(hrtf) = hrtf_data {
        // HRTF mode: always use full 4-filter asymmetric path since HRTF
        // data is inherently asymmetric (different left/right ear responses)
        compute_xtc_filters_hrtf(params, num_bins, cache, room_data_ref, hrtf)
    } else if is_symmetric {
        // Use optimized symmetric computation
        let (filter_ll, filter_lr) =
            compute_xtc_filters_symmetric_with_cache(params, num_bins, cache, room_data_ref);
        XtcFilters {
            filter_ll,
            filter_lr,
            filter_rl: None,
            filter_rr: None,
            is_symmetric: true,
            speaker_filters: None,
        }
    } else {
        // Full asymmetric computation for yaw != 0
        let mut f =
            compute_xtc_filters_asymmetric_with_cache(params, num_bins, cache, room_data_ref);
        f.is_symmetric = false;
        f
    };

    // Sanitize filter coefficients (NaN/Inf guard)
    sanitize_filter(&mut filters.filter_ll);

    sanitize_filter(&mut filters.filter_lr);
    if let Some(ref mut rl) = filters.filter_rl {
        sanitize_filter(rl);
    }
    if let Some(ref mut rr) = filters.filter_rr {
        sanitize_filter(rr);
    }

    filters
}

/// Compute asymmetric filters for non-zero yaw angle using pre-computed geometry cache.
///
fn compute_xtc_filters_asymmetric_with_cache(
    params: &XtcPluginParams,
    num_bins: usize,
    cache: &GeometryCache,
    room_data: Option<&RoomReflectionData>,
) -> XtcFilters {
    let mut filter_ll = vec![Complex::new(0.0, 0.0); num_bins];
    let mut filter_lr = vec![Complex::new(0.0, 0.0); num_bins];
    let mut filter_rl = vec![Complex::new(0.0, 0.0); num_bins];
    let mut filter_rr = vec![Complex::new(0.0, 0.0); num_bins];

    let max_gain_linear = 10.0_f32.powf(params.max_gain_db / 20.0);
    for bin in 0..num_bins {
        let freq = bin as f32 * cache.freq_per_bin;
        let [[h_ll_ipsi, h_ll_contra], [h_rr_contra, h_rr_ipsi]] =
            analytical_plant(params, cache, freq);

        // Integrate room reflections: add reflection contributions to transfer functions
        let (h_ll_ipsi_final, h_ll_contra_final, h_rr_ipsi_final, h_rr_contra_final) =
            if let Some(room) = room_data {
                (
                    h_ll_ipsi + room.h_ll_ipsi[bin],
                    h_ll_contra + room.h_lr_contra[bin],
                    h_rr_ipsi + room.h_rr_ipsi[bin],
                    h_rr_contra + room.h_rl_contra[bin],
                )
            } else {
                (h_ll_ipsi, h_ll_contra, h_rr_ipsi, h_rr_contra)
            };

        // Condition-number based regularization (Phase 2)
        let beta = compute_beta_condition_number_full(
            h_ll_ipsi_final,
            h_ll_contra_final,
            h_rr_contra_final,
            h_rr_ipsi_final,
            freq,
            params,
        );
        let beta = if let Some(room) = room_data {
            beta * room.beta_boost[bin]
        } else {
            beta
        };

        // Compute full 2x2 regularized inverse for both ears simultaneously.
        // Speaker L -> Ear L: h_ll_ipsi_final
        // Speaker R -> Ear L: h_ll_contra_final
        // Speaker L -> Ear R: h_rr_contra_final
        // Speaker R -> Ear R: h_rr_ipsi_final
        let (mut w_ll, mut w_lr, mut w_rl, mut w_rr) = compute_full_2x2_inverse(
            h_ll_ipsi_final,
            h_ll_contra_final,
            h_rr_contra_final,
            h_rr_ipsi_final,
            beta,
            max_gain_linear,
            params.bypass_neumann_refinement,
        );

        // Per-bin spectral normalization: target unity gain for the estimated ear response.
        if params.spectral_normalization && !params.bypass_spectral_normalization {
            // Left Ear response for unit Left Input (L_in = 1, R_in = 0)
            // Speakers emit: out_L = w_ll, out_R = w_rl
            // Left ear hears: h_ipsi * w_ll + h_contra * w_rl
            let ear_l = w_ll * h_ll_ipsi_final + w_rl * h_ll_contra_final;

            // Right Ear response for unit Right Input (L_in = 0, R_in = 1)
            // Speakers emit: out_L = w_lr, out_R = w_rr
            // Right ear hears: h_contra * w_lr + h_ipsi * w_rr
            let ear_r = w_lr * h_rr_contra_final + w_rr * h_rr_ipsi_final;

            // Compute both gains before applying, to avoid contamination
            let mag_l = ear_l.norm();
            let gain_l = if mag_l > 0.01 {
                1.0 + 0.9 * ((1.0 / mag_l).clamp(0.5, 4.0) - 1.0)
            } else {
                1.0
            };

            let mag_r = ear_r.norm();
            let gain_r = if mag_r > 0.01 {
                1.0 + 0.9 * ((1.0 / mag_r).clamp(0.5, 4.0) - 1.0)
            } else {
                1.0
            };

            // Scale COLUMNS, not rows:
            // Left column (w_ll, w_rl) → left ear correction
            // Right column (w_lr, w_rr) → right ear correction
            w_ll *= gain_l;
            w_rl *= gain_l;
            w_lr *= gain_r;
            w_rr *= gain_r;

            // Re-apply soft limit: spectral normalization can push gains past budget
            [w_ll, w_rl] = soft_limit_filter_column([w_ll, w_rl], max_gain_linear);
            [w_lr, w_rr] = soft_limit_filter_column([w_lr, w_rr], max_gain_linear);
        }

        // Crossfade to identity (stereo passthrough) at very low and very high frequencies
        // to prevent Tikhonov attenuation from muting the audio at band edges.
        let low_fade = 1.0 - sigmoid_smooth(100.0 - freq, 30.0);
        let high_fade = 1.0 - sigmoid_smooth(freq - 12000.0, 1500.0);
        let alpha = low_fade * high_fade; // 1.0 in passband, 0.0 at extreme edges

        let passthrough = Complex::new(1.0 - alpha, 0.0);
        filter_ll[bin] = w_ll * alpha + passthrough;
        filter_lr[bin] = w_lr * alpha;
        filter_rl[bin] = w_rl * alpha;
        filter_rr[bin] = w_rr * alpha + passthrough;
    }

    // Phase 3: effort-constrained gain limiting
    apply_effort_constraint(
        &mut filter_ll,
        &mut filter_lr,
        Some(&mut filter_rl),
        Some(&mut filter_rr),
        max_gain_linear,
    );

    XtcFilters {
        filter_ll,
        filter_lr,
        filter_rl: Some(filter_rl),
        filter_rr: Some(filter_rr),
        is_symmetric: false,
        speaker_filters: None,
    }
}

/// Compute XTC filters using measured HRTF data as the plant matrix.
///
/// The HRTF transfer functions replace the Woodworth analytical model.
/// The plant matrix C(f) is:
///   C = [[h_ll, h_lr],   (Speaker L->EarL, Speaker R->EarL)
///        [h_rl, h_rr]]   (Speaker L->EarR, Speaker R->EarR)
///
/// This always produces a full 4-filter (asymmetric) result since measured
/// HRTFs are inherently asymmetric between ears.
fn compute_xtc_filters_hrtf(
    params: &XtcPluginParams,
    num_bins: usize,
    cache: &GeometryCache,
    room_data: Option<&RoomReflectionData>,
    hrtf: &HrtfTransferFunctions,
) -> XtcFilters {
    let mut filter_ll = vec![Complex::new(0.0, 0.0); num_bins];
    let mut filter_lr = vec![Complex::new(0.0, 0.0); num_bins];
    let mut filter_rl = vec![Complex::new(0.0, 0.0); num_bins];
    let mut filter_rr = vec![Complex::new(0.0, 0.0); num_bins];

    let max_gain_linear = 10.0_f32.powf(params.max_gain_db / 20.0);

    // Build condition-number based beta LUT
    let beta_lut =
        build_beta_lut_condition_number(num_bins, cache.freq_per_bin, params, Some(hrtf));

    for bin in 0..num_bins {
        let freq = bin as f32 * cache.freq_per_bin;

        // Plant matrix from HRTF data
        let h_ll_val = hrtf.h_ll[bin];
        let h_lr_val = hrtf.h_lr[bin];
        let h_rl_val = hrtf.h_rl[bin];
        let h_rr_val = hrtf.h_rr[bin];

        // Integrate room reflections
        let (h_ll_final, h_lr_final, h_rl_final, h_rr_final) = if let Some(room) = room_data {
            (
                h_ll_val + room.h_ll_ipsi[bin],
                h_lr_val + room.h_lr_contra[bin],
                h_rl_val + room.h_rl_contra[bin],
                h_rr_val + room.h_rr_ipsi[bin],
            )
        } else {
            (h_ll_val, h_lr_val, h_rl_val, h_rr_val)
        };

        let beta = if let Some(room) = room_data {
            beta_lut[bin] * room.beta_boost[bin]
        } else {
            beta_lut[bin]
        };

        let (mut w_ll, mut w_lr, mut w_rl, mut w_rr) = compute_full_2x2_inverse(
            h_ll_final,
            h_lr_final,
            h_rl_final,
            h_rr_final,
            beta,
            max_gain_linear,
            params.bypass_neumann_refinement,
        );

        // Per-bin spectral normalization
        if params.spectral_normalization && !params.bypass_spectral_normalization {
            let ear_l = w_ll * h_ll_final + w_rl * h_lr_final;
            let ear_r = w_lr * h_rl_final + w_rr * h_rr_final;

            let mag_l = ear_l.norm();
            let gain_l = if mag_l > 0.01 {
                1.0 + 0.9 * ((1.0 / mag_l).clamp(0.5, 4.0) - 1.0)
            } else {
                1.0
            };

            let mag_r = ear_r.norm();
            let gain_r = if mag_r > 0.01 {
                1.0 + 0.9 * ((1.0 / mag_r).clamp(0.5, 4.0) - 1.0)
            } else {
                1.0
            };

            w_ll *= gain_l;
            w_rl *= gain_l;
            w_lr *= gain_r;
            w_rr *= gain_r;

            [w_ll, w_rl] = soft_limit_filter_column([w_ll, w_rl], max_gain_linear);
            [w_lr, w_rr] = soft_limit_filter_column([w_lr, w_rr], max_gain_linear);
        }

        // Crossfade to identity at band edges
        let low_fade = 1.0 - sigmoid_smooth(100.0 - freq, 30.0);
        let high_fade = 1.0 - sigmoid_smooth(freq - 12000.0, 1500.0);
        let alpha = low_fade * high_fade;

        let passthrough = Complex::new(1.0 - alpha, 0.0);
        filter_ll[bin] = w_ll * alpha + passthrough;
        filter_lr[bin] = w_lr * alpha;
        filter_rl[bin] = w_rl * alpha;
        filter_rr[bin] = w_rr * alpha + passthrough;
    }

    // Phase 3: effort-constrained gain limiting
    apply_effort_constraint(
        &mut filter_ll,
        &mut filter_lr,
        Some(&mut filter_rl),
        Some(&mut filter_rr),
        max_gain_linear,
    );

    XtcFilters {
        filter_ll,
        filter_lr,
        filter_rl: Some(filter_rl),
        filter_rr: Some(filter_rr),
        is_symmetric: false,
        speaker_filters: None,
    }
}

/// Compute crosstalk cancellation filters in frequency domain (symmetric version)
///
/// Since the XTC matrix is symmetric (filter_rl == filter_lr and filter_rr == filter_ll),
/// we only need to compute and store 2 filters instead of 4.
///
/// Returns (filter_ll, filter_lr) where:
/// - filter_ll: diagonal filter (direct path processing)
/// - filter_lr: cross filter (crosstalk cancellation)
///
/// Geometry is prepared once; magnitude design remains off the audio thread.
fn compute_xtc_filters_symmetric_with_cache(
    params: &XtcPluginParams,
    num_bins: usize,
    cache: &GeometryCache,
    room_data: Option<&RoomReflectionData>,
) -> (Vec<Complex<f32>>, Vec<Complex<f32>>) {
    let mut filter_ll = vec![Complex::new(0.0, 0.0); num_bins];
    let mut filter_lr = vec![Complex::new(0.0, 0.0); num_bins];

    let max_gain_linear = 10.0_f32.powf(params.max_gain_db / 20.0);
    for bin in 0..num_bins {
        let freq = bin as f32 * cache.freq_per_bin;
        let [[h_ipsi_shaped, h_contra_shaped], _] = analytical_plant(params, cache, freq);

        // Integrate room reflections: add reflection contributions to transfer functions
        let (h_ipsi_final, h_contra_final) = if let Some(room) = room_data {
            (
                h_ipsi_shaped + room.h_ll_ipsi[bin],
                h_contra_shaped + room.h_lr_contra[bin],
            )
        } else {
            (h_ipsi_shaped, h_contra_shaped)
        };

        // Recompute condition-number beta with final (shaped+room) transfer functions
        let beta = compute_beta_condition_number(h_ipsi_final, h_contra_final, freq, params);
        let beta = if let Some(room) = room_data {
            beta * room.beta_boost[bin]
        } else {
            beta
        };

        // Use shared 2x2 inverse computation with gain clamping
        let (mut w_ll, mut w_lr) = compute_2x2_inverse(
            h_ipsi_final,
            h_contra_final,
            beta,
            max_gain_linear,
            params.bypass_neumann_refinement,
        );

        // Per-bin spectral normalization: target unity gain for the estimated ear response.
        // This compensates for attenuation introduced by regularization (beta),
        // preventing dull/mono-like sound at low frequencies.
        if params.spectral_normalization && !params.bypass_spectral_normalization {
            let ear_response = w_ll * h_ipsi_final + w_lr * h_contra_final;
            let mag = ear_response.norm();
            if mag > 0.01 {
                // Gentle correction: 90% of the way to unity.
                let correction = (1.0 / mag).clamp(0.5, 4.0);
                let gain = 1.0 + 0.9 * (correction - 1.0);
                w_ll *= gain;
                w_lr *= gain;
            }
            // Re-apply soft limit: spectral normalization can push gains past budget
            [w_ll, w_lr] = soft_limit_filter_column([w_ll, w_lr], max_gain_linear);
        }

        // Crossfade to identity (stereo passthrough) at very low and very high frequencies
        // to prevent Tikhonov attenuation from muting the audio at band edges.
        let low_fade = 1.0 - sigmoid_smooth(100.0 - freq, 30.0);
        let high_fade = 1.0 - sigmoid_smooth(freq - 12000.0, 1500.0);
        let alpha = low_fade * high_fade; // 1.0 in passband, 0.0 at extreme edges

        let passthrough = Complex::new(1.0 - alpha, 0.0);
        filter_ll[bin] = w_ll * alpha + passthrough;
        filter_lr[bin] = w_lr * alpha;
    }

    // Phase 3: effort-constrained gain limiting
    apply_effort_constraint(&mut filter_ll, &mut filter_lr, None, None, max_gain_linear);

    (filter_ll, filter_lr)
}

/// Compute 2x2 inverse filter for one ear with multi-stage Neumann series refinement.
///
/// First computes the regularized inverse W₁ = (C^H*C + β*I)^-1 * C^H, then
/// refines it with one iteration of Neumann series: W₂ = W₁ * (2*I - C*W₁).
/// This improves broadband cancellation depth by correcting residual errors
/// from regularization, similar to the crosstalk cancellation literature.
///
/// Returns (w_ipsi, w_contra) filter coefficients.
/// max_gain_linear limits the magnitude of each output coefficient.
#[inline]
pub(crate) fn compute_2x2_inverse(
    h_ipsi: Complex<f32>,
    h_contra: Complex<f32>,
    beta: f32,
    max_gain_linear: f32,
    bypass_neumann: bool,
) -> (Complex<f32>, Complex<f32>) {
    let (ll, lr, _, _) = compute_full_2x2_inverse(
        h_ipsi,
        h_contra,
        h_contra,
        h_ipsi,
        beta,
        max_gain_linear,
        bypass_neumann,
    );
    (ll, lr)
}

/// Compute full 2x2 regularized inverse for asymmetric crosstalk cancellation.
///
/// Matrix C = [[h_ll, h_lr],
///             [h_rl, h_rr]]
///
/// Returns (w_ll, w_lr, w_rl, w_rr) such that W = (C^H * C + beta * I)^-1 * C^H.
#[inline]
fn compute_full_2x2_inverse(
    h_ll: Complex<f32>,
    h_lr: Complex<f32>,
    h_rl: Complex<f32>,
    h_rr: Complex<f32>,
    beta: f32,
    max_gain_linear: f32,
    bypass_neumann: bool,
) -> (Complex<f32>, Complex<f32>, Complex<f32>, Complex<f32>) {
    // Filter preparation runs off the callback. Use f64 for the Gram determinant
    // and inverse/refinement arithmetic: the four-path model can be nearly rank
    // deficient at a quarter turn, where f32 cancellation spoiled mirror limits.
    let widen = |c: Complex<f32>| Complex::new(f64::from(c.re), f64::from(c.im));
    let [h_ll, h_lr, h_rl, h_rr] = [h_ll, h_lr, h_rl, h_rr].map(widen);
    let beta = f64::from(beta);
    let max_gain_linear = f64::from(max_gain_linear);
    let limit = |values: (Complex<f64>, Complex<f64>, Complex<f64>, Complex<f64>)| {
        let [ll, rl] = soft_limit_column_f64([values.0, values.2], max_gain_linear);
        let [lr, rr] = soft_limit_column_f64([values.1, values.3], max_gain_linear);
        (ll, lr, rl, rr)
    };
    let pack = |values: (Complex<f64>, Complex<f64>, Complex<f64>, Complex<f64>)| {
        let narrow = |c: Complex<f64>| Complex::new(c.re as f32, c.im as f32);
        (
            narrow(values.0),
            narrow(values.1),
            narrow(values.2),
            narrow(values.3),
        )
    };
    // 1. Compute A = C^H * C + beta * I
    // C^H = [[h_ll*, h_rl*],
    //        [h_lr*, h_rr*]]
    //
    // A_00 = h_ll* * h_ll + h_rl* * h_rl + beta
    // A_01 = h_ll* * h_lr + h_rl* * h_rr
    // A_10 = h_lr* * h_ll + h_rr* * h_rl = A_01*
    // A_11 = h_lr* * h_lr + h_rr* * h_rr + beta

    let a_00 = h_ll.norm_sqr() + h_rl.norm_sqr() + beta;
    let a_01 = h_ll.conj() * h_lr + h_rl.conj() * h_rr;
    let a_10 = a_01.conj();
    let a_11 = h_lr.norm_sqr() + h_rr.norm_sqr() + beta;

    // 2. Compute inv(A) = (1/det) * [[a_11, -a_01], [-a_10, a_00]]
    let det = a_00 * a_11 - a_01.norm_sqr();
    if det == 0.0 || det.abs() < 1e-10 * (a_00 * a_11).abs() {
        return pack(limit((
            Complex::new(1.0, 0.0),
            Complex::new(0.0, 0.0),
            Complex::new(0.0, 0.0),
            Complex::new(1.0, 0.0),
        )));
    }

    let inv_a_00 = a_11 / det;
    let inv_a_01 = -a_01 / det;
    let inv_a_10 = -a_10 / det;
    let inv_a_11 = a_00 / det;

    // 3. W = inv(A) * C^H
    // C^H = [[h_ll*, h_rl*],
    //        [h_lr*, h_rr*]]
    //
    // W_ll = inv_a_00 * h_ll* + inv_a_01 * h_lr*
    // W_lr = inv_a_00 * h_rl* + inv_a_01 * h_rr*
    // W_rl = inv_a_10 * h_ll* + inv_a_11 * h_lr*
    // W_rr = inv_a_10 * h_rl* + inv_a_11 * h_rr*

    let w1_ll = inv_a_00 * h_ll.conj() + inv_a_01 * h_lr.conj();
    let w1_lr = inv_a_00 * h_rl.conj() + inv_a_01 * h_rr.conj();
    let w1_rl = inv_a_10 * h_ll.conj() + inv_a_11 * h_lr.conj();
    let w1_rr = inv_a_10 * h_rl.conj() + inv_a_11 * h_rr.conj();

    if bypass_neumann {
        return pack(limit((w1_ll, w1_lr, w1_rl, w1_rr)));
    }

    // Neumann series refinement: W₂ = W₁ * (2I - C*W₁)
    // C = [[h_ll, h_lr], [h_rl, h_rr]]
    // W1 = [[w1_ll, w1_lr], [w1_rl, w1_rr]]

    // Compute C * W1
    let cw_00 = h_ll * w1_ll + h_lr * w1_rl;
    let cw_01 = h_ll * w1_lr + h_lr * w1_rr;
    let cw_10 = h_rl * w1_ll + h_rr * w1_rl;
    let cw_11 = h_rl * w1_lr + h_rr * w1_rr;

    // R = 2I - C * W1
    let r_00 = Complex::new(2.0, 0.0) - cw_00;
    let r_01 = Complex::new(0.0, 0.0) - cw_01;
    let r_10 = Complex::new(0.0, 0.0) - cw_10;
    let r_11 = Complex::new(2.0, 0.0) - cw_11;

    // W2 = W1 * R
    // W2[0,0] = w1_ll * r_00 + w1_lr * r_10
    // W2[0,1] = w1_ll * r_01 + w1_lr * r_11
    // W2[1,0] = w1_rl * r_00 + w1_rr * r_10
    // W2[1,1] = w1_rl * r_01 + w1_rr * r_11
    let w2_ll_full = w1_ll * r_00 + w1_lr * r_10;
    let w2_lr_full = w1_ll * r_01 + w1_lr * r_11;
    let w2_rl_full = w1_rl * r_00 + w1_rr * r_10;
    let w2_rr_full = w1_rl * r_01 + w1_rr * r_11;

    // Dampen refinement: blend 70% between first-order (w1) and full refinement (w2).
    let w2_ll = w1_ll + (w2_ll_full - w1_ll) * 0.7;
    let w2_lr = w1_lr + (w2_lr_full - w1_lr) * 0.7;
    let w2_rl = w1_rl + (w2_rl_full - w1_rl) * 0.7;
    let w2_rr = w1_rr + (w2_rr_full - w1_rr) * 0.7;

    let (w1_ll_limit, w1_lr_limit, w1_rl_limit, w1_rr_limit) = limit((w1_ll, w1_lr, w1_rl, w1_rr));
    let (w2_ll_limit, w2_lr_limit, w2_rl_limit, w2_rr_limit) = limit((w2_ll, w2_lr, w2_rl, w2_rr));

    let identity = Complex::new(1.0, 0.0);
    // Error for W1 = C*W1 - I
    let err1_00 = h_ll * w1_ll_limit + h_lr * w1_rl_limit - identity;
    let err1_01 = h_ll * w1_lr_limit + h_lr * w1_rr_limit;
    let err1_10 = h_rl * w1_ll_limit + h_rr * w1_rl_limit;
    let err1_11 = h_rl * w1_lr_limit + h_rr * w1_rr_limit - identity;
    let err1_sq = err1_00.norm_sqr() + err1_01.norm_sqr() + err1_10.norm_sqr() + err1_11.norm_sqr();

    // Error for W2 = C*W2 - I
    let err2_00 = h_ll * w2_ll_limit + h_lr * w2_rl_limit - identity;
    let err2_01 = h_ll * w2_lr_limit + h_lr * w2_rr_limit;
    let err2_10 = h_rl * w2_ll_limit + h_rr * w2_rl_limit;
    let err2_11 = h_rl * w2_lr_limit + h_rr * w2_rr_limit - identity;
    let err2_sq = err2_00.norm_sqr() + err2_01.norm_sqr() + err2_10.norm_sqr() + err2_11.norm_sqr();

    pack(if err2_sq <= err1_sq {
        (w2_ll_limit, w2_lr_limit, w2_rl_limit, w2_rr_limit)
    } else {
        (w1_ll_limit, w1_lr_limit, w1_rl_limit, w1_rr_limit)
    })
}

/// Compute condition-number based regularization parameter β(f).
///
/// β(f) = β_base × max(1, κ(f) / κ_target)
///
/// This automatically increases regularization at frequency bins where the
/// plant matrix is ill-conditioned (high condition number), without needing
/// manual low/high frequency boost parameters.
pub(crate) fn compute_beta_condition_number(
    h_ipsi: Complex<f32>,
    h_contra: Complex<f32>,
    freq: f32,
    params: &XtcPluginParams,
) -> f32 {
    // Symmetric plant matrix: C = [[h_ipsi, h_contra], [h_contra, h_ipsi]]
    let kappa = condition_number_2x2(h_ipsi, h_contra, h_contra, h_ipsi);
    let beta = params.beta_base * (kappa / params.kappa_target).max(1.0);
    apply_beta_freq_boosts(beta, freq, params)
}

/// Compute condition-number based beta for a full (asymmetric) 2x2 plant matrix.
pub(crate) fn compute_beta_condition_number_full(
    h00: Complex<f32>,
    h01: Complex<f32>,
    h10: Complex<f32>,
    h11: Complex<f32>,
    freq: f32,
    params: &XtcPluginParams,
) -> f32 {
    let kappa = condition_number_2x2(h00, h01, h10, h11);
    let beta = params.beta_base * (kappa / params.kappa_target).max(1.0);
    apply_beta_freq_boosts(beta, freq, params)
}

#[cfg(test)]
#[inline]
pub(crate) fn compute_path_length(distance: f32, theta: f32, ear_offset: f32) -> f32 {
    ((distance * theta.sin() + ear_offset).powi(2) + (distance * theta.cos()).powi(2)).sqrt()
}

#[cfg(test)]
mod singular_plant_tests {
    use super::*;

    #[test]
    fn zero_plant_has_explicit_singular_and_regularized_outcomes() {
        let zero = Complex::new(0.0, 0.0);
        let one = Complex::new(1.0, 0.0);
        assert_eq!(
            compute_full_2x2_inverse(zero, zero, zero, zero, 0.0, 4.0, true),
            (one, zero, zero, one)
        );
        for ceiling in [0.125, 0.5, 1.0, 4.0] {
            let (ll, lr, rl, rr) =
                compute_full_2x2_inverse(zero, zero, zero, zero, 0.0, ceiling, true);
            assert!(ll.norm() <= ceiling && rr.norm() <= ceiling);
            assert_eq!((lr, rl), (zero, zero));
            assert_eq!(ll, rr);
        }
        // Positive beta makes the Gram matrix invertible even at small scale;
        // its zero adjoint yields zero, rather than an absolute-threshold fallback.
        for beta in [0.000001, 0.01] {
            assert_eq!(
                compute_full_2x2_inverse(zero, zero, zero, zero, beta, 4.0, true),
                (zero, zero, zero, zero)
            );
        }
    }
}
