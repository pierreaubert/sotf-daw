// Rust guideline compliant 2026-02-21
fn prepare_linked_stereo_detector(accum: &[Vec<f32>], detector: &mut [f32]) {
    let mut left_energy = 0.0_f32;
    let mut right_energy = 0.0_f32;
    let mut cross = 0.0_f32;
    for (&left, &right) in accum[0].iter().zip(&accum[1]) {
        left_energy += left * left;
        right_energy += right * right;
        cross += left * right;
    }
    let polarity = if cross < 0.0 { -1.0 } else { 1.0 };
    let mut combined_energy = 0.0_f32;
    let mut combined_peak = 0.0_f32;
    for (&left, &right) in accum[0].iter().zip(&accum[1]) {
        let combined = left + polarity * right;
        combined_energy += combined * combined;
        if combined.abs() > combined_peak.abs() {
            combined_peak = combined;
        }
    }
    // For negative correlation, L-R changes sign under a channel swap.
    // Anchor the detector to the louder channel, with a positive-peak tie
    // convention for equal energies. Both channel orders then supply exactly
    // the same detector waveform, including across correlation sign changes.
    // Only the neural detector is oriented; original speaker channels retain
    // their polarity and receive the same shared model gains as before.
    let reference_sign = if polarity < 0.0
        && (right_energy > left_energy || (right_energy == left_energy && combined_peak < 0.0))
    {
        -1.0
    } else {
        1.0
    };
    let target_energy = 0.5 * (left_energy + right_energy);
    let scale = if combined_energy > 1.0e-20 {
        (target_energy / combined_energy).sqrt()
    } else {
        0.0
    };
    for ((sample, &left), &right) in detector.iter_mut().zip(&accum[0]).zip(&accum[1]) {
        *sample = ((left + polarity * right) * (scale * reference_sign)).clamp(-1.0, 1.0) * 32768.0;
    }
}

#[test]
fn crossing_equal_energies_cannot_invert_the_neural_waveform() {
    let left: Vec<f32> = (0..480).map(|i| 0.1 * (std::f32::consts::TAU * 3.0 * i as f32 / 480.0).sin()).collect();
    let a = vec![left.clone(), left.iter().map(|x| -0.999 * x).collect()];
    let b = vec![left.clone(), left.iter().map(|x| -1.001 * x).collect()];
    let mut first = vec![0.0; 480]; let mut second = first.clone();
    prepare_linked_stereo_detector(&a, &mut first);
    prepare_linked_stereo_detector(&b, &mut second);
    let difference = first.iter().zip(&second).map(|(a,b)| f64::from(a-b).powi(2)).sum::<f64>();
    let reference = first.iter().map(|a| f64::from(*a).powi(2)).sum::<f64>();
    let relative = (difference/reference).sqrt();
    println!("speech_energy_crossing relative_detector_delta={relative}");
    assert!(relative < 0.01, "a 0.2% channel-level change must not invert the detector");
}
