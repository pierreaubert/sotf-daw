// Rust guideline compliant 2026-02-21
fn main() {
    let mut maximum_error = 0.0_f64;
    for rate in [44_100.0_f32, 48_000.0, 96_000.0, 192_000.0] {
        for (index, frequency) in [0.07_f32, 0.11, 0.13, 0.17, 0.19, 0.23, 0.29, 0.31].into_iter().enumerate() {
            let initial_phase = index as f32 / 8.0;
            let mut phase = initial_phase;
            let increment = frequency / rate;
            for frame in 0..1_000_000 {
                let actual = (phase * std::f32::consts::TAU).sin();
                let expected = (std::f64::consts::TAU * (f64::from(initial_phase) + frame as f64 * f64::from(frequency) / f64::from(rate))).sin();
                maximum_error = maximum_error.max((f64::from(actual) - expected).abs());
                phase += increment;
                if phase >= 1.0 { phase -= 1.0; }
            }
        }
    }
    println!("{{\"cases\":32,\"samples\":32000000,\"maximum_error\":{maximum_error},\"gate\":0.00000006,\"passed\":{}}}", maximum_error < 6e-8);
    if maximum_error >= 6e-8 { std::process::exit(1); }
}
