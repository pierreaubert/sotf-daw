//! Offline centered reconstruction of fully emitted, zero-extended channel audio.

// Rust guideline compliant 2026-02-21
use std::f64::consts::PI;

pub fn db(value: f64) -> f64 {
    20.0 * value.max(1e-30).log10()
}
pub fn sample_peak(x: &[f32], channels: usize, ch: usize) -> f64 {
    x.iter()
        .skip(ch)
        .step_by(channels)
        .map(|&x| f64::from(x).abs())
        .fold(0.0, f64::max)
}
pub fn reconstructed_peak(x: &[f32], channels: usize, ch: usize, rate: u32) -> f64 {
    let factor = if rate < 96_000 {
        4
    } else if rate < 192_000 {
        2
    } else {
        1
    };
    let mut peak = sample_peak(x, channels, ch);
    if factor == 1 {
        return peak;
    }
    let radius = 24 / factor;
    // Evaluate a centered continuous Hann-windowed sinc directly. This uses
    // no detector, production coefficient table, causal history or meter data.
    let kernels: Vec<Vec<(isize, f64)>> = (1..factor)
        .map(|phase| {
            (-radius as isize..=radius as isize + 1)
                .filter_map(|offset| {
                    let t = phase as f64 / factor as f64 - offset as f64;
                    if t.abs() > radius as f64 {
                        return None;
                    }
                    let sinc = if t == 0.0 {
                        1.0
                    } else {
                        (PI * t).sin() / (PI * t)
                    };
                    Some((offset, sinc * 0.5 * (1.0 + (PI * t / radius as f64).cos())))
                })
                .collect()
        })
        .collect();
    let frames = x.len() / channels;
    for time in -(radius as isize)..frames as isize + radius as isize {
        for kernel in &kernels {
            let value = kernel
                .iter()
                .map(|&(offset, weight)| {
                    let index = time + offset;
                    if (0..frames as isize).contains(&index) {
                        f64::from(x[index as usize * channels + ch]) * weight
                    } else {
                        0.0
                    }
                })
                .sum::<f64>();
            peak = peak.max(value.abs());
        }
    }
    peak
}
