//! Captured profiles own both analysis grids and their prepared sample clock.

// Rust guideline compliant 2026-02-21
use super::*;
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};

fn processor(low_latency: bool, multi_resolution: bool, rate: u32) -> DenoiserPlugin {
    let mut p = DenoiserPlugin::from_params(
        2,
        DenoiserPluginParams {
            low_latency,
            multi_resolution,
            reduction_db: 20.0,
            floor_db: -40.0,
            spectral_smoothing_enabled: false,
            temporal_smoothing_enabled: false,
            dd_enabled: false,
            polyphonic_detection: false,
            psychoacoustic_masking: false,
            formant_preservation: false,
            spatial_denoise: false,
            harmonic_percussive: false,
            ..Default::default()
        },
    );
    p.initialize(f64::from(rate)).unwrap();
    p
}

fn noise(seed: &mut u32) -> f32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 17;
    *seed ^= *seed << 5;
    (*seed as f64 / f64::from(u32::MAX) * 2.0 - 1.0) as f32
}

fn render(p: &mut DenoiserPlugin, source: &[f32], rate: u32, block: usize) -> Vec<f32> {
    let mut output = source.to_vec();
    for chunk in output.chunks_mut(block * 2) {
        let frames = chunk.len() / 2;
        p.process_in_place(chunk, &ProcessContext::new(rate, frames))
            .unwrap();
    }
    let mut tail = vec![0.0; p.drain_output_frames_max() * 2];
    for _ in 0..32 {
        let step = p.drain(&mut tail, &ProcessContext::new(rate, 0)).unwrap();
        output.extend_from_slice(&tail[..step.frames * 2]);
        if step.complete {
            return output;
        }
    }
    panic!("drain did not finish");
}

#[test]
fn retained_profile_reduces_the_first_frame_while_live_estimation_bootstraps() {
    for low_latency in [false, true] {
        let mut p = processor(low_latency, false, 48_000);
        p.noise_profile.has_noise_profile = true;
        p.noise_profile.use_captured_profile = true;
        for profile in &mut p.noise_profile.noise_profile_storage {
            profile.fill(1e6);
        }
        p.reset();
        p.io.input_buffer_fill = p.config.fft_size * 2;
        p.process_fft_block(false).unwrap();
        let maximum = p
            .gains
            .smoothed_gain
            .iter()
            .flatten()
            .copied()
            .fold(0.0, f32::max);
        println!("profile_first_frame low_latency={low_latency} maximum_gain={maximum}");
        assert!(
            maximum <= 0.010001,
            "valid profile must override warm-up bypass"
        );
        p.noise_profile.use_captured_profile = false;
        p.reset();
        p.io.input_buffer_fill = p.config.fft_size * 2;
        p.process_fft_block(false).unwrap();
        assert!(
            p.gains
                .smoothed_gain
                .iter()
                .flatten()
                .all(|&gain| gain == 1.0),
            "disabled profile retains live bootstrap policy"
        );
    }
}

#[test]
fn captured_profile_is_retained_only_for_the_same_sample_clock() {
    let mut p = processor(false, true, 48_000);
    p.noise_profile.has_noise_profile = true;
    p.noise_profile.use_captured_profile = true;
    p.noise_profile.noise_profile_storage[0].fill(0.25);
    let storage = p.noise_profile.noise_profile_storage.clone();
    p.initialize(48_000.0).unwrap();
    assert!(p.noise_profile.has_noise_profile);
    assert_eq!(p.noise_profile.noise_profile_storage, storage);
    for invalid in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(p.initialize(invalid).is_err());
        assert!(p.noise_profile.has_noise_profile);
        assert_eq!(p.noise_profile.noise_profile_storage, storage);
    }
    p.initialize(96_000.0).unwrap();
    println!(
        "profile_new_clock has_profile={}",
        p.noise_profile.has_noise_profile
    );
    assert!(
        !p.noise_profile.has_noise_profile,
        "a binned profile belongs to its captured clock"
    );
}

#[test]
fn captured_silence_does_not_learn_wanted_audio_in_the_small_fft_arm() {
    for low_latency in [false, true] {
        for rate in [48_000, 96_000] {
            let mut p = processor(low_latency, true, rate);
            p.parametric_set_parameter(
                ParameterId::from("learn_noise"),
                ParameterValue::Bool(true),
            )
            .unwrap();
            let mut seed = 0x12345678;
            // Independent fixed profiles: left has captured silence, right noise.
            let capture: Vec<f32> = (0..rate as usize * 3 / 2)
                .flat_map(|_| [0.0, noise(&mut seed) * 0.2])
                .collect();
            let _ = render(&mut p, &capture, rate, 257);
            assert!(p.noise_profile.has_noise_profile && p.noise_profile.use_captured_profile);
            p.reset();
            let source: Vec<f32> = (0..rate as usize)
                .flat_map(|_| [noise(&mut seed) * 0.2, noise(&mut seed) * 0.2])
                .collect();
            let latency = p.latency_samples();
            let out = render(&mut p, &source, rate, 257);
            let gains: [f64; 2] = std::array::from_fn(|channel| {
                let start = rate as usize / 4;
                let end = rate as usize * 3 / 4;
                let before: f64 = (start..end)
                    .map(|n| f64::from(source[n * 2 + channel]).powi(2))
                    .sum();
                let after: f64 = (start..end)
                    .map(|n| f64::from(out[(n + latency) * 2 + channel]).powi(2))
                    .sum();
                (after / before).sqrt()
            });
            println!(
                "profile_small_fft rate={rate} low_latency={low_latency} retained_gain={} noise_gain={}",
                gains[0], gains[1]
            );
            assert!(
                gains[0] > 0.99,
                "captured silence must not be replaced by live wanted-audio learning"
            );
            assert!(
                gains[1] < 0.8,
                "the independent captured noise channel must still be reduced"
            );
            // The stored profile survives live-mode selection. Disabling it
            // must actually restore adaptive noise reduction; enabling it after
            // reset must reproduce the original fixed-profile waveform exactly.
            p.reset();
            p.parametric_set_parameter(
                ParameterId::from("use_captured_profile"),
                ParameterValue::Bool(false),
            )
            .unwrap();
            let live = render(&mut p, &source, rate, 257);
            let before: f64 = (rate as usize / 4..rate as usize * 3 / 4)
                .map(|n| f64::from(source[n * 2]).powi(2))
                .sum();
            let after: f64 = (rate as usize / 4..rate as usize * 3 / 4)
                .map(|n| f64::from(live[(n + latency) * 2]).powi(2))
                .sum();
            let live_gain = (after / before).sqrt();
            assert!(live_gain < 0.8);
            p.reset();
            p.parametric_set_parameter(
                ParameterId::from("use_captured_profile"),
                ParameterValue::Bool(true),
            )
            .unwrap();
            let restored = render(&mut p, &source, rate, 257);
            assert_eq!(out.len(), restored.len());
            assert!(out.iter().zip(&restored).all(|(a, b)| a == b));
            println!(
                "profile_selection rate={rate} low_latency={low_latency} live_gain={live_gain} restored_waveform_error=0"
            );
            p.initialize(f64::from(rate)).unwrap();
            assert!(
                p.noise_profile.has_noise_profile
                    && p.multi_res
                        .multi_res_state
                        .as_ref()
                        .unwrap()
                        .has_captured_profile
            );
            p.initialize(f64::from(rate) + 1.0).unwrap();
            assert!(
                !p.noise_profile.has_noise_profile
                    && !p
                        .multi_res
                        .multi_res_state
                        .as_ref()
                        .unwrap()
                        .has_captured_profile
            );
            assert_eq!(
                p.parametric_get_parameter(&ParameterId::from("use_captured_profile")),
                Some(ParameterValue::Bool(false))
            );
        }
    }
}

#[test]
fn small_profile_matches_independent_periodograms_and_commits_new_gains_together() {
    use super::multi_resolution::{MultiResState, SmallProfileControl};
    const N: usize = 512;
    let mut seed = 0x2468ace1;
    let source: Vec<f32> = (0..2048)
        .flat_map(|frame| {
            let a = if frame < 1536 { 0.01 } else { 0.2 };
            [
                a * (0.2 + noise(&mut seed)),
                a * 0.5 * (0.3 + noise(&mut seed)),
            ]
        })
        .collect();
    let power = |origin: usize, channel: usize, bin: usize| {
        let mut re = 0.0;
        let mut im = 0.0;
        for n in 0..N {
            // Independent f64 periodic sqrt-Hann and unnormalized DFT.
            let w = (std::f64::consts::PI * n as f64 / N as f64).sin();
            let phase = std::f64::consts::TAU * bin as f64 * n as f64 / N as f64;
            let x = f64::from(source[(origin + n) * 2 + channel]) * w;
            re += x * phase.cos();
            im -= x * phase.sin();
        }
        re * re + im * im
    };
    let mut state = MultiResState::new(2, 0.9, 0.7, 50, 5.0);
    state.start_capture();
    state
        .feed_and_process(
            &source,
            2,
            100.0,
            0.01,
            None,
            SmallProfileControl {
                capture: true,
                use_captured: false,
            },
        )
        .unwrap();
    assert_eq!(state.capture_frames, 7);
    assert!(!state.has_captured_profile);
    state.finalize_profile(100.0, 0.01, None);
    assert!(state.has_captured_profile);
    assert_eq!(state.capture_frames, 0);
    let mut maximum_power_error = 0.0_f64;
    let mut maximum_gain_error = 0.0_f64;
    for channel in 0..2 {
        for bin in [0, 1, 5, 37, 128, 256] {
            let expected = (0..7)
                .map(|frame| power(frame * 256, channel, bin))
                .sum::<f64>()
                / 7.0;
            let actual = f64::from(state.channels[channel].captured_psd[bin]);
            let error = (actual - expected).abs() / expected.max(1.0);
            maximum_power_error = maximum_power_error.max(error);
            assert!(error < 2e-6, "channel={channel} bin={bin} error={error}");
            let ratio = (power(1536, channel, bin) - expected).max(0.0) / expected.max(1e-10);
            let expected_gain = (ratio / (ratio + 100.0)).max(0.01);
            let error =
                (f64::from(state.channels[channel].smoothed_gain[bin]) - expected_gain).abs();
            maximum_gain_error = maximum_gain_error.max(error);
            assert!(
                error < 1e-5,
                "new epoch gain channel={channel} bin={bin} error={error}"
            );
        }
    }
    println!(
        "small_profile_oracle bins=12 maximum_scaled_power_error={maximum_power_error} maximum_gain_error={maximum_gain_error}"
    );
}

#[test]
fn profile_capture_is_partition_invariant_and_padding_does_not_measure_noise() {
    for low in [false, true] {
        let rate = 48_000;
        let mut seed = 0x13579bdf;
        let source: Vec<f32> = (0..rate as usize * 5 / 4)
            .flat_map(|_| [noise(&mut seed) * 0.03, noise(&mut seed) * 0.1])
            .collect();
        let mut reference = None;
        let mut profiles = None;
        let mut small_profiles = None;
        for block in [1, 257, 4096] {
            let mut p = processor(low, true, rate);
            p.parametric_set_parameter(
                ParameterId::from("learn_noise"),
                ParameterValue::Bool(true),
            )
            .unwrap();
            let out = render(&mut p, &source, rate, block);
            assert!(p.noise_profile.has_noise_profile);
            assert_eq!(
                p.parametric_get_parameter(&ParameterId::from("use_captured_profile")),
                Some(ParameterValue::Bool(true))
            );
            assert_eq!(
                p.parametric_get_parameter(&ParameterId::from("learn_noise")),
                Some(ParameterValue::Bool(false))
            );
            let small = p.multi_res.multi_res_state.as_ref().unwrap();
            assert!(small.has_captured_profile);
            let small: Vec<Vec<f32>> = small
                .channels
                .iter()
                .map(|ch| ch.captured_psd.clone())
                .collect();
            if let Some(expected) = &reference {
                let expected: &Vec<f32> = expected;
                assert_eq!(out.len(), expected.len());
                assert!(out.iter().zip(expected).all(|(a, b)| a == b));
                assert_eq!(
                    &p.noise_profile.noise_profile_storage,
                    profiles.as_ref().unwrap()
                );
                assert_eq!(&small, small_profiles.as_ref().unwrap());
            } else {
                reference = Some(out);
                profiles = Some(p.noise_profile.noise_profile_storage.clone());
                small_profiles = Some(small);
            }
            p.reset();
            assert!(
                p.multi_res
                    .multi_res_state
                    .as_ref()
                    .unwrap()
                    .has_captured_profile
            );
            p.parametric_set_parameter(
                ParameterId::from("learn_noise"),
                ParameterValue::Bool(true),
            )
            .unwrap();
            let frames = p.config.fft_size + 137;
            let mut partial = vec![0.01; frames * 2];
            p.process_in_place(&mut partial, &ProcessContext::new(rate, frames))
                .unwrap();
            let count = p.noise_profile.learning_frames_count;
            let small_count = p.multi_res.multi_res_state.as_ref().unwrap().capture_frames;
            assert!(count > 0 && small_count > 0);
            let mut tail = vec![0.0; p.drain_output_frames_max() * 2];
            for _ in 0..32 {
                let step = p.drain(&mut tail, &ProcessContext::new(rate, 0)).unwrap();
                assert_eq!(p.noise_profile.learning_frames_count, count);
                assert_eq!(
                    p.multi_res.multi_res_state.as_ref().unwrap().capture_frames,
                    small_count
                );
                if step.complete {
                    break;
                }
            }
            // Controls are frozen once drain begins. Rejection retains the
            // pending epoch, then reset cancels it without losing either profile.
            assert!(
                p.parametric_set_parameter(
                    ParameterId::from("learn_noise"),
                    ParameterValue::Bool(false)
                )
                .is_err()
            );
            assert_eq!(
                p.multi_res.multi_res_state.as_ref().unwrap().capture_frames,
                small_count
            );
            p.reset();
            p.parametric_set_parameter(
                ParameterId::from("learn_noise"),
                ParameterValue::Bool(true),
            )
            .unwrap();
            p.process_in_place(&mut partial, &ProcessContext::new(rate, frames))
                .unwrap();
            p.parametric_set_parameter(
                ParameterId::from("learn_noise"),
                ParameterValue::Bool(false),
            )
            .unwrap();
            assert_eq!(
                p.multi_res.multi_res_state.as_ref().unwrap().capture_frames,
                0
            );
            assert_eq!(
                &p.noise_profile.noise_profile_storage,
                profiles.as_ref().unwrap()
            );
            assert_eq!(
                p.multi_res
                    .multi_res_state
                    .as_ref()
                    .unwrap()
                    .channels
                    .iter()
                    .map(|ch| ch.captured_psd.clone())
                    .collect::<Vec<_>>(),
                *small_profiles.as_ref().unwrap()
            );
            p.parametric_set_parameter(
                ParameterId::from("clear_profile"),
                ParameterValue::Bool(true),
            )
            .unwrap();
            assert!(!p.noise_profile.has_noise_profile);
            assert!(
                !p.multi_res
                    .multi_res_state
                    .as_ref()
                    .unwrap()
                    .has_captured_profile
            );
        }
    }
    println!(
        "profile_capture_partition cases=6 exact_audio_and_both_grids=true eof_capture_frozen=true cancel_retains_profile=true clear_invalidates_both=true"
    );
}
