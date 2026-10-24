// Rust guideline compliant 2026-02-21
pub use super::config::*;
use super::filters::{HrtfTransferFunctions, XtcFilters};
use super::misc::fir_taps_to_half_spectrum;
use super::types::RoomeqRecommendedMatrix;
use rustfft::num_complex::Complex;
use sotf_host::sofa::{SourcePosition, load_sofa};

/// Load HRTF data from a SOFA file and compute frequency-domain transfer functions
/// for the XTC plant matrix at the configured speaker angles.
///
/// Fixed world speaker positions are transformed into the translated/yawed head
/// frame before selecting each measured response. SOFA azimuth is positive left;
/// XTC yaw is positive right. Both ears retain ownership within each source column.
pub(super) fn load_hrtf_for_xtc(
    hrtf_path: &str,
    params: &XtcPluginParams,
    sample_rate: impl Into<f64>,
    num_bins: usize,
) -> Result<Option<HrtfTransferFunctions>, String> {
    let sample_rate = sample_rate.into();
    let path = std::path::Path::new(hrtf_path);
    if !path.exists() {
        return Err(format!("HRTF file not found: {}", hrtf_path));
    }

    let loaded = load_sofa(path)?;
    let delay_rebase_seconds = loaded.delay_rebase_seconds;
    let sofa = loaded.data;

    // Reject SOFA files with missing or mismatched sample rate.
    // A missing sample rate means all spectral features could be shifted in
    // frequency (e.g., a 44.1 kHz file loaded at 48 kHz shifts notches by ~8.8 %).
    let sofa_sr = sofa.data_sample_rate.ok_or_else(|| {
        "HRTF SOFA file does not declare a sample rate. \
         Resample the file and ensure it carries a valid DataSamplingRate attribute."
            .to_string()
    })?;
    if (sofa_sr - sample_rate).abs() > 1.0 {
        return Err(format!(
            "SOFA sample rate ({} Hz) differs from plugin sample rate ({} Hz). \
             Resample the SOFA file or match sample rates.",
            sofa_sr, sample_rate
        ));
    }

    // Match the analytical geometry's fixed world speakers and right/forward
    // listener frame. Loading the same +/- angle at every pose silently froze
    // measured-HRTF head tracking even while fresh filter epochs were prepared.
    let distance = f64::from(params.distance_m);
    let angle = f64::from(params.speaker_angle_deg).to_radians();
    let (sin_yaw, cos_yaw) = f64::from(params.head_yaw_deg).to_radians().sin_cos();
    let relative = [-1.0, 1.0].map(|side| {
        [
            side * distance * angle.sin() - f64::from(params.head_offset_x),
            distance * angle.cos() - f64::from(params.head_offset_z),
        ]
    });
    if relative
        .iter()
        .any(|p| !p[0].hypot(p[1]).is_finite() || p[0].hypot(p[1]) == 0.0)
    {
        return Err("Measured HRTF speaker direction is undefined at the listener position".into());
    }
    let [left_speaker, right_speaker] = relative.map(|[x, forward]| {
        let head_right = x * cos_yaw - forward * sin_yaw;
        let head_forward = x * sin_yaw + forward * cos_yaw;
        SourcePosition::new(
            (-head_right).atan2(head_forward).to_degrees() as f32,
            0.0,
            x.hypot(forward) as f32,
        )
    });

    // Get HRTF for left speaker position (contains left ear + right ear responses)
    let hrtf_left_speaker = sofa
        .get_hrtf_at_position(&left_speaker)
        .ok_or_else(|| "No HRTF measurement found for left speaker angle".to_string())?;

    // Get HRTF for right speaker position
    let hrtf_right_speaker = sofa
        .get_hrtf_at_position(&right_speaker)
        .ok_or_else(|| "No HRTF measurement found for right speaker angle".to_string())?;

    // FFT the impulse responses to get frequency-domain transfer functions
    let fft_size = (num_bins - 1) * 2;
    // Preserve legacy raw/zero-delay/cache truncation. Newly materialized delays
    // must not move selected plant energy outside the FFT and silently lose it.
    // Long unselected measurements and pure zero padding do not affect this plant.
    if loaded.delay_applied
        && [
            &hrtf_left_speaker.ir_left,
            &hrtf_left_speaker.ir_right,
            &hrtf_right_speaker.ir_left,
            &hrtf_right_speaker.ir_right,
        ]
        .iter()
        .any(|ir| {
            ir.get(fft_size..)
                .is_some_and(|tail| tail.iter().any(|&sample| sample != 0.0))
        })
    {
        return Err(format!(
            "Unsupported XTC SOFA capability: selected delayed HRIR has nonzero support beyond FFT size {fft_size}"
        ));
    }

    let mut planner = realfft::RealFftPlanner::new();
    let fft_forward = planner.plan_fft_forward(fft_size);

    // Helper: FFT an IR, zero-padding or truncating to fft_size
    let fft_ir = |ir: &[f32]| -> Vec<Complex<f32>> {
        let mut padded = vec![0.0_f32; fft_size];
        let copy_len = ir.len().min(fft_size);
        padded[..copy_len].copy_from_slice(&ir[..copy_len]);

        let mut output = vec![Complex::new(0.0, 0.0); num_bins];
        fft_forward
            .process(&mut padded, &mut output)
            .expect("FFT processing failed");
        output
    };

    // Plant matrix:
    //   C = [[h_ll, h_lr],    Speaker L->EarL, Speaker R->EarL
    //        [h_rl, h_rr]]    Speaker L->EarR, Speaker R->EarR
    //
    // Left speaker HRTF: ir_left = L speaker -> L ear, ir_right = L speaker -> R ear
    // Right speaker HRTF: ir_left = R speaker -> L ear, ir_right = R speaker -> R ear
    let h_ll = fft_ir(&hrtf_left_speaker.ir_left); // Speaker L -> Left ear
    let h_rl = fft_ir(&hrtf_left_speaker.ir_right); // Speaker L -> Right ear
    let h_lr = fft_ir(&hrtf_right_speaker.ir_left); // Speaker R -> Left ear
    let h_rr = fft_ir(&hrtf_right_speaker.ir_right); // Speaker R -> Right ear

    Ok(Some(HrtfTransferFunctions {
        h_ll,
        h_lr,
        h_rl,
        h_rr,
        delay_rebase_seconds,
    }))
}

pub(super) fn load_roomeq_recommended_filters(
    artifact_path: &str,
    sample_rate: impl Into<f64>,
    num_bins: usize,
) -> Result<XtcFilters, String> {
    let sample_rate = sample_rate.into();
    let bytes = std::fs::read(artifact_path)
        .map_err(|e| format!("Failed to read roomEQ recommended matrix: {}", e))?;
    let artifact: RoomeqRecommendedMatrix = serde_json::from_slice(&bytes)
        .map_err(|e| format!("Invalid roomEQ recommended matrix JSON: {}", e))?;
    if f64::from(artifact.sample_rate) != sample_rate {
        return Err(format!(
            "roomEQ recommended matrix sample rate {} Hz differs from plugin sample rate {} Hz",
            artifact.sample_rate, sample_rate
        ));
    }
    if artifact.speakers.len() < 2 {
        return Err(format!(
            "roomEQ recommended matrix must contain at least two speakers, got {}",
            artifact.speakers.len()
        ));
    }
    if artifact.ears.len() != 2 {
        return Err("roomEQ recommended matrix must contain exactly two ears".to_string());
    }

    let left_speaker = artifact.speakers[0].as_str();
    let right_speaker = artifact.speakers[1].as_str();
    let left_ear = artifact.ears[0].as_str();
    let right_ear = artifact.ears[1].as_str();

    let get_filter = |speaker: &str, target_ear: &str| -> Result<Vec<Complex<f32>>, String> {
        let filter = artifact
            .filters
            .iter()
            .find(|filter| filter.speaker == speaker && filter.target_ear == target_ear)
            .ok_or_else(|| {
                format!(
                    "roomEQ recommended matrix missing filter speaker='{}', target_ear='{}'",
                    speaker, target_ear
                )
            })?;
        fir_taps_to_half_spectrum(&filter.taps, num_bins)
    };

    let mut speaker_filters = Vec::with_capacity(artifact.speakers.len());
    for speaker in &artifact.speakers {
        speaker_filters.push([
            get_filter(speaker, left_ear)?,
            get_filter(speaker, right_ear)?,
        ]);
    }

    Ok(XtcFilters {
        filter_ll: get_filter(left_speaker, left_ear)?,
        filter_lr: get_filter(left_speaker, right_ear)?,
        filter_rl: Some(get_filter(right_speaker, left_ear)?),
        filter_rr: Some(get_filter(right_speaker, right_ear)?),
        is_symmetric: false,
        speaker_filters: Some(speaker_filters),
    })
}

pub(super) fn validate_roomeq_recommended_source(
    params: &XtcPluginParams,
    sample_rate: f64,
    num_bins: usize,
) -> Result<(), String> {
    if params.source_mode != "roomeq_recommended" {
        return Ok(());
    }
    let matrix_path = params.recommended_matrix_file.as_deref().ok_or_else(|| {
        "source_mode='roomeq_recommended' requires recommended_matrix_file".to_string()
    })?;
    load_roomeq_recommended_filters(matrix_path, sample_rate, num_bins).map(|_| ())
}
