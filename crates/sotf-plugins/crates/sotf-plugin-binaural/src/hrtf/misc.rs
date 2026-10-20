use rustfft::num_complex::Complex;
use sotf_host::sofa::{SofaFile, SourcePosition};

/// Calculate VBAP gains using barycentric interpolation
///
/// Uses barycentric coordinates to interpolate between 3 source positions.
/// This is more numerically stable than matrix inversion and provides better
/// spatial accuracy.
///
/// Reference: Pulkki, "Virtual Sound Source Positioning Using Vector Base Amplitude Panning"
pub fn calculate_vbap_gains(
    target: &SourcePosition,
    nearest: &[(usize, f32); 3],
    sofa: &SofaFile,
) -> [f32; 3] {
    let p = target.to_cartesian_unit_vector();

    let v0 = sofa.positions[nearest[0].0].to_cartesian_unit_vector();
    let v1 = sofa.positions[nearest[1].0].to_cartesian_unit_vector();
    let v2 = sofa.positions[nearest[2].0].to_cartesian_unit_vector();

    // Calculate barycentric coordinates using cross products
    // This is more stable than matrix inversion
    //
    // The barycentric coordinates (w0, w1, w2) satisfy:
    // p = w0*v0 + w1*v1 + w2*v2
    // w0 + w1 + w2 = 1
    //
    // Using the formula:
    // w0 = area(p,v1,v2) / area(v0,v1,v2)
    // w1 = area(v0,p,v2) / area(v0,v1,v2)
    // w2 = area(v0,v1,p) / area(v0,v1,v2)
    //
    // Where area is computed using cross product magnitude

    // Helper to compute cross product
    let cross = |a: [f32; 3], b: [f32; 3]| -> [f32; 3] {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };

    // Helper to compute dot product
    let dot = |a: [f32; 3], b: [f32; 3]| -> f32 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] };

    // Compute edge vectors
    let v01 = [v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]];
    let v02 = [v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]];
    let v0p = [p[0] - v0[0], p[1] - v0[1], p[2] - v0[2]];

    // Normal of triangle (v0, v1, v2)
    let n = cross(v01, v02);
    let n_dot_n = dot(n, n);

    // Check for degenerate triangle (collinear points)
    if n_dot_n < 1e-6 {
        log::warn!("[BinauralDecoder] Degenerate triangle detected, using nearest neighbor");
        return [1.0, 0.0, 0.0];
    }

    // Calculate barycentric coordinates
    // w1 corresponds to v1, w2 corresponds to v2
    let v02_cross_n = cross(v02, n);
    let n_cross_v01 = cross(n, v01);

    let w1 = dot(v02_cross_n, v0p) / n_dot_n;
    let w2 = dot(n_cross_v01, v0p) / n_dot_n;
    let w0 = 1.0 - w1 - w2;

    // Check if point is inside triangle (all weights non-negative)
    // If outside, clamp to valid range and warn
    let mut weights = [w0, w1, w2];
    if weights.iter().any(|&w| w < -0.01) {
        // Point is significantly outside triangle
        // This can happen with sparse HRTF measurements
        log::debug!(
            "[BinauralDecoder] Target outside triangle: weights=[{:.3}, {:.3}, {:.3}], clamping to boundary",
            w0,
            w1,
            w2
        );
        // Clamp negative weights to zero
        for w in &mut weights {
            if *w < 0.0 {
                *w = 0.0;
            }
        }

        // Renormalize so the clamped weights sum to 1.0.
        let sum: f32 = weights.iter().sum();
        if sum > 1e-6 {
            for w in &mut weights {
                *w /= sum;
            }
        } else {
            // All weights were negative, use nearest neighbor
            weights = [1.0, 0.0, 0.0];
        }
    }

    // These are interpolation weights for an HRTF field, not loudspeaker VBAP
    // gains. Preserve affine/constant fields by normalizing to unit sum.
    let sum: f32 = weights.iter().sum();
    if sum > 1e-6 {
        [weights[0] / sum, weights[1] / sum, weights[2] / sum]
    } else {
        [1.0, 0.0, 0.0]
    }
}

/// Applies a normalized contralateral head-shadow magnitude profile.
///
/// Coordinates are forward/left/up: positive azimuth denotes a left source.
/// The first-order shelf shape is inspired by structural head-shadow models,
/// normalized to unity in the median plane. This optional extra magnitude
/// heuristic adds no explicit ITD and is not a calibrated near-field HRTF.
/// Preparation later retains the complete real design response for convolution.
pub fn apply_near_field_shadowing<S: Into<f64>>(
    left_fft: &mut [Complex<f32>],
    right_fft: &mut [Complex<f32>],
    azimuth: f32,
    elevation: f32,
    fft_size: usize,
    sample_rate: S,
    near_field_strength: f32,
) {
    let sample_rate = sample_rate.into();
    // Keep the existing nominal head radius and shelf extremes. Normalize by
    // the median-plane shelf at each frequency so shaping cannot boost an ear.
    const HEAD_RADIUS: f64 = 0.0875;
    const SPEED_OF_SOUND: f64 = 343.0;
    const MEDIAN_ALPHA: f64 = 1.05;
    const SIDE_ALPHA: f64 = 0.1;
    const TAU: f64 = 2.0 * HEAD_RADIUS / SPEED_OF_SOUND;

    let lateral = f64::from(azimuth).to_radians().sin() * f64::from(elevation).to_radians().cos();
    let shadowed = if lateral > 0.0 { right_fft } else { left_fft };
    let alpha = MEDIAN_ALPHA - (MEDIAN_ALPHA - SIDE_ALPHA) * lateral.abs().min(1.0);
    // Lateral projection already includes elevation. It is zero on the
    // front/back median plane and at either pole, without an azimuth dead zone.
    for (k, value) in shadowed.iter_mut().enumerate().take(fft_size / 2 + 1) {
        let omega = 2.0 * std::f64::consts::PI * k as f64 * sample_rate / fft_size as f64;
        let x = TAU * omega;
        let ratio = (1.0 + (alpha * x).powi(2)) / (1.0 + (MEDIAN_ALPHA * x).powi(2));
        let gain = ratio.powf(f64::from(near_field_strength) * 0.5);
        *value *= gain as f32;
    }
}

/// Detect IR onset (ITD) using threshold-based method
///
/// Finds the first sample where the IR exceeds 10% of peak magnitude.
/// Returns the delay in seconds.
pub(super) fn detect_ir_onset<S: Into<f64>>(ir: &[f32], sample_rate: S) -> f32 {
    let sample_rate = sample_rate.into();
    if ir.is_empty() {
        return 0.0;
    }

    // Find peak magnitude
    let peak = ir.iter().map(|x| x.abs()).fold(0.0f32, f32::max);

    if peak < 1e-6 {
        return 0.0; // Silent IR
    }

    // Find first sample exceeding 10% of peak
    let threshold = peak * 0.1;
    for (i, &sample) in ir.iter().enumerate() {
        if sample.abs() >= threshold {
            return i as f32 / sample_rate as f32;
        }
    }

    0.0
}

/// Normalize HRTF gains to prevent clipping
///
/// Calculates the worst-case scenario (all input channels at full scale)
/// and normalizes all HRTFs to ensure the output stays within [-1, 1].
///
/// `freq_size` is the number of frequency bins (N/2+1 for half-spectrum).
/// HRTFs are stored as [left_freq_size | right_freq_size] per channel.
pub fn normalize_hrtf_gains(
    hrtf_filters_freq: &mut [Vec<Complex<f32>>],
    lfe_channels: &[usize],
    freq_size: usize,
    input_channels: usize,
) {
    let mut max_left_magnitude = 0.0f32;
    let mut max_right_magnitude = 0.0f32;

    // Find worst-case magnitude for each frequency bin
    // This is the sum of magnitudes when all channels play at full scale
    for k in 0..freq_size {
        let mut left_sum = 0.0f32;
        let mut right_sum = 0.0f32;

        for (ch, hrtf) in hrtf_filters_freq.iter().enumerate().take(input_channels) {
            // Skip LFE channels (they're mixed separately with -3dB gain)
            // Note: we don't have easy access to channel index here anymore if we fully removed it,
            // but we need 'ch' to check lfe_channels.
            // So let's revert to using enumerate
            if lfe_channels.contains(&ch) {
                continue;
            }

            let hrtf = &hrtf;
            left_sum += hrtf[k].norm(); // Magnitude
            right_sum += hrtf[k + freq_size].norm();
        }

        max_left_magnitude = max_left_magnitude.max(left_sum);
        max_right_magnitude = max_right_magnitude.max(right_sum);
    }

    // Include LFE contribution (mixed full-scale to both ears)
    let lfe_contribution = lfe_channels.len() as f32;
    max_left_magnitude += lfe_contribution;
    max_right_magnitude += lfe_contribution;

    // Find the maximum across both channels
    let max_magnitude = max_left_magnitude.max(max_right_magnitude);

    // Calculate normalization factor with headroom
    // Target peak of 0.95 (-0.44 dBFS) to leave headroom for:
    // - Numerical errors
    // - Externalization reflections
    // - Sample rate conversion artifacts
    let target_peak = 0.95;
    let normalization_factor = if max_magnitude > target_peak {
        target_peak / max_magnitude
    } else {
        1.0 // No normalization needed
    };

    if normalization_factor < 1.0 {
        log::info!(
            "[BinauralDecoder] Normalizing HRTFs by {:.3} ({:.2} dB) to prevent clipping (worst-case magnitude: {:.2})",
            normalization_factor,
            20.0 * normalization_factor.log10(),
            max_magnitude
        );

        // Apply normalization to all HRTFs
        for (ch, hrtf_samples) in hrtf_filters_freq
            .iter_mut()
            .enumerate()
            .take(input_channels)
        {
            // Skip LFE channels (they don't use HRTFs)
            if lfe_channels.contains(&ch) {
                continue;
            }

            for sample in hrtf_samples {
                *sample *= normalization_factor;
            }
        }
    } else {
        log::debug!(
            "[BinauralDecoder] No HRTF normalization needed (worst-case magnitude: {:.2})",
            max_magnitude
        );
    }
}

/// Selects stable interpolation neighbors for horizontal or spatial datasets.
///
/// A single-elevation dataset contains no measured vertical variation. Use
/// circular azimuth brackets instead of an out-of-plane triangle projection,
/// whose third-neighbor ties can cause discontinuities. Uncovered gaps wider
/// than a hemisphere use the nearest angular measurement.
pub fn find_hrtf_neighbors(
    sofa: &SofaFile,
    target: &SourcePosition,
) -> ([(usize, f32); 3], [f32; 3]) {
    // One f32 elevation ULP near a pole is about 8e-6 degrees. Smaller
    // differences cannot provide useful independent ring-elevation data.
    const HORIZONTAL_TOLERANCE_DEG: f32 = 1e-5;
    // Avoid ill-conditioned division between effectively identical azimuths.
    const MIN_BRACKET_SPAN_DEG: f32 = 1e-6;
    let horizontal = sofa.positions.first().is_some_and(|first| {
        sofa.positions
            .iter()
            .all(|p| (p.elevation - first.elevation).abs() < HORIZONTAL_TOLERANCE_DEG)
    });
    if horizontal {
        let mut before = None::<(usize, f32)>;
        let mut after = None::<(usize, f32)>;
        let mut closest = (0, f32::INFINITY);
        for (index, position) in sofa.positions.iter().enumerate() {
            let offset = (position.azimuth - target.azimuth + 180.0).rem_euclid(360.0) - 180.0;
            if offset.abs() < closest.1 {
                closest = (index, offset.abs());
            }
            if offset <= 0.0 && before.is_none_or(|(_, old)| offset > old) {
                before = Some((index, offset));
            }
            if offset >= 0.0 && after.is_none_or(|(_, old)| offset < old) {
                after = Some((index, offset));
            }
        }
        if let (Some((a, da)), Some((b, db))) = (before, after) {
            let span = db - da;
            if span > MIN_BRACKET_SPAN_DEG && span <= 180.0 {
                return (
                    [(a, da.abs()), (b, db.abs()), (a, da.abs())],
                    [db / span, -da / span, 0.0],
                );
            }
        }
        return ([(closest.0, closest.1); 3], [1.0, 0.0, 0.0]);
    }
    let near = sofa.find_three_nearest(target);
    let gains = calculate_vbap_gains(target, &near, sofa);
    (near, gains)
}
