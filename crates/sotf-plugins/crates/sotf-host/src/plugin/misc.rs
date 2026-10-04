pub(super) fn samples_to_ppq(sample_position: u64, sample_rate: f64, bpm: f64) -> f64 {
    if !sample_rate.is_finite() || sample_rate <= 0.0 || !bpm.is_finite() || bpm <= 0.0 {
        return 0.0;
    }
    sample_position as f64 / sample_rate * bpm / 60.0
}
