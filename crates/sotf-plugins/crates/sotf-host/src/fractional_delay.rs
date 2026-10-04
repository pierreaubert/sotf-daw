//! Prepared fractional-sample delay coefficients shared by audio adapters.

/// Build a 65-tap normalized Hann-windowed sinc delay for a fraction in `[0, 1)`.
///
/// The filter's causal group delay is 32 samples plus `fraction`. Prepare the
/// coefficients off the audio thread and reuse them for every sample.
pub fn fractional_delay_coefficients(fraction: f64) -> [f32; 65] {
    let mut coefficients = [0.0_f64; 65];
    let mut sum = 0.0;
    for (index, coefficient) in coefficients.iter_mut().enumerate() {
        // Subtract the fraction from the small tap-relative offset rather than
        // forming `32 + fraction`, which rounds fractions near 0 or 1 away.
        let offset = (index as f64 - 32.0) - fraction;
        let argument = std::f64::consts::PI * offset;
        let sinc = if argument.abs() < 1.0e-8 {
            1.0 - argument * argument / 6.0
        } else {
            argument.sin() / argument
        };
        let window = 0.5 - 0.5 * (std::f64::consts::TAU * index as f64 / 64.0).cos();
        *coefficient = sinc * window;
        sum += *coefficient;
    }
    std::array::from_fn(|index| (coefficients[index] / sum) as f32)
}
