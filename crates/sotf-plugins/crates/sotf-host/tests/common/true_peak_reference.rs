// Direct oversampled convolution from the published ITU interpolation table.
// Rust guideline compliant 2026-02-21

// BS.1770-5 Annex 2, printed pp. 18–19, row-major table. Each entry is an
// exact integer multiple of 1/32768. The table is deliberately represented
// differently from production's phase-major, oldest-to-newest coefficients.
// https://www.itu.int/dms_pubrec/itu-r/rec/bs/R-REC-BS.1770-5-202311-I!!PDF-E.pdf
const ROWS: [[i32; 4]; 12] = [
    [56, -956, -620, -272],
    [360, 960, 1084, 488],
    [-644, -1696, -1908, -872],
    [1088, 2920, 3328, 1560],
    [-1948, -5456, -6564, -3352],
    [4500, 15240, 25552, 31856],
    [31856, 25552, 15240, 4500],
    [-3352, -6564, -5456, -1948],
    [1560, 3328, 2920, 1088],
    [-872, -1908, -1696, -644],
    [488, 1084, 960, 360],
    [-272, -620, -956, 56],
];

/// Return the peak of each input frame's emitted interpolation samples.
pub fn frame_peaks(input: &[f32], factor: usize) -> Vec<f64> {
    assert!(matches!(factor, 2 | 4));
    let mut inserted = vec![0.0; input.len() * 4];
    for (i, &sample) in input.iter().enumerate() {
        inserted[i * 4] = f64::from(sample);
    }
    let mut peaks = vec![0.0_f64; input.len()];
    for time in (0..inserted.len()).step_by(4 / factor) {
        let mut sum = 0.0;
        for lag in 0..48.min(time + 1) {
            sum += inserted[time - lag] * f64::from(ROWS[lag / 4][lag % 4]) / 32768.0;
        }
        peaks[time / 4] = peaks[time / 4].max(sum.abs());
    }
    peaks
}
