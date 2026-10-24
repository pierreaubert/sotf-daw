//! Component traces retain clean/noise ground truth without choosing a remedy.

// Rust guideline compliant 2026-02-21
use crate::{DenoiserPlugin, DenoiserPluginParams};
use sotf_host::{ParametricInPlacePlugin, ProcessContext};

fn input() -> (Vec<f32>, Vec<f64>) {
    let bytes = include_bytes!("../../sotf-plugin-de-esser/tests/fixtures/recorded/s.pcm");
    let mut state = 0x12345678_u32;
    let mut random = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        (f64::from(state) / f64::from(u32::MAX) * 2.0 - 1.0) as f32
    };
    let mut mixed = Vec::with_capacity(96_000);
    let mut clean = Vec::with_capacity(48_000);
    for frame in 0..48_000 {
        let i = (frame + 12_000) * 2;
        let sample = i16::from_le_bytes([bytes[i], bytes[i + 1]]);
        let signal = f32::from(sample) / 32768.0 * 0.2;
        clean.push(f64::from(sample) / 32768.0 * 0.2);
        let amplitude = if frame > 12_000 && frame < 36_000 {
            0.1
        } else {
            0.001
        };
        mixed.push(signal + random() * amplitude);
        mixed.push(random() * (0.002 + 0.003 * frame as f32 / 48_000.0));
    }
    (mixed, clean)
}

// Direct f64 DFT of the independently defined source window. This does not use
// the production transform, window, stored PSD or gain computation.
fn powers(clean: &[f64], mixed: &[f32], end: usize, size: usize, bin: usize) -> (f64, f64, f64) {
    let mut real = [0.0_f64; 3];
    let mut imaginary = [0.0_f64; 3];
    for n in 0..size {
        let source = end as isize - size as isize + n as isize;
        let (speech, total) = if source >= 0 && (source as usize) < clean.len() {
            (
                clean[source as usize],
                f64::from(mixed[source as usize * 2]),
            )
        } else {
            (0.0, 0.0)
        };
        let window = (std::f64::consts::PI * n as f64 / size as f64).sin();
        let angle = std::f64::consts::TAU * bin as f64 * n as f64 / size as f64;
        let (sine, cosine) = angle.sin_cos();
        for (index, value) in [speech, total - speech, total].into_iter().enumerate() {
            real[index] += value * window * cosine;
            imaginary[index] -= value * window * sine;
        }
    }
    let values: [f64; 3] = std::array::from_fn(|i| real[i] * real[i] + imaginary[i] * imaginary[i]);
    (values[0], values[1], values[2])
}

#[test]
fn recorded_noise_step_component_traces_preserve_independent_spectral_ground_truth() {
    let (source, clean) = input();
    let times = [0.0_f64, 0.1, 0.2, 0.26, 0.3, 0.375, 0.45, 0.6, 0.9];
    let mut records = 0;
    let mut maximum_power_error = 0.0_f64;
    for low_latency in [false, true] {
        let mut baseline_psd = Vec::new();
        for (variant, dd, smoothing, formant) in [
            ("default", true, true, false),
            ("instantaneous_snr", false, true, false),
            ("no_gaussian_smoothing", true, false, false),
            ("instantaneous_no_gaussian", false, false, false),
            ("formant", true, true, true),
        ] {
            let mut plugin = DenoiserPlugin::from_params(
                2,
                DenoiserPluginParams {
                    low_latency,
                    dd_enabled: dd,
                    spectral_smoothing_enabled: smoothing,
                    formant_preservation: formant,
                    formant_strength: 1.0,
                    spatial_denoise: false,
                    polyphonic_detection: false,
                    psychoacoustic_masking: false,
                    ..Default::default()
                },
            );
            plugin.initialize(48_000.0).unwrap();
            let size = plugin.config.fft_size;
            let hop = plugin.config.hop_size;
            let mut next_time = 0;
            let mut end = 0;
            for (frame, input) in source.chunks(hop * 2).enumerate() {
                let previous_gain = plugin.gains.smoothed_gain[0].clone();
                let previous_power = plugin.decision_directed.prev_power[0].clone();
                let mut block = input.to_vec();
                let block_frames = block.len() / 2;
                plugin
                    .process_in_place(&mut block, &ProcessContext::new(48_000, block_frames))
                    .unwrap();
                end += input.len() / 2;
                if input.len() != hop * 2 {
                    continue;
                }
                if variant == "default" {
                    baseline_psd.push(plugin.mcra.noise_psd[0].clone());
                } else {
                    assert_eq!(
                        plugin.mcra.noise_psd[0], baseline_psd[frame],
                        "gain options must not change the source estimator"
                    );
                }
                let center = end.saturating_sub(hop) as f64 / 48_000.0;
                if next_time >= times.len() || center < times[next_time] {
                    continue;
                }
                let requested = times[next_time];
                next_time += 1;
                let active = plugin.mcra.frame_counter[0] >= plugin.mcra.bootstrap_frames;
                let first = (100.0 * size as f64 / 48_000.0).ceil() as usize;
                let last = (4000.0 * size as f64 / 48_000.0).floor() as usize;
                // Select the strongest clean-source bins, never by output or
                // observed gain, and retain their independent noise components.
                let mut bins: Vec<_> = (first..=last)
                    .map(|bin| (bin, powers(&clean, &source, end, size, bin)))
                    .collect();
                bins.sort_by(|a, b| b.1.0.total_cmp(&a.1.0));
                for (bin, (speech, noise, total)) in bins.into_iter().take(8) {
                    let observed = f64::from(plugin.decision_directed.prev_power[0][bin]);
                    if active {
                        let error = (observed - total).abs() / (1.0 + total);
                        maximum_power_error = maximum_power_error.max(error);
                        assert!(error < 2e-6, "independent observed-power mismatch {error}");
                    }
                    let estimated_noise = f64::from(plugin.mcra.noise_psd[0][bin]).max(1e-10);
                    let ml = (observed / estimated_noise - 1.0).max(0.0);
                    let prediction = f64::from(previous_gain[bin]).powi(2)
                        * f64::from(previous_power[bin])
                        / estimated_noise;
                    let prior = if dd {
                        f64::from(plugin.decision_directed.dd_alpha) * prediction
                            + (1.0 - f64::from(plugin.decision_directed.dd_alpha)) * ml
                    } else {
                        ml
                    };
                    let raw_gain = (prior / (prior + f64::from(plugin.coeffs.reduction_linear)))
                        .max(f64::from(plugin.coeffs.floor_linear));
                    println!(
                        "COMPONENT {}",
                        serde_json::json!({"variant":variant,"low_latency":low_latency,"requested_source_time":requested,"source_window_center":center,"bin":bin,"frequency_hz":bin as f64*48_000.0/size as f64,"gain_frame_active":active,"clean_periodogram":speech,"true_noise_periodogram":noise,"mixed_periodogram":total,"observed_gain_power":observed,"estimated_noise":estimated_noise,"speech_probability":plugin.mcra.speech_presence[0][bin],"ml_prior_snr":ml,"decision_prediction":prediction,"raw_wiener_gain_before_median":if active {Some(raw_gain)} else {None},"final_gain":plugin.gains.smoothed_gain[0][bin]})
                    );
                    records += 1;
                }
            }
            assert_eq!(next_time, times.len());
        }
    }
    println!(
        "component_trace_summary records={records} maximum_scaled_power_error={maximum_power_error:e} estimator_invariant=true"
    );
    assert_eq!(records, 720);
}
