use super::super::config::XtcPluginParams;
use super::super::filters::{
    compute_geometry_cache, compute_xtc_filters_full, head_shadowing_for_plant,
};
use super::measure::{diagnostic_config_supported, prediction_with_filters, scalar_depth};
use super::misc::CANCELLATION_DEPTH_TARGETS;
use super::misc::REFERENCE_ILD_POINTS;
use super::reference::reference_itd_ms;
use super::validation_report::ValidationReport;
use super::validation_result::ValidationResult;

/// Compare analytical diagnostics with nominal acoustic reference targets.
///
/// Returns pass/fail status for every metric. This is not a measured-listener
/// validation suite. A caller must inspect the complete report; stability and
/// symmetry alone do not establish ILD or cancellation accuracy.
///
/// Optimization 2: Computes filters once and reuses for all validation checks.
pub fn run_validation(params: &XtcPluginParams, sample_rate: u32) -> ValidationReport {
    if sample_rate == 0 || !diagnostic_config_supported(params) {
        return ValidationReport::new(vec![ValidationResult::check(
            "Cancellation model support (synthetic, no resource snapshot)",
            1.0,
            0.0,
            0.0,
        )]);
    }
    let mut results = Vec::new();

    // Pre-compute filters once for all validation checks (Optimization 2)
    let fft_size = params.fft_size;
    let num_bins = fft_size / 2 + 1;
    let filters = compute_xtc_filters_full(params, sample_rate, num_bins);

    // 1. ITD validation - ITD is derived from geometry, so validate the setup
    // A correct XTC setup should have ITD matching the Woodworth formula
    let expected_itd = reference_itd_ms(params.speaker_angle_deg, params.head_radius_m);
    let geometry = compute_geometry_cache(params, sample_rate, num_bins);
    let measured_itd = (geometry.symmetric.delay_contra - geometry.symmetric.delay_ipsi) * 1_000.0;
    results.push(ValidationResult::check(
        "ITD (ms) - Geometry Check",
        expected_itd,
        measured_itd,
        0.02,
    ));

    // 2. ILD validation at key frequencies
    // Note: ILD varies significantly with individual anatomy, so we use wider tolerances
    for &(freq, expected_ild) in REFERENCE_ILD_POINTS {
        // Nominal head-shadow magnitude for the selected model, not measured
        // binaural ILD (which also includes pinna, geometry and room response).
        let shadow = head_shadowing_for_plant(
            freq,
            geometry.symmetric.contra_angle,
            params.head_radius_m,
            params.head_model,
            params.head_shadow_cutoff_hz,
            params.head_shadow_slope_db_per_octave,
        )
        .norm();
        let measured_ild = -20.0 * shadow.max(1e-6).log10();
        results.push(ValidationResult::check(
            &format!("ILD @ {}Hz (dB)", freq),
            expected_ild,
            measured_ild,
            20.0,
        ));
    }

    // 3. Cancellation depth validation (using pre-computed filters)
    for &(freq, min_depth, _optimal) in CANCELLATION_DEPTH_TARGETS {
        let prediction = prediction_with_filters(&filters, params, sample_rate, freq, num_bins);
        let name = match &prediction {
            Some(prediction) => format!(
                "Cancellation @ {}Hz (bin {}Hz, dB)",
                freq, prediction.frequency_hz
            ),
            None => format!("Cancellation @ {}Hz (unsupported bin, dB)", freq),
        };
        let measured = prediction
            .map(|p| scalar_depth(p.metrics))
            .unwrap_or(f32::NAN);
        results.push(ValidationResult::check_min(&name, min_depth, measured));
    }

    // 4. Spatial cue preservation (symmetry check for zero yaw)
    if params.head_yaw_deg.abs() < 0.1 {
        results.push(ValidationResult::check(
            "Filter Symmetry",
            1.0,
            if filters.is_symmetric { 1.0 } else { 0.0 },
            0.0,
        ));
    }

    // 5. Filter stability (magnitude bounds)
    let max_mag: f32 = filters
        .filter_ll
        .iter()
        .chain(&filters.filter_lr)
        .chain(filters.filter_rl.iter().flatten())
        .chain(filters.filter_rr.iter().flatten())
        .map(|c| c.norm())
        .fold(0.0, |a, b| {
            if a.is_finite() && b.is_finite() {
                a.max(b)
            } else {
                f32::NAN
            }
        });
    let max_gain_linear = 10.0_f32.powf(params.max_gain_db / 20.0);
    // Filter stability: max magnitude should not exceed the configured limit
    // Measured should be <= expected for stability
    results.push(ValidationResult::check_min(
        "Filter Stability (max magnitude)",
        0.0, // min value (just needs to be finite)
        if max_mag.is_finite() {
            max_mag
        } else {
            f32::NAN
        },
    ));
    // Also check that filters are below the configured gain limit
    if max_mag <= max_gain_linear {
        results.push(ValidationResult::check(
            "Filter Gain Limit",
            max_gain_linear,
            max_mag,
            max_gain_linear, // Any value below limit is acceptable
        ));
    } else {
        results.push(ValidationResult::check(
            "Filter Gain Limit",
            max_gain_linear,
            max_mag,
            0.0, // Will fail since max_mag > max_gain_linear
        ));
    }

    ValidationReport::new(results)
}

/// Run validation with detailed output.
///
/// Prints a formatted report to stdout.
pub fn run_validation_verbose(params: &XtcPluginParams, sample_rate: u32) -> ValidationReport {
    println!("\n=== XTC Validation Report ===");
    println!(
        "Configuration: {}° speakers, {}m distance, {}cm head radius",
        params.speaker_angle_deg,
        params.distance_m,
        params.head_radius_m * 100.0
    );
    println!();

    let report = run_validation(params, sample_rate);

    for result in &report.results {
        println!("{}", result);
    }

    println!();
    println!(
        "Summary: {}/{} tests passed",
        report.passed_count,
        report.results.len()
    );

    if !report.categories_failed.is_empty() {
        println!("Failed categories: {:?}", report.categories_failed);
    }

    report
}
