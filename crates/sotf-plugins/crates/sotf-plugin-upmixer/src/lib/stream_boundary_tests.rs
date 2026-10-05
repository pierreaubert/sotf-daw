//! Independent neutral reconstruction oracle for stream boundary scheduling.
// Rust guideline compliant 2026-02-21

use crate::{UpmixerPlugin, UpmixerPluginParams};
use rustfft::num_complex::Complex;
use sotf_host::{Plugin, ProcessContext};

#[test]
fn declared_drain_work_includes_cached_output_and_long_release_cap() {
    for subharmonic in [false, true] {
        for source in [1, 31, 32, 33, 65] {
            for partial in [false, true] {
                let mut plugin = neutral(64);
                assert_eq!(plugin.drain_call_bound().unwrap().get(), 1);
                if subharmonic {
                    plugin
                        .set_parameter(
                            "enable_subharmonic_synth".into(),
                            sotf_host::ParameterValue::Bool(true),
                        )
                        .unwrap();
                    plugin
                        .set_parameter(
                            "subharmonic_release_ms".into(),
                            sotf_host::ParameterValue::Float(500.0),
                        )
                        .unwrap();
                }
                render(&mut plugin, &vec![0.125; source * 2], &[7]);
                if partial {
                    plugin
                        .drain(&mut [0.0; 2], &ProcessContext::new(48_000, 0))
                        .unwrap();
                }
                let bound = plugin.drain_call_bound().unwrap().get();
                if subharmonic {
                    assert!(bound > 4096);
                }
                let mut calls = 0;
                let mut output = vec![0.0; plugin.drain_output_frames_max() * 2];
                loop {
                    calls += 1;
                    assert!(calls <= bound);
                    let result = plugin
                        .drain(&mut output, &ProcessContext::new(48_000, 0))
                        .unwrap();
                    if result.complete {
                        break;
                    }
                }
                assert_eq!(calls, bound);
                assert_eq!(plugin.drain_call_bound().unwrap().get(), 1);
                plugin.reset();
                assert_eq!(plugin.drain_call_bound().unwrap().get(), 1);
            }
        }
    }
}

fn neutral(fft_size: usize) -> UpmixerPlugin {
    let params: UpmixerPluginParams = serde_json::from_value(serde_json::json!({
        "fft_size": fft_size,
        "speaker_config": "2.0",
        "gain_front_direct": 1.0,
        "gain_front_ambient": 0.0,
        "gain_rear_ambient": 0.0,
        "stereo_width": 0.0,
        "enable_hr_direct": false,
        "enable_subharmonic_synth": false,
        "bypass_decorrelation": true,
        "auto_gain_enabled": false,
        "safety_cap_db": -1.0,
        "multi_source_extraction": false
    }))
    .unwrap();
    let mut plugin = UpmixerPlugin::from_params(params);
    plugin.initialize(48_000.0).unwrap();
    neutralize_tables(&mut plugin);
    plugin
}

fn neutralize_tables(plugin: &mut UpmixerPlugin) {
    assert_eq!(plugin.output_channels(), 2);
    // Identity routing and crossover isolate the real process scheduler,
    // analysis FFT, inverse FFT, synthesis window, and overlap accumulator.
    // No production bypass skips the transforms under test.
    plugin
        .panning
        .panning_gains_left
        .copy_from_slice(&[1.0, 0.0]);
    plugin
        .panning
        .panning_gains_right
        .copy_from_slice(&[0.0, 1.0]);
    plugin
        .spectral
        .mains_high_gains
        .fill(Complex::new(1.0, 0.0));
    plugin.spectral.lfe_low_gains.fill(Complex::new(0.0, 0.0));
}

fn render(plugin: &mut UpmixerPlugin, input: &[f32], partitions: &[usize]) -> Vec<f32> {
    let channels = plugin.output_channels();
    let mut output = Vec::with_capacity(input.len());
    let mut position = 0;
    for &requested in partitions.iter().cycle() {
        if position == input.len() / 2 {
            break;
        }
        let frames = requested.min(input.len() / 2 - position);
        let mut block = vec![f32::NAN; frames * channels];
        assert_eq!(
            plugin
                .process(
                    &input[position * 2..(position + frames) * 2],
                    &mut block,
                    &ProcessContext::new(48_000, frames),
                )
                .unwrap(),
            frames
        );
        output.extend(block);
        position += frames;
    }
    output
}

#[test]
fn neutral_reconstruction_preserves_every_initial_window_phase() {
    let mut maximum_error = 0.0_f32;
    for fft_size in [2, 4, 8, 16, 32, 64, 512] {
        for partitions in [&[1][..], &[17, 3, 97][..], &[512][..]] {
            let mut plugin = neutral(fft_size);
            let mut worst = 0.0_f32;
            let mut first = 0.0;
            let mut quarter = 0.0;
            for phase in 0..fft_size {
                plugin.reset();
                let mut input = vec![0.0; fft_size * 4 * 2];
                input[phase * 2] = 0.25;
                input[phase * 2 + 1] = -0.125;
                let mut output = render(&mut plugin, &input, partitions);
                output.extend(finish_with_capacity(&mut plugin, 17));
                let expected_index = (plugin.latency_samples() + phase) * 2;
                if phase == 0 {
                    first = output[expected_index] / 0.25;
                }
                if phase == fft_size / 4 {
                    quarter = output[expected_index] / 0.25;
                }
                for (index, &actual) in output.iter().enumerate() {
                    let expected = if index == expected_index {
                        0.25
                    } else if index == expected_index + 1 {
                        -0.125
                    } else {
                        0.0
                    };
                    worst = worst.max((actual - expected).abs());
                }
            }
            eprintln!(
                "N={fft_size} callbacks={partitions:?}: first gain={first}, quarter gain={quarter}, max absolute error={worst}"
            );
            maximum_error = maximum_error.max(worst);
        }
    }
    assert!(
        maximum_error < 2.0e-6,
        "initial-window identity reconstruction error {maximum_error}"
    );
}

#[test]
fn fft_two_reconstructs_dc_nyquist_dense_signal_and_finite_eos() {
    const FFT_SIZE: usize = 2;
    const INPUT_FRAMES: usize = 29;

    let input: Vec<f32> = (0..INPUT_FRAMES)
        .flat_map(|frame| {
            let nyquist = if frame.is_multiple_of(2) { 1.0 } else { -1.0 };
            let phase = std::f32::consts::TAU * frame as f32 / 7.0;
            [
                0.13 + 0.17 * nyquist + 0.03 * phase.sin(),
                -0.09 - 0.08 * nyquist + 0.02 * phase.cos(),
            ]
        })
        .collect();

    // For N=2, hop=1. The finite source is followed by the exact N+(N-hop)
    // zero-continuation frames needed to complete the last overlapping window.
    let latency = FFT_SIZE.max(512);
    let mut expected = vec![0.0; (INPUT_FRAMES + latency + FFT_SIZE / 2) * 2];
    let delayed = latency * 2;
    expected[delayed..delayed + input.len()].copy_from_slice(&input);

    for partitions in [&[1][..], &[2, 1, 5][..], &[13][..]] {
        let mut plugin = neutral(FFT_SIZE);
        let mut actual = render(&mut plugin, &input, partitions);
        actual.extend(finish_with_capacity(&mut plugin, 5));
        assert_eq!(actual.len(), expected.len(), "callbacks={partitions:?}");
        let maximum_error = actual
            .iter()
            .zip(&expected)
            .map(|(actual, expected)| (actual - expected).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            maximum_error < 2.0e-6,
            "N=2 DC/Nyquist/dense/EOS error {maximum_error}, callbacks={partitions:?}"
        );
    }
}

#[test]
fn small_main_fft_hr_impulse_probe_records_the_sub_512_timeline() {
    for fft_size in [2, 32] {
        let mut hr = neutral(fft_size);
        hr.params.enable_hr_direct = true;
        hr.hr_state.hr_direct_envelope = 1.0;
        assert!(hr.hr_buffers.hr_delay_buffer.is_empty());

        // The streaming HR buffer is prefixed with half an HR window of zeros.
        // Put an impulse at the first real sample, then run the same HR block
        // and startup discard used by the stream path.
        let hr_frames = hr.fft.hr_fft_size;
        let first_real_frame = hr_frames / 2;
        let mut hr_input = vec![0.0; hr_frames * 2];
        hr_input[first_real_frame * 2] = 0.25;
        hr_input[first_real_frame * 2 + 1] = -0.125;
        hr.process_hr_block(&hr_input);
        let mut next_hr_input = vec![0.0; hr_frames * 2];
        next_hr_input[..hr_frames].copy_from_slice(&hr_input[hr_frames..]);
        hr.process_hr_block(&next_hr_input);
        let retained_frames = hr.hr_buffers.hr_output_accumulator_fill;
        assert_eq!(retained_frames, hr_frames / 2);
        hr.output.hr_mix_gains.fill(1.0);
        let mut hr_only = vec![0.0; retained_frames * hr.output_channels()];
        hr.mix_hr_output_source_aligned(
            &mut hr_only,
            0,
            0,
            retained_frames,
            hr.output_channels(),
            false,
        );

        let peak_frame = hr_only
            .chunks_exact(hr.output_channels())
            .enumerate()
            .max_by(|(_, left), (_, right)| {
                let left = left.iter().map(|sample| sample.abs()).fold(0.0, f32::max);
                let right = right.iter().map(|sample| sample.abs()).fold(0.0, f32::max);
                left.total_cmp(&right)
            })
            .map(|(frame, _)| frame)
            .unwrap();
        let peak_amplitude = hr_only
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0, f32::max);

        // Independent identity main-path oracle: a source impulse is placed
        // exactly at the API-reported latency, while the prefixed HR impulse
        // emerges at timeline frame zero. These are separate path timing
        // facts; this probe does not claim full HR/main quality parity.
        let mut main = neutral(fft_size);
        let mut impulse = vec![0.0; (fft_size * 4) * 2];
        impulse[0] = 0.25;
        impulse[1] = -0.125;
        let mut main_output = render(&mut main, &impulse, &[17, 3, 97]);
        main_output.extend(finish_with_capacity(&mut main, 17));
        let main_peak_frame = main_output
            .chunks_exact(main.output_channels())
            .enumerate()
            .max_by(|(_, left), (_, right)| {
                let left = left.iter().map(|sample| sample.abs()).fold(0.0, f32::max);
                let right = right.iter().map(|sample| sample.abs()).fold(0.0, f32::max);
                left.total_cmp(&right)
            })
            .map(|(frame, _)| frame)
            .unwrap();

        eprintln!(
            "N={fft_size}: HR retained impulse peak frame={peak_frame}, main peak frame={main_peak_frame}, API latency={}",
            main.latency_samples()
        );
        assert_eq!(peak_frame, 0);
        assert!(
            peak_amplitude > 1.0e-6,
            "silent HR impulse probe at N={fft_size}"
        );
        assert_eq!(main_peak_frame, main.latency_samples());
    }
}

#[test]
fn hr_stream_arrival_offsets_are_recorded_across_fft_sizes() {
    for fft_size in [2, 32, 64, 128, 256, 512, 1024, 2048] {
        let input_frames = (fft_size * 2).max(4096);
        let mut input: Vec<f32> = (0..input_frames)
            .flat_map(|frame| {
                [
                    0.003 * (frame as f32 * 0.19).sin(),
                    0.002 * (frame as f32 * 0.11).cos(),
                ]
            })
            .collect();
        input[0] = 0.25;
        input[1] = -0.125;
        input[300 * 2] = 0.18;
        input[300 * 2 + 1] = -0.09;

        for partitions in [&[1, 17, 137, 256][..], &[512][..]] {
            let mut hr = neutral(fft_size);
            let mut reference = neutral(fft_size);
            for plugin in [&mut hr, &mut reference] {
                plugin.params.enable_hr_direct = true;
                plugin.hr_state.hr_direct_envelope = 1.0;
                // Make the leading broadband impulse produce a nonzero HR gain
                // schedule without altering the neutral main-route tables.
                plugin.hr_state.spectral_flux_smooth = 1.0e-6;
            }
            // Keep every main-path state and scheduling decision, but omit the
            // HR channel writes from the reference stream.
            reference.panning.cached_hr_active_channels.clear();

            let mut with_hr = render(&mut hr, &input, partitions);
            with_hr.extend(finish_with_capacity(&mut hr, fft_size / 2));
            let mut without_hr = render(&mut reference, &input, partitions);
            without_hr.extend(finish_with_capacity(&mut reference, fft_size / 2));
            assert_eq!(with_hr.len(), without_hr.len());

            let channels = hr.output_channels();
            let contribution: Vec<f32> = with_hr
                .iter()
                .zip(&without_hr)
                .map(|(with, without)| with - without)
                .collect();
            let contribution_by_frame = contribution
                .chunks_exact(channels)
                .map(|frame| frame.iter().map(|sample| sample.abs()).fold(0.0, f32::max))
                .collect::<Vec<_>>();
            let peak_amplitude = contribution_by_frame.iter().copied().fold(0.0, f32::max);
            assert!(
                peak_amplitude > 1.0e-8,
                "HR contribution is silent at N={fft_size}, callbacks={partitions:?}, transient={}, channels={:?}",
                hr.hr_state.hr_transient_env,
                hr.panning.cached_hr_active_channels
            );
            let peak_frame = contribution_by_frame
                .iter()
                .position(|amplitude| *amplitude == peak_amplitude)
                .unwrap();
            let first_nonzero_frame = contribution_by_frame
                .iter()
                .position(|amplitude| *amplitude > 1.0e-8)
                .unwrap();
            let main_peak_frame = without_hr
                .chunks_exact(channels)
                .enumerate()
                .max_by(|(_, left), (_, right)| {
                    let left = left.iter().map(|sample| sample.abs()).fold(0.0, f32::max);
                    let right = right.iter().map(|sample| sample.abs()).fold(0.0, f32::max);
                    left.total_cmp(&right)
                })
                .map(|(frame, _)| frame)
                .unwrap();
            if matches!(fft_size, 2 | 256 | 512 | 1024 | 2048) {
                let local_start = first_nonzero_frame.saturating_sub(4);
                let local_end = (first_nonzero_frame + 8).min(contribution_by_frame.len());
                eprintln!(
                    "stream N={fft_size} callbacks={partitions:?}: HR contribution near first={local_start}..{local_end} {:?}",
                    &contribution_by_frame[local_start..local_end]
                );
            }

            eprintln!(
                "stream N={fft_size} callbacks={partitions:?}: HR contribution first={first_nonzero_frame}, peak={peak_frame} ({peak_amplitude}), neutral main peak={main_peak_frame}, API latency={}, accepted={}, emitted={}",
                hr.latency_samples(),
                input_frames,
                with_hr.len() / channels
            );
            assert_eq!(main_peak_frame, hr.latency_samples());
            if matches!(fft_size, 2 | 32) {
                // Preserve the already measured AUD130 cases. Other sizes are
                // logged as current behavior; their timing is not assumed.
                assert_eq!(first_nonzero_frame, hr.fft.hr_fft_size);
                assert_eq!(peak_frame, hr.fft.hr_fft_size);
                assert_eq!(
                    first_nonzero_frame - main_peak_frame,
                    hr.fft.hr_fft_size - hr.latency_samples(),
                    "sub-512 HR arrival offset at N={fft_size}"
                );
            }
        }
    }
}

#[test]
fn current_hr_stream_phase_is_projected_independently() {
    const HR_FFT_SIZE: usize = 512;
    const SUB_1024_TONE_BIN: f64 = 57.75;
    const TONE_BIN_1024: f64 = 57.5;
    const TONE_BIN_2048: f64 = 57.125;
    const INPUT_FRAMES: usize = 8192;
    const WINDOW_START: usize = 4096;

    let mut residual_failures = Vec::new();
    for fft_size in [2, 32, 64, 128, 256, 512, 1024, 2048] {
        let tone_bin = match fft_size {
            1024 => TONE_BIN_1024,
            2048 => TONE_BIN_2048,
            _ => SUB_1024_TONE_BIN,
        };
        let window_frames = if fft_size == 2048 { 4096 } else { 2048 };
        let omega = std::f64::consts::TAU * tone_bin / HR_FFT_SIZE as f64;
        let input: Vec<f32> = (0..INPUT_FRAMES)
            .flat_map(|frame| {
                // Keep the combined main + HR path below the final safety cap;
                // otherwise subtracting a separately rendered main-only path
                // would measure limiter nonlinearity as HR phase residual.
                let sample = (0.1 * (omega * frame as f64).sin()) as f32;
                [sample, 0.0]
            })
            .collect();
        let mut with_hr = neutral(fft_size);
        let mut without_hr = neutral(fft_size);
        with_hr.params.enable_hr_direct = true;
        with_hr.hr_state.hr_direct_envelope = 1.0;
        with_hr.hr_state.hr_transient_env = 1.0;
        with_hr.hr_state.spectral_flux_smooth = 1.0e-6;
        without_hr.params.enable_hr_direct = true;
        without_hr.hr_state.hr_direct_envelope = 1.0;
        without_hr.hr_state.hr_transient_env = 1.0;
        without_hr.hr_state.spectral_flux_smooth = 1.0e-6;
        // Pin the mix gain independently of detector state. Re-seeding the
        // transient detector per callback does not make its hop-scheduled gain
        // constant, so use the test gain clock to isolate only phase/residual.
        let fixed_hr_gain = (fft_size as f32 / HR_FFT_SIZE as f32).sqrt()
            * std::f32::consts::SQRT_2
            / HR_FFT_SIZE as f32;
        let gain_hops = (INPUT_FRAMES + fft_size * 4) / with_hr.core.hop_size + 16;
        with_hr.output.hr_test_gain_sequence = vec![fixed_hr_gain; gain_hops];
        without_hr.output.hr_test_gain_sequence = vec![fixed_hr_gain; gain_hops];
        // Preserve the same main analysis and output path while removing only
        // HR channel writes from the phase reference.
        without_hr.panning.cached_hr_active_channels.clear();

        let rendered_hr = render_with_reseeded_hr_envelope(&mut with_hr, &input);
        let rendered_hr_reference = render_with_reseeded_hr_envelope(&mut without_hr, &input);
        let mut neutral_main = neutral(fft_size);
        let rendered_main = render(&mut neutral_main, &input, &[1]);
        assert_eq!(rendered_hr.len(), rendered_hr_reference.len());
        assert_eq!(rendered_hr.len(), rendered_main.len());
        let full_peak = rendered_hr
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0, f32::max);
        assert!(
            full_peak < 0.5,
            "phase probe entered the final safety cap at N={fft_size}: peak={full_peak}"
        );

        let hr_only: Vec<f32> = rendered_hr
            .as_chunks::<2>()
            .0
            .iter()
            .zip(rendered_hr_reference.as_chunks::<2>().0.iter())
            .map(|(with, without)| with[0] - without[0])
            .collect();
        let main_left: Vec<f32> = rendered_main
            .as_chunks::<2>()
            .0
            .iter()
            .map(|frame| frame[0])
            .collect();
        let (hr_projection, hr_residual) =
            project_tone(&hr_only, WINDOW_START, window_frames, omega);
        let (main_projection, main_residual) =
            project_tone(&main_left, WINDOW_START, window_frames, omega);
        let hr_amplitude = 2.0 * hr_projection.norm() / window_frames as f64;
        let main_amplitude = 2.0 * main_projection.norm() / window_frames as f64;
        let relative = hr_projection * main_projection.conj();
        let relative_phase = relative.arg();
        let phase_eligible = hr_residual <= 0.01 && main_residual <= 0.01;
        if fft_size >= 1024 {
            let mask = with_hr.output.output_accumulator_mask;
            let (gain_min, gain_max) = (WINDOW_START..WINDOW_START + window_frames)
                .map(|frame| with_hr.output.hr_mix_gains[frame & mask])
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), gain| {
                    (min.min(gain), max.max(gain))
                });
            eprintln!(
                "phase diagnostic N={fft_size} transient_env={} hr_direct_envelope={} sharpen={} previous_hr_scale={} scheduled_gain_min={gain_min} scheduled_gain_max={gain_max}",
                with_hr.hr_state.hr_transient_env,
                with_hr.hr_state.hr_direct_envelope,
                with_hr.gains.hr_sharpen.current(),
                with_hr.hr_state.prev_hr_scale
            );
        }
        eprintln!(
            "phase N={fft_size} sample_rate=48000 tone_hz={:.4} callbacks=[1] window={WINDOW_START}..{} HR_amp={hr_amplitude:.9} main_amp={main_amplitude:.9} HR_residual={hr_residual:.6} main_residual={main_residual:.6} phase_eligible={phase_eligible} HR_minus_main_phase={relative_phase:.9} API_latency={}",
            tone_bin * 48_000.0 / HR_FFT_SIZE as f64,
            WINDOW_START + window_frames,
            with_hr.latency_samples()
        );
        if fft_size == 2 {
            let expected_phase = -omega * with_hr.latency_samples() as f64;
            let mut maximum_identity_error = 0.0_f64;
            for (frame, sample) in main_left
                .iter()
                .enumerate()
                .skip(WINDOW_START)
                .take(window_frames)
            {
                let source_frame = frame - with_hr.latency_samples();
                let expected = (0.1 * (omega * source_frame as f64).sin()) as f32;
                maximum_identity_error =
                    maximum_identity_error.max(f64::from((*sample - expected).abs()));
            }
            assert!(maximum_identity_error <= 1.0e-6);
            eprintln!(
                "phase diagnostic N=2 expected_main_phase={expected_phase:.9} max_identity_error={maximum_identity_error:.9} samples={:?}",
                &main_left[WINDOW_START..WINDOW_START + 8]
            );
        }
        assert!(
            hr_amplitude > 1.0e-6,
            "HR phase projection is silent at N={fft_size}: amplitude={hr_amplitude}"
        );
        assert!(
            main_amplitude > 1.0e-3,
            "main phase projection is silent at N={fft_size}: amplitude={main_amplitude}"
        );
        if !phase_eligible {
            residual_failures.push(format!(
                "N={fft_size}: HR residual={hr_residual:.6}, main residual={main_residual:.6}"
            ));
        }
    }
    assert!(
        residual_failures.is_empty(),
        "phase characterization exceeded the 1% residual ceiling: {residual_failures:?}"
    );
}

#[test]
fn aud132_mono_left_tagged_tone_matches_the_pre_correction_route_shift() {
    const HR_FFT_SIZE: usize = 512;
    const INPUT_FRAMES: usize = 16_384;
    const WINDOW_START: usize = 4_096;
    let mut differences = Vec::new();

    for (fft_size, tone_bin, window_frames) in
        [(1_024_usize, 57.5_f64, 2_048_usize), (2_048, 57.125, 4_096)]
    {
        let delay_frames = (fft_size - HR_FFT_SIZE) / 2;
        let omega = std::f64::consts::TAU * tone_bin / HR_FFT_SIZE as f64;
        let input: Vec<f32> = (0..INPUT_FRAMES)
            .flat_map(|frame| [(0.1 * (omega * frame as f64).sin()) as f32, 0.0])
            .collect();
        let make_plugin = |include_hr: bool, legacy_route: bool| {
            let mut plugin = neutral(fft_size);
            plugin.params.enable_hr_direct = true;
            plugin.hr_state.hr_direct_envelope = 1.0;
            plugin.hr_state.hr_transient_env = 1.0;
            let gain = (fft_size as f32 / HR_FFT_SIZE as f32).sqrt() * std::f32::consts::SQRT_2
                / HR_FFT_SIZE as f32;
            plugin.output.hr_test_gain_sequence = vec![gain; 256];
            if legacy_route {
                plugin.output.hr_use_pre_correction_mixer_for_test = true;
                plugin.hr_buffers.hr_startup_discard_remaining = HR_FFT_SIZE / 2;
            }
            if !include_hr {
                plugin.panning.cached_hr_active_channels.clear();
            }
            plugin
        };

        let raw_frames = fft_size;
        let raw_input = input[..raw_frames * 2].to_vec();
        let mut raw_tagged = make_plugin(true, false);
        let mut raw_legacy = make_plugin(true, true);
        let mut raw_output = vec![f32::NAN; raw_frames * 2];
        assert_eq!(
            raw_tagged
                .process(
                    &raw_input,
                    &mut raw_output,
                    &ProcessContext::new(48_000, raw_frames),
                )
                .unwrap(),
            raw_frames
        );
        assert_eq!(
            raw_legacy
                .process(
                    &raw_input,
                    &mut raw_output,
                    &ProcessContext::new(48_000, raw_frames),
                )
                .unwrap(),
            raw_frames
        );
        let tagged_mask = raw_tagged.hr_buffers.hr_output_accumulator_mask;
        let legacy_mask = raw_legacy.hr_buffers.hr_output_accumulator_mask;
        let tagged_read = raw_tagged.hr_buffers.hr_output_read_position;
        let legacy_read = raw_legacy.hr_buffers.hr_output_read_position;
        let tag_zero_offset = (0..raw_tagged.hr_buffers.hr_output_accumulator_fill)
            .find(|offset| {
                let position = (tagged_read + offset) & tagged_mask;
                raw_tagged.hr_buffers.hr_output_source_tags[position] == 0
            })
            .expect("tag zero should be ready in the raw HR comparison");
        let raw_compare_frames = raw_tagged.hr_buffers.hr_output_accumulator_fill - tag_zero_offset;
        assert!(
            raw_legacy.hr_buffers.hr_output_accumulator_fill >= delay_frames + raw_compare_frames
        );
        let mut maximum_raw_delta = 0.0_f32;
        for offset in 0..raw_compare_frames {
            let tagged_position = (tagged_read + tag_zero_offset + offset) & tagged_mask;
            let legacy_position = (legacy_read + delay_frames + offset) & legacy_mask;
            assert_eq!(
                raw_tagged.hr_buffers.hr_output_source_tags[tagged_position],
                offset as u64
            );
            let tagged_base = tagged_position * raw_tagged.core.num_output_channels;
            let legacy_base = legacy_position * raw_legacy.core.num_output_channels;
            for channel in 0..raw_tagged.core.num_output_channels {
                maximum_raw_delta = maximum_raw_delta.max(
                    (raw_tagged.hr_buffers.hr_output_accumulator[tagged_base + channel]
                        - raw_legacy.hr_buffers.hr_output_accumulator[legacy_base + channel])
                        .abs(),
                );
            }
        }
        eprintln!(
            "AUD132 mono raw HR N={fft_size} D={delay_frames} tagged_fill={} legacy_fill={} compared={raw_compare_frames} max_raw_delta={maximum_raw_delta:.9e}",
            raw_tagged.hr_buffers.hr_output_accumulator_fill,
            raw_legacy.hr_buffers.hr_output_accumulator_fill,
        );

        let mut tagged = make_plugin(true, false);
        let mut tagged_main = make_plugin(false, false);
        let mut legacy = make_plugin(true, true);
        let mut legacy_main = make_plugin(false, true);
        let tagged_full = render_with_reseeded_hr_envelope(&mut tagged, &input);
        let tagged_main = render_with_reseeded_hr_envelope(&mut tagged_main, &input);
        let legacy_full = render_with_reseeded_hr_envelope(&mut legacy, &input);
        let legacy_main = render_with_reseeded_hr_envelope(&mut legacy_main, &input);
        let tagged_peak = tagged_full
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0, f32::max);
        let legacy_peak = legacy_full
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0, f32::max);
        assert!(tagged_peak < 0.5 && legacy_peak < 0.5);
        let tagged_hr: Vec<f32> = tagged_full
            .as_chunks::<2>()
            .0
            .iter()
            .zip(tagged_main.as_chunks::<2>().0.iter())
            .map(|(full, main)| full[0] - main[0])
            .collect();
        let legacy_hr: Vec<f32> = legacy_full
            .as_chunks::<2>()
            .0
            .iter()
            .zip(legacy_main.as_chunks::<2>().0.iter())
            .map(|(full, main)| full[0] - main[0])
            .collect();
        let shifted_legacy: Vec<f32> = (0..window_frames)
            .map(|offset| legacy_hr[WINDOW_START + offset + delay_frames])
            .collect();
        let tagged_window = &tagged_hr[WINDOW_START..WINDOW_START + window_frames];
        let maximum_shift_delta = tagged_window
            .iter()
            .zip(&shifted_legacy)
            .map(|(actual, expected)| (actual - expected).abs())
            .fold(0.0_f32, f32::max);
        let (tagged_projection, tagged_residual) =
            project_tone(&tagged_hr, WINDOW_START, window_frames, omega);
        let (shifted_projection, shifted_residual) =
            project_tone(&shifted_legacy, 0, window_frames, omega);
        let phase_delta = (tagged_projection * shifted_projection.conj()).arg();
        eprintln!(
            "AUD132 mono tone N={fft_size} D={delay_frames} tagged_residual={tagged_residual:.7} shifted_legacy_residual={shifted_residual:.7} phase_delta={phase_delta:.9} max_shift_delta={maximum_shift_delta:.9e}"
        );
        assert!(tagged_projection.norm() > 1.0e-6);
        assert!(shifted_projection.norm() > 1.0e-6);
        if maximum_shift_delta > 1.0e-4 {
            differences.push(format!(
                "N={fft_size}: shifted old-route max delta={maximum_shift_delta:.6e}"
            ));
        }
    }

    assert!(differences.is_empty(), "{differences:?}");
}

#[test]
fn sub_512_hr_phase_crosscheck_uses_a_second_tone() {
    const HR_FFT_SIZE: usize = 512;
    const TONE_BIN: f64 = 57.25;
    const INPUT_FRAMES: usize = 8192;
    const WINDOW_START: usize = 4096;
    const WINDOW_FRAMES: usize = 2048;
    let omega = std::f64::consts::TAU * TONE_BIN / HR_FFT_SIZE as f64;
    let input: Vec<f32> = (0..INPUT_FRAMES)
        .flat_map(|frame| {
            let sample = (0.5 * (omega * frame as f64).sin()) as f32;
            [sample, 0.0]
        })
        .collect();

    for fft_size in [2, 32, 64, 128, 256] {
        let mut with_hr = neutral(fft_size);
        let mut without_hr = neutral(fft_size);
        for plugin in [&mut with_hr, &mut without_hr] {
            plugin.params.enable_hr_direct = true;
            plugin.hr_state.hr_direct_envelope = 1.0;
            plugin.hr_state.hr_transient_env = 1.0;
            plugin.hr_state.spectral_flux_smooth = 1.0e-6;
        }
        without_hr.panning.cached_hr_active_channels.clear();
        let rendered_hr = render_with_reseeded_hr_envelope(&mut with_hr, &input);
        let rendered_hr_reference = render_with_reseeded_hr_envelope(&mut without_hr, &input);
        let mut neutral_main = neutral(fft_size);
        let rendered_main = render(&mut neutral_main, &input, &[1]);

        let hr_only: Vec<f32> = rendered_hr
            .as_chunks::<2>()
            .0
            .iter()
            .zip(rendered_hr_reference.as_chunks::<2>().0.iter())
            .map(|(with, without)| with[0] - without[0])
            .collect();
        let main_left: Vec<f32> = rendered_main
            .as_chunks::<2>()
            .0
            .iter()
            .map(|frame| frame[0])
            .collect();
        let (hr_projection, hr_residual) =
            project_tone(&hr_only, WINDOW_START, WINDOW_FRAMES, omega);
        let (main_projection, main_residual) =
            project_tone(&main_left, WINDOW_START, WINDOW_FRAMES, omega);
        let amplitude = 2.0 * hr_projection.norm() / WINDOW_FRAMES as f64;
        assert!(amplitude > 1.0e-6, "silent second-tone HR at N={fft_size}");
        assert!(
            hr_residual <= 0.01 && main_residual <= 0.01,
            "second-tone residual exceeds 1% at N={fft_size}: HR={hr_residual}, main={main_residual}"
        );

        let phase = (hr_projection * main_projection.conj()).arg();
        let hypothesized_delay = 511 - fft_size;
        let expected = -omega * hypothesized_delay as f64;
        let phase_error = (phase - expected + std::f64::consts::PI)
            .rem_euclid(std::f64::consts::TAU)
            - std::f64::consts::PI;
        eprintln!(
            "second-tone phase N={fft_size} tone_hz={:.4} window={WINDOW_START}..{} HR_amp={amplitude:.9} HR_residual={hr_residual:.6} main_residual={main_residual:.6} HR_minus_main_phase={phase:.9} candidate_511_frame_phase={expected:.9} error={phase_error:.9}",
            TONE_BIN * 48_000.0 / HR_FFT_SIZE as f64,
            WINDOW_START + WINDOW_FRAMES
        );
    }
}

#[test]
fn fixed_gain_hr_impulse_arrival_is_measured_on_the_phase_path() {
    for fft_size in [2, 32, 64, 128, 256, 512, 1024, 2048] {
        let input_frames = (fft_size * 4).max(4096);
        let mut input = vec![0.0; input_frames * 2];
        input[0] = 0.25;
        input[1] = -0.125;

        let mut with_hr = neutral(fft_size);
        let mut without_hr = neutral(fft_size);
        for plugin in [&mut with_hr, &mut without_hr] {
            plugin.params.enable_hr_direct = true;
            plugin.hr_state.hr_direct_envelope = 1.0;
            plugin.hr_state.hr_transient_env = 1.0;
            plugin.hr_state.spectral_flux_smooth = 1.0e-6;
        }
        without_hr.panning.cached_hr_active_channels.clear();

        let mut rendered_hr = render_with_reseeded_hr_envelope(&mut with_hr, &input);
        let mut rendered_reference = render_with_reseeded_hr_envelope(&mut without_hr, &input);
        rendered_hr.extend(finish_with_capacity(&mut with_hr, fft_size / 2));
        rendered_reference.extend(finish_with_capacity(&mut without_hr, fft_size / 2));
        assert_eq!(rendered_hr.len(), rendered_reference.len());

        let contribution = rendered_hr
            .as_chunks::<2>()
            .0
            .iter()
            .zip(rendered_reference.as_chunks::<2>().0.iter())
            .map(|(with, without)| {
                (with[0] - without[0])
                    .abs()
                    .max((with[1] - without[1]).abs())
            })
            .collect::<Vec<_>>();
        let peak_amplitude = contribution.iter().copied().fold(0.0_f32, f32::max);
        assert!(
            peak_amplitude > 1.0e-8,
            "silent fixed-gain HR impulse at N={fft_size}"
        );
        let first_nonzero = contribution
            .iter()
            .position(|amplitude| *amplitude > 1.0e-8)
            .unwrap();
        let peak_frame = contribution
            .iter()
            .position(|amplitude| *amplitude == peak_amplitude)
            .unwrap();

        let mut main = neutral(fft_size);
        let rendered_main = render(&mut main, &input, &[1]);
        let main_peak_frame = rendered_main
            .chunks_exact(main.output_channels())
            .enumerate()
            .max_by(|(_, left), (_, right)| {
                let left = left.iter().map(|sample| sample.abs()).fold(0.0, f32::max);
                let right = right.iter().map(|sample| sample.abs()).fold(0.0, f32::max);
                left.total_cmp(&right)
            })
            .map(|(frame, _)| frame)
            .unwrap();
        eprintln!(
            "fixed-gain HR impulse N={fft_size}: callbacks=[1], first={first_nonzero}, peak={peak_frame} ({peak_amplitude:.9}), main_peak={main_peak_frame}, API_latency={}, accepted={input_frames}, emitted={}",
            main.latency_samples(),
            rendered_hr.len() / 2
        );
        assert_eq!(main_peak_frame, main.latency_samples());
        assert!(rendered_hr.iter().all(|sample| sample.is_finite()));
    }
}

#[test]
fn prepared_hr_gain_impulse_arrival_is_recorded_across_callback_partitions() {
    for fft_size in [2, 32, 64, 128, 256] {
        let input_frames = (fft_size * 4).max(4096);
        let mut input = vec![0.0; input_frames * 2];
        input[0] = 0.25;
        input[1] = -0.125;
        let mut baseline: Option<Vec<f32>> = None;

        for partitions in [&[1][..], &[17, 137, 256][..], &[512][..]] {
            let make = || {
                let mut plugin = neutral(fft_size);
                let gain = (fft_size as f32 / 512.0).sqrt()
                    * plugin.gains.hr_sharpen.current()
                    * std::f32::consts::SQRT_2
                    / 512.0;
                let prepared_hops = (input_frames + fft_size * 4 + 4096) / plugin.core.hop_size + 4;
                plugin.params.enable_hr_direct = true;
                plugin.hr_state.hr_direct_envelope = 1.0;
                plugin.hr_state.prev_hr_scale = gain;
                plugin.output.hr_test_gain_sequence = vec![gain; prepared_hops];
                plugin
            };
            let mut with_hr = make();
            let mut without_hr = make();
            without_hr.panning.cached_hr_active_channels.clear();

            let mut rendered_hr = render(&mut with_hr, &input, partitions);
            rendered_hr.extend(finish_with_capacity(&mut with_hr, fft_size / 2));
            let mut rendered_reference = render(&mut without_hr, &input, partitions);
            rendered_reference.extend(finish_with_capacity(&mut without_hr, fft_size / 2));
            assert_eq!(rendered_hr.len(), rendered_reference.len());
            assert!(rendered_hr.iter().all(|sample| sample.is_finite()));

            let contribution = rendered_hr
                .as_chunks::<2>()
                .0
                .iter()
                .zip(rendered_reference.as_chunks::<2>().0.iter())
                .map(|(with, without)| {
                    (with[0] - without[0])
                        .abs()
                        .max((with[1] - without[1]).abs())
                })
                .collect::<Vec<_>>();
            let peak_amplitude = contribution.iter().copied().fold(0.0_f32, f32::max);
            assert!(
                peak_amplitude > 1.0e-8,
                "silent fixed-gain HR at N={fft_size}"
            );
            let first_nonzero = contribution
                .iter()
                .position(|amplitude| *amplitude > 1.0e-8)
                .unwrap();
            let peak_frame = contribution
                .iter()
                .position(|amplitude| *amplitude == peak_amplitude)
                .unwrap();
            eprintln!(
                "prepared constant-gain N={fft_size} callbacks={partitions:?}: first={first_nonzero}, peak={peak_frame} ({peak_amplitude:.9}), accepted={input_frames}, emitted={}",
                rendered_hr.len() / 2
            );

            let maximum_partition_delta = if let Some(expected) = &baseline {
                assert_eq!(contribution.len(), expected.len());
                contribution
                    .iter()
                    .zip(expected)
                    .map(|(actual, reference)| (actual - reference).abs())
                    .fold(0.0_f32, f32::max)
            } else {
                baseline = Some(contribution.clone());
                0.0
            };
            eprintln!(
                "prepared HR gain partition delta N={fft_size} callbacks={partitions:?}: max={maximum_partition_delta:.9e}"
            );
            if fft_size <= 512 {
                assert_eq!((first_nonzero, peak_frame), (512, 512));
            }
            assert!(
                maximum_partition_delta <= 1.0e-7,
                "fixed-gain HR contribution changed across callbacks at N={fft_size}: {maximum_partition_delta}"
            );
        }
    }
}

#[test]
fn small_fft_fixed_latency_hr_contribution_is_partition_invariant() {
    const EXPECTED_LATENCY: usize = 512;
    for fft_size in [2, 32, 64, 128, 256] {
        let input_frames = 4096;
        let mut input = vec![0.0; input_frames * 2];
        input[0] = 0.25;
        input[1] = -0.125;
        let mut baseline: Option<Vec<f32>> = None;
        let mut observed_peak = Vec::new();

        for partitions in [&[1][..], &[17, 137, 256][..], &[512][..]] {
            let make = || {
                let mut plugin = neutral(fft_size);
                let channels = plugin.output_channels();
                let expected_ring_frames = fft_size.max(EXPECTED_LATENCY) * 4;
                assert_eq!(
                    plugin.output.output_accumulator.len(),
                    expected_ring_frames * channels
                );
                assert_eq!(plugin.output.hr_mix_gains.len(), expected_ring_frames);
                assert_eq!(
                    plugin.output.output_accumulator_mask + 1,
                    expected_ring_frames
                );
                assert_eq!(plugin.core.startup_padding_remaining, EXPECTED_LATENCY);

                let gain = (fft_size as f32 / 512.0).sqrt()
                    * plugin.gains.hr_sharpen.current()
                    * std::f32::consts::SQRT_2
                    / 512.0;
                let prepared_hops =
                    (input_frames + EXPECTED_LATENCY * 4 + 4096) / plugin.core.hop_size + 4;
                plugin.params.enable_hr_direct = true;
                plugin.hr_state.hr_direct_envelope = 1.0;
                plugin.hr_state.prev_hr_scale = gain;
                plugin.output.hr_test_gain_sequence = vec![gain; prepared_hops];
                plugin
            };
            let mut with_hr = make();
            let mut without_hr = make();
            without_hr.panning.cached_hr_active_channels.clear();

            let mut rendered_hr = render(&mut with_hr, &input, partitions);
            rendered_hr.extend(finish_with_capacity(&mut with_hr, EXPECTED_LATENCY / 2));
            let mut rendered_reference = render(&mut without_hr, &input, partitions);
            rendered_reference.extend(finish_with_capacity(&mut without_hr, EXPECTED_LATENCY / 2));
            assert_eq!(rendered_hr.len(), rendered_reference.len());
            assert!(rendered_hr.iter().all(|sample| sample.is_finite()));

            let contribution = rendered_hr
                .as_chunks::<2>()
                .0
                .iter()
                .zip(rendered_reference.as_chunks::<2>().0.iter())
                .map(|(with, without)| {
                    (with[0] - without[0])
                        .abs()
                        .max((with[1] - without[1]).abs())
                })
                .collect::<Vec<_>>();
            let peak_amplitude = contribution.iter().copied().fold(0.0_f32, f32::max);
            assert!(
                peak_amplitude > 1.0e-8,
                "silent candidate HR at N={fft_size}"
            );
            let first_nonzero = contribution
                .iter()
                .position(|amplitude| *amplitude > 1.0e-8)
                .unwrap();
            let peak_frame = contribution
                .iter()
                .position(|amplitude| *amplitude == peak_amplitude)
                .unwrap();
            observed_peak.push(peak_frame);

            let main_peak = rendered_reference
                .as_chunks::<2>()
                .0
                .iter()
                .enumerate()
                .max_by(|(_, left), (_, right)| {
                    let left = left.iter().map(|sample| sample.abs()).fold(0.0, f32::max);
                    let right = right.iter().map(|sample| sample.abs()).fold(0.0, f32::max);
                    left.total_cmp(&right)
                })
                .map(|(frame, _)| frame)
                .unwrap();
            let maximum_partition_delta = if let Some(expected) = &baseline {
                contribution
                    .iter()
                    .zip(expected)
                    .map(|(actual, reference)| (actual - reference).abs())
                    .fold(0.0_f32, f32::max)
            } else {
                baseline = Some(contribution);
                0.0
            };

            eprintln!(
                "source-tagged HR N={fft_size} callbacks={partitions:?}: first={first_nonzero}, HR_peak={peak_frame} ({peak_amplitude:.9}), main_peak={main_peak}, max_partition_delta={maximum_partition_delta:.9e}, emitted={}",
                rendered_hr.len() / 2
            );

            assert_eq!(main_peak, EXPECTED_LATENCY);
            assert_eq!(first_nonzero, EXPECTED_LATENCY);
            assert!(
                maximum_partition_delta <= 1.0e-6,
                "source-tagged HR contribution changed across callbacks at N={fft_size}: {maximum_partition_delta}"
            );
        }

        assert_eq!(observed_peak.len(), 3);
        assert_eq!(observed_peak, [EXPECTED_LATENCY; 3]);
    }
}

fn sample_digest(samples: &[f32]) -> u64 {
    let mut digest = 0xcbf29ce484222325_u64;
    for sample in samples {
        for byte in sample.to_bits().to_le_bytes() {
            digest = (digest ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
    digest
}

fn samples_from_f32le(bytes: &[u8]) -> Vec<f32> {
    assert_eq!(bytes.len() % std::mem::size_of::<f32>(), 0);
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect()
}

fn aud132_preedit_impulse(fft_size: usize, main_only: bool) -> Option<Vec<f32>> {
    let bytes: &[u8] = match (fft_size, main_only) {
        (1_024, false) => include_bytes!(
            "../../tests/data/aud132-preedit/n1024_delayed_control_impulse_full.f32le"
        ),
        (1_024, true) => include_bytes!(
            "../../tests/data/aud132-preedit/n1024_delayed_control_impulse_main.f32le"
        ),
        (2_048, false) => include_bytes!(
            "../../tests/data/aud132-preedit/n2048_delayed_control_impulse_full.f32le"
        ),
        (2_048, true) => include_bytes!(
            "../../tests/data/aud132-preedit/n2048_delayed_control_impulse_main.f32le"
        ),
        _ => return None,
    };
    Some(samples_from_f32le(bytes))
}

#[test]
fn n_ge_hr_fft_matches_pre_correction_hr_mixer_samples() {
    for fft_size in [512, 1024, 2048] {
        let input_frames = (fft_size * 4).max(4096);
        let mut input = vec![0.0; input_frames * 2];
        for frame in 0..input_frames {
            let level = [0.02, 0.2, 0.003, 0.1][frame / 512 % 4];
            let phase = std::f32::consts::TAU * 997.0 * frame as f32 / 48_000.0;
            let sample = level * phase.sin();
            input[frame * 2] = sample;
            input[frame * 2 + 1] = -0.5 * sample;
        }
        input[0] += 0.25;

        for partitions in [&[1][..], &[17, 137, 256][..], &[512][..]] {
            let mut current = neutral(fft_size);
            let mut pre_correction = neutral(fft_size);
            let mut main_only = neutral(fft_size);
            for plugin in [&mut current, &mut pre_correction] {
                assert!(fft_size >= plugin.fft.hr_fft_size);
                plugin.params.enable_hr_direct = true;
                plugin.hr_state.hr_direct_envelope = 1.0;
                plugin.hr_state.hr_transient_env = 1.0;
            }
            pre_correction.output.hr_use_pre_correction_mixer_for_test = true;

            let mut current_output = render(&mut current, &input, partitions);
            current_output.extend(finish_with_capacity(&mut current, fft_size / 2));
            let mut baseline_output = render(&mut pre_correction, &input, partitions);
            baseline_output.extend(finish_with_capacity(&mut pre_correction, fft_size / 2));
            let mut main_output = render(&mut main_only, &input, partitions);
            main_output.extend(finish_with_capacity(&mut main_only, fft_size / 2));

            assert_eq!(current_output.len(), baseline_output.len());
            assert!(current_output.iter().all(|sample| sample.is_finite()));
            let hr_peak = current_output
                .iter()
                .zip(&main_output)
                .map(|(with_hr, without_hr)| (with_hr - without_hr).abs())
                .fold(0.0_f32, f32::max);
            assert!(hr_peak > 1.0e-8, "silent N={fft_size} HR baseline");
            assert_eq!(
                current_output, baseline_output,
                "N={fft_size} callbacks={partitions:?} differs from the isolated pre-correction HR drain mixer"
            );
            let digest = sample_digest(&baseline_output);
            eprintln!(
                "pre-correction N={fft_size} callbacks={partitions:?}: frames={}, sample_digest={digest:016x}",
                baseline_output.len() / current.output_channels()
            );
        }
    }
}

#[test]
#[ignore = "Rejected AUD130 static output-delay candidate; failed impulse evidence is retained in audit/upmixer-hr-timing.md"]
fn rejected_static_delay_candidate_does_not_align_fixed_gain_impulses() {
    for fft_size in [2, 32, 64, 128, 256, 512, 1024, 2048] {
        let input_frames = (fft_size * 4).max(4096);
        let mut input = vec![0.0; input_frames * 2];
        input[0] = 0.25;
        input[1] = -0.125;

        let mut with_hr = neutral(fft_size);
        let mut without_hr = neutral(fft_size);
        for plugin in [&mut with_hr, &mut without_hr] {
            plugin.params.enable_hr_direct = true;
            plugin.hr_state.hr_direct_envelope = 1.0;
            plugin.hr_state.hr_transient_env = 1.0;
            plugin.hr_state.spectral_flux_smooth = 1.0e-6;
            if fft_size > 512 {
                // Candidate experiment only: remove the measured input delay
                // that currently shifts HR impulses later than the main path.
                plugin.hr_buffers.hr_delay_buffer.clear();
                plugin.hr_buffers.hr_delay_cursor = 0;
            }
        }
        without_hr.panning.cached_hr_active_channels.clear();
        let rendered_hr = render_with_reseeded_hr_envelope(&mut with_hr, &input);
        let rendered_reference = render_with_reseeded_hr_envelope(&mut without_hr, &input);
        let hr_contribution = rendered_hr
            .as_chunks::<2>()
            .0
            .iter()
            .zip(rendered_reference.as_chunks::<2>().0.iter())
            .map(|(with, without)| {
                (with[0] - without[0])
                    .abs()
                    .max((with[1] - without[1]).abs())
            })
            .collect::<Vec<_>>();
        let hr_peak = hr_contribution
            .iter()
            .enumerate()
            .max_by(|(_, left), (_, right)| left.total_cmp(right))
            .map(|(frame, amplitude)| (frame, *amplitude))
            .unwrap();
        assert!(
            hr_peak.1 > 1.0e-8,
            "silent candidate HR impulse at N={fft_size}"
        );

        let mut main = neutral(fft_size);
        let main_output = render(&mut main, &input, &[1]);
        let main_delay = 511_usize.saturating_sub(fft_size);
        let mut candidate_main = vec![0.0; main_delay * main.output_channels()];
        candidate_main.extend_from_slice(&main_output);
        let candidate_main_peak = candidate_main
            .chunks_exact(main.output_channels())
            .enumerate()
            .max_by(|(_, left), (_, right)| {
                let left = left.iter().map(|sample| sample.abs()).fold(0.0, f32::max);
                let right = right.iter().map(|sample| sample.abs()).fold(0.0, f32::max);
                left.total_cmp(&right)
            })
            .map(|(frame, _)| frame)
            .unwrap();
        eprintln!(
            "candidate common timeline N={fft_size}: HR peak={} ({:.9}), main peak after output delay={candidate_main_peak}, main delay={main_delay}, candidate latency={}",
            hr_peak.0,
            hr_peak.1,
            fft_size.max(511)
        );
        assert_eq!(hr_peak.0, candidate_main_peak);
        assert_eq!(candidate_main_peak, fft_size.max(511));
    }
}

#[test]
#[ignore = "Rejected AUD130 static output-delay candidate; failed tone residual evidence is retained in audit/upmixer-hr-timing.md"]
fn rejected_static_delay_candidate_does_not_align_fixed_gain_tone_phase() {
    const HR_FFT_SIZE: usize = 512;
    const TONE_BIN_512: f64 = 57.75;
    const TONE_BIN_1024: f64 = 57.5;
    const TONE_BIN_2048: f64 = 57.125;
    const INPUT_FRAMES: usize = 8192;
    const WINDOW_START: usize = 4096;

    for fft_size in [2, 32, 64, 128, 256, 512] {
        let tone_bin = match fft_size {
            1024 => TONE_BIN_1024,
            2048 => TONE_BIN_2048,
            _ => TONE_BIN_512,
        };
        let window_frames = if fft_size == 2048 { 4096 } else { 2048 };
        let omega = std::f64::consts::TAU * tone_bin / HR_FFT_SIZE as f64;
        let input: Vec<f32> = (0..INPUT_FRAMES)
            .flat_map(|frame| {
                let sample = (0.5 * (omega * frame as f64).sin()) as f32;
                [sample, 0.0]
            })
            .collect();

        let mut with_hr = neutral(fft_size);
        let mut without_hr = neutral(fft_size);
        for plugin in [&mut with_hr, &mut without_hr] {
            plugin.params.enable_hr_direct = true;
            plugin.hr_state.hr_direct_envelope = 1.0;
            plugin.hr_state.hr_transient_env = 1.0;
            plugin.hr_state.spectral_flux_smooth = 1.0e-6;
            if fft_size > HR_FFT_SIZE {
                plugin.hr_buffers.hr_delay_buffer.clear();
                plugin.hr_buffers.hr_delay_cursor = 0;
            }
        }
        without_hr.panning.cached_hr_active_channels.clear();
        let rendered_hr = render_with_reseeded_hr_envelope(&mut with_hr, &input);
        let rendered_reference = render_with_reseeded_hr_envelope(&mut without_hr, &input);
        let hr_only: Vec<f32> = rendered_hr
            .as_chunks::<2>()
            .0
            .iter()
            .zip(rendered_reference.as_chunks::<2>().0.iter())
            .map(|(with, without)| with[0] - without[0])
            .collect();

        let mut main = neutral(fft_size);
        let rendered_main = render(&mut main, &input, &[1]);
        let main_delay = 511_usize.saturating_sub(fft_size);
        let mut candidate_main = vec![0.0; main_delay];
        candidate_main.extend(
            rendered_main
                .chunks_exact(main.output_channels())
                .map(|frame| frame[0]),
        );
        let (hr_projection, hr_residual) =
            project_tone(&hr_only, WINDOW_START, window_frames, omega);
        let (main_projection, main_residual) =
            project_tone(&candidate_main, WINDOW_START, window_frames, omega);
        let hr_amplitude = 2.0 * hr_projection.norm() / window_frames as f64;
        let main_amplitude = 2.0 * main_projection.norm() / window_frames as f64;
        assert!(
            hr_amplitude > 1.0e-6,
            "silent candidate HR tone at N={fft_size}"
        );
        assert!(
            main_amplitude > 1.0e-3,
            "silent candidate main tone at N={fft_size}"
        );
        assert!(
            hr_residual <= 0.01 && main_residual <= 0.01,
            "candidate tone residual exceeds 1% at N={fft_size}: HR={hr_residual}, main={main_residual}"
        );
        let phase = (hr_projection * main_projection.conj()).arg();
        eprintln!(
            "candidate phase N={fft_size} tone_hz={:.4} window={WINDOW_START}..{} HR_amp={hr_amplitude:.9} main_amp={main_amplitude:.9} HR_residual={hr_residual:.6} main_residual={main_residual:.6} phase={phase:.9} main_delay={main_delay}",
            tone_bin * 48_000.0 / HR_FFT_SIZE as f64,
            WINDOW_START + window_frames
        );
        assert!(
            phase.abs() < 0.01,
            "candidate HR/main phase mismatch at N={fft_size}: {phase}"
        );
    }
}

#[test]
fn hr_toggle_resume_discards_partial_pre_toggle_input_before_eos() {
    const FFT_SIZE: usize = 64;
    const PRE_TOGGLE_FRAMES: usize = 700;
    const OFF_FRAMES: usize = 256;
    const RESUME_FRAMES: usize = 1024;

    let pre_toggle_input: Vec<f32> = (0..PRE_TOGGLE_FRAMES)
        .flat_map(|frame| {
            let sample = (std::f32::consts::TAU * 6_000.0 * frame as f32 / 48_000.0).sin() * 0.5;
            [sample, sample]
        })
        .collect();
    let silence = vec![0.0; OFF_FRAMES * 2];
    let resume = vec![0.0; RESUME_FRAMES * 2];

    let mut resumed_with_history = neutral(FFT_SIZE);
    let mut resumed_with_cleared_history = neutral(FFT_SIZE);
    for plugin in [&mut resumed_with_history, &mut resumed_with_cleared_history] {
        plugin.params.enable_hr_direct = true;
        plugin.hr_state.hr_direct_envelope = 1.0;
        plugin.hr_state.hr_transient_env = 1.0;
        plugin.hr_state.spectral_flux_smooth = 1.0e-6;
    }

    // Build identical active streams, then let the real per-callback disable
    // envelope settle while the HR input path is gated.
    let pre_outputs =
        render_with_reseeded_hr_transient(&mut resumed_with_history, &pre_toggle_input);
    let control_pre_outputs =
        render_with_reseeded_hr_transient(&mut resumed_with_cleared_history, &pre_toggle_input);
    assert_eq!(pre_outputs, control_pre_outputs);
    for plugin in [&mut resumed_with_history, &mut resumed_with_cleared_history] {
        plugin.params.enable_hr_direct = false;
    }
    let off_output = render(&mut resumed_with_history, &silence, &[1]);
    let control_off_output = render(&mut resumed_with_cleared_history, &silence, &[1]);
    assert_eq!(off_output, control_off_output);
    assert_eq!(resumed_with_history.hr_state.hr_direct_envelope, 0.0);
    assert_eq!(
        resumed_with_cleared_history.hr_state.hr_direct_envelope,
        0.0
    );
    assert_eq!(
        resumed_with_history.hr_buffers.hr_input_buffer_fill,
        resumed_with_cleared_history.hr_buffers.hr_input_buffer_fill
    );
    let retained = resumed_with_history.hr_buffers.hr_input_buffer_fill;
    assert!(retained > 0);
    let retained_peak = resumed_with_history.hr_buffers.hr_input_buffer[..retained]
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0_f32, f32::max);
    assert!(
        retained_peak > 1.0e-6,
        "no pre-toggle HR samples were retained"
    );

    // The control differs only by clearing the unprocessed HR input history.
    // Resuming must discard that partial history in both streams.
    resumed_with_cleared_history.hr_buffers.hr_input_buffer[..retained].fill(0.0);
    for plugin in [&mut resumed_with_history, &mut resumed_with_cleared_history] {
        plugin.params.enable_hr_direct = true;
    }
    let mut with_history = render_with_reseeded_hr_transient(&mut resumed_with_history, &resume);
    let mut with_cleared_history =
        render_with_reseeded_hr_transient(&mut resumed_with_cleared_history, &resume);
    let history_tail = finish_with_capacity(&mut resumed_with_history, 13);
    let cleared_tail = finish_with_capacity(&mut resumed_with_cleared_history, 13);
    with_history.extend_from_slice(&history_tail);
    with_cleared_history.extend_from_slice(&cleared_tail);
    assert_eq!(with_history.len(), with_cleared_history.len());
    assert!(with_history.iter().all(|sample| sample.is_finite()));
    assert!(with_cleared_history.iter().all(|sample| sample.is_finite()));
    assert_eq!(
        with_history, with_cleared_history,
        "HR resume reused partial pre-toggle input"
    );

    let accepted = PRE_TOGGLE_FRAMES + OFF_FRAMES + RESUME_FRAMES;
    let emitted = pre_outputs.len() / 2 + off_output.len() / 2 + with_history.len() / 2;
    let latency = FFT_SIZE.max(512);
    let input_phase = accepted % latency;
    let main_hop = FFT_SIZE / 2;
    let main_padding = (main_hop - input_phase % main_hop) % main_hop;
    let main_tail = latency + FFT_SIZE - main_hop + main_padding;
    let hr_hop = resumed_with_history.fft.hr_fft_size / 2;
    let hr_delay = resumed_with_history.hr_buffers.hr_delay_buffer.len() / 2;
    let hr_padding = (hr_hop - (input_phase + hr_delay) % hr_hop) % hr_hop;
    let hr_tail = latency + hr_delay + resumed_with_history.fft.hr_fft_size - hr_hop + hr_padding;
    assert_eq!(emitted, accepted + main_tail.max(hr_tail));
    eprintln!(
        "HR toggle reset N={FFT_SIZE}: discarded_partial_frames={} retained_peak={retained_peak:.9} resumed_delta=0 accepted={accepted} emitted={emitted} resume_plus_EOS_emitted={} EOS_tail={}",
        retained / 2,
        with_history.len() / 2,
        history_tail.len() / 2
    );
    assert_eq!(with_history, with_cleared_history);
}

#[test]
fn aud132_above512_hr_resume_clears_delay_history_and_partial_hop() {
    const PRE_TOGGLE_FRAMES: usize = 2_048;
    const OFF_FRAMES: usize = 220;
    const RESUME_FRAMES: usize = 2_048;

    for fft_size in [1_024_usize, 2_048] {
        let pre_toggle: Vec<f32> = (0..PRE_TOGGLE_FRAMES)
            .flat_map(|frame| {
                let phase = std::f32::consts::TAU * 7_000.0 * frame as f32 / 48_000.0;
                [0.4 * phase.sin(), -0.3 * phase.cos()]
            })
            .collect();
        let silence = vec![0.0; OFF_FRAMES * 2];
        let mut resume = vec![0.0; RESUME_FRAMES * 2];
        for frame in 0..512 {
            let value = (std::f32::consts::TAU * 5_000.0 * frame as f32 / 48_000.0).sin() * 0.25;
            resume[frame * 2] = value;
            resume[frame * 2 + 1] = -0.5 * value;
        }

        let mut with_stale_history = neutral(fft_size);
        let mut manually_cleared_control = neutral(fft_size);
        for plugin in [&mut with_stale_history, &mut manually_cleared_control] {
            plugin.params.enable_hr_direct = true;
            plugin.hr_state.hr_direct_envelope = 1.0;
            plugin.hr_state.hr_transient_env = 1.0;
            plugin.hr_state.spectral_flux_smooth = 1.0e-6;
        }
        let before = render_with_reseeded_hr_transient(&mut with_stale_history, &pre_toggle);
        let control_before =
            render_with_reseeded_hr_transient(&mut manually_cleared_control, &pre_toggle);
        assert_eq!(before, control_before);
        for plugin in [&mut with_stale_history, &mut manually_cleared_control] {
            plugin.params.enable_hr_direct = false;
        }
        let off = render_with_reseeded_hr_transient(&mut with_stale_history, &silence);
        let control_off =
            render_with_reseeded_hr_transient(&mut manually_cleared_control, &silence);
        assert_eq!(off, control_off);
        assert_eq!(with_stale_history.hr_state.hr_direct_envelope, 0.0);
        assert!(!with_stale_history.hr_buffers.hr_input_active);

        let delay_peak = with_stale_history
            .hr_buffers
            .hr_delay_buffer
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0_f32, f32::max);
        let partial_fill = with_stale_history.hr_buffers.hr_input_buffer_fill;
        let partial_peak = with_stale_history.hr_buffers.hr_input_buffer[..partial_fill]
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0_f32, f32::max);
        assert!(
            delay_peak > 1.0e-6,
            "N={fft_size} delay history was not populated"
        );
        assert!(
            partial_peak > 1.0e-6,
            "N={fft_size} partial HR hop was not populated"
        );
        assert!(partial_fill > 0 && partial_fill < fft_size * 2);

        manually_cleared_control
            .hr_buffers
            .hr_delay_buffer
            .fill(0.0);
        manually_cleared_control.hr_buffers.hr_delay_cursor = 0;
        manually_cleared_control.hr_buffers.hr_input_buffer[..partial_fill].fill(0.0);
        for plugin in [&mut with_stale_history, &mut manually_cleared_control] {
            plugin.params.enable_hr_direct = true;
        }
        let mut resumed = render_with_reseeded_hr_transient(&mut with_stale_history, &resume);
        let mut resumed_control =
            render_with_reseeded_hr_transient(&mut manually_cleared_control, &resume);
        let resumed_tail = finish_with_capacity(&mut with_stale_history, 17);
        let control_tail = finish_with_capacity(&mut manually_cleared_control, 17);
        resumed.extend_from_slice(&resumed_tail);
        resumed_control.extend_from_slice(&control_tail);
        assert_eq!(resumed.len(), resumed_control.len());
        assert!(resumed.iter().all(|sample| sample.is_finite()));
        assert_eq!(
            resumed, resumed_control,
            "N={fft_size} resumed HR output reused old delay/input history"
        );
        eprintln!(
            "AUD132 HR resume N={fft_size} D={} delay_peak={delay_peak:.9} partial_fill={partial_fill} partial_peak={partial_peak:.9} resumed_frames={} eos_frames={} digest={:016x}",
            with_stale_history.hr_buffers.hr_delay_buffer.len() / 2,
            resumed.len() / 2,
            resumed_tail.len() / 2,
            sample_digest(&resumed),
        );
    }
}

fn render_with_reseeded_hr_envelope(plugin: &mut UpmixerPlugin, input: &[f32]) -> Vec<f32> {
    assert_eq!(input.len() % 2, 0);
    let frames = input.len() / 2;
    let mut output = Vec::with_capacity(frames * plugin.output_channels());
    let mut frame_output = [f32::NAN; 2];
    for frame in 0..frames {
        // This measurement isolates the fixed-gain path phase. Re-seeding the
        // detector before each one-frame callback keeps a nonzero, nearly
        // constant transient mix without asserting callback invariance.
        plugin.hr_state.hr_direct_envelope = 1.0;
        plugin.hr_state.hr_transient_env = 1.0;
        plugin.hr_state.spectral_flux_smooth = 1.0e-6;
        plugin.hr_state.prev_power_spectrum.fill(0.0);
        frame_output.fill(f32::NAN);
        assert_eq!(
            plugin
                .process(
                    &input[frame * 2..frame * 2 + 2],
                    &mut frame_output,
                    &ProcessContext::new(48_000, 1),
                )
                .unwrap(),
            1
        );
        output.extend_from_slice(&frame_output);
    }
    output
}

fn render_with_reseeded_hr_transient(plugin: &mut UpmixerPlugin, input: &[f32]) -> Vec<f32> {
    assert_eq!(input.len() % 2, 0);
    let frames = input.len() / 2;
    let mut output = Vec::with_capacity(frames * plugin.output_channels());
    let mut frame_output = [f32::NAN; 2];
    for frame in 0..frames {
        // Keep transient gain measurable while preserving the real HR enable
        // envelope's attack/decay and the streaming input/output buffers.
        plugin.hr_state.hr_transient_env = 1.0;
        plugin.hr_state.spectral_flux_smooth = 1.0e-6;
        plugin.hr_state.prev_power_spectrum.fill(0.0);
        frame_output.fill(f32::NAN);
        assert_eq!(
            plugin
                .process(
                    &input[frame * 2..frame * 2 + 2],
                    &mut frame_output,
                    &ProcessContext::new(48_000, 1),
                )
                .unwrap(),
            1
        );
        output.extend_from_slice(&frame_output);
    }
    output
}

fn project_tone(
    samples: &[f32],
    window_start: usize,
    window_frames: usize,
    omega: f64,
) -> (Complex<f64>, f64) {
    let mut projection = Complex::new(0.0, 0.0);
    for (offset, &sample) in samples[window_start..window_start + window_frames]
        .iter()
        .enumerate()
    {
        let angle = omega * (window_start + offset) as f64;
        let sample = f64::from(sample);
        projection.re += sample * angle.cos();
        projection.im -= sample * angle.sin();
    }

    let scale = 2.0 / window_frames as f64;
    let mut residual_energy = 0.0;
    let mut fitted_energy = 0.0;
    for (offset, &sample) in samples[window_start..window_start + window_frames]
        .iter()
        .enumerate()
    {
        let angle = omega * (window_start + offset) as f64;
        let fitted = scale * (projection.re * angle.cos() - projection.im * angle.sin());
        let residual = f64::from(sample) - fitted;
        residual_energy += residual * residual;
        fitted_energy += fitted * fitted;
    }
    let residual_ratio = (residual_energy / fitted_energy).sqrt();
    (projection, residual_ratio)
}

#[test]
fn first_and_final_impulses_survive_finite_zero_padded_streams() {
    let n = 512;
    for frames in [1, 255, 256, 257, 511, 512, 513] {
        let mut input = vec![0.0; (frames + n * 2) * 2];
        input[0] = 0.25;
        input[(frames - 1) * 2 + 1] = -0.125;
        for partitions in [&[1][..], &[17, 3, 97][..], &[512][..]] {
            let mut plugin = neutral(n);
            let actual = render(&mut plugin, &input, partitions);
            for (index, &sample) in actual.iter().enumerate() {
                let expected = index.checked_sub(n * 2).map_or(0.0, |i| input[i]);
                assert!((sample - expected).abs() < 2.0e-6);
            }
        }
    }
}

#[test]
fn fft_reconfiguration_restores_the_prefixed_window_timeline() {
    let mut plugin = neutral(512);
    render(&mut plugin, &vec![0.1; 700 * 2], &[97]);
    plugin
        .set_parameter(
            sotf_host::ParameterId::from("low_latency"),
            sotf_host::ParameterValue::Bool(true),
        )
        .unwrap();
    assert_eq!(plugin.latency_samples(), 1024);
    neutralize_tables(&mut plugin);
    let mut input = vec![0.0; 4096 * 2];
    input[0] = 0.2;
    input[1] = -0.1;
    input[513 * 2] = -0.15;
    let actual = render(&mut plugin, &input, &[17, 97, 3]);
    let expected = render(&mut neutral(1024), &input, &[17, 97, 3]);
    assert!(
        actual
            .iter()
            .zip(&expected)
            .all(|(a, b)| (a - b).abs() < 2.0e-6)
    );
}

#[test]
fn aud132_hr_tag_startup_tracks_reconfiguration_across_512_and_1024() {
    for (old_fft_size, new_fft_size) in [(512_usize, 1_024_usize), (1_024, 512)] {
        let make_plugin = |include_hr: bool| {
            let mut plugin = neutral(old_fft_size);
            plugin.params.enable_hr_direct = true;
            plugin.hr_state.hr_direct_envelope = 1.0;
            plugin.hr_state.hr_transient_env = 1.0;
            if !include_hr {
                plugin.panning.cached_hr_active_channels.clear();
            }
            plugin
        };
        let mut reconfigured = make_plugin(true);
        let mut main_only = make_plugin(false);
        let pre_toggle = vec![0.1; 733 * 2];
        render(&mut reconfigured, &pre_toggle, &[97, 137]);
        render(&mut main_only, &pre_toggle, &[97, 137]);
        reconfigured.resize_fft(new_fft_size);
        main_only.resize_fft(new_fft_size);
        neutralize_tables(&mut reconfigured);
        neutralize_tables(&mut main_only);

        let delay_frames = reconfigured.hr_buffers.hr_delay_buffer.len() / 2;
        let startup_discard = reconfigured.fft.hr_fft_size / 2 + delay_frames;
        assert_eq!(reconfigured.core.fft_size, new_fft_size);
        assert_eq!(
            reconfigured.hr_buffers.hr_startup_discard_remaining,
            startup_discard
        );
        assert_eq!(
            reconfigured.hr_buffers.hr_next_add_source_frame,
            -(startup_discard as i64)
        );
        assert!(
            reconfigured
                .hr_buffers
                .hr_delay_buffer
                .iter()
                .all(|v| *v == 0.0)
        );
        assert_eq!(reconfigured.hr_buffers.hr_delay_cursor, 0);

        for plugin in [&mut reconfigured, &mut main_only] {
            plugin.params.enable_hr_direct = true;
            plugin.hr_state.hr_direct_envelope = 1.0;
            plugin.hr_state.hr_transient_env = 1.0;
            let gain = (new_fft_size as f32 / 512.0).sqrt()
                * plugin.gains.hr_sharpen.current()
                * std::f32::consts::SQRT_2
                / 512.0;
            plugin.hr_state.prev_hr_scale = gain;
            plugin.output.hr_test_gain_sequence = vec![gain; 64];
        }
        main_only.panning.cached_hr_active_channels.clear();

        let mut input = vec![0.0; (new_fft_size * 8) * 2];
        input[0] = 0.25;
        input[1] = -0.125;
        input[new_fft_size * 2 + 2] = -0.1;
        let actual = render(&mut reconfigured, &input, &[17, 137, 256]);
        let expected_output = render(&mut main_only, &input, &[17, 137, 256]);
        let actual_tail = finish_with_capacity(&mut reconfigured, new_fft_size);
        let expected_tail = finish_with_capacity(&mut main_only, new_fft_size);
        assert_eq!(actual.len(), expected_output.len());
        assert_eq!(actual_tail.len(), expected_tail.len());
        let full_output: Vec<f32> = actual
            .iter()
            .zip(&expected_output)
            .map(|(full, main)| full - main)
            .chain(
                actual_tail
                    .iter()
                    .zip(&expected_tail)
                    .map(|(full, main)| full - main),
            )
            .collect();
        assert!(actual.iter().all(|sample| sample.is_finite()));
        assert!(actual_tail.iter().all(|sample| sample.is_finite()));
        let hr_peak_samples: Vec<f32> = full_output
            .as_chunks::<2>()
            .0
            .iter()
            .map(|frame| frame[0].abs().max(frame[1].abs()))
            .collect();
        assert!(hr_peak_samples.iter().any(|value| *value > 1.0e-8));
        let peak = hr_peak_samples
            .iter()
            .position(|value| *value == hr_peak_samples.iter().copied().fold(0.0_f32, f32::max))
            .unwrap();
        assert_eq!(peak, new_fft_size);
        eprintln!(
            "AUD132 reconfigure {old_fft_size}->{new_fft_size} frames={} tail={} HR_peak={peak} digest={:016x}",
            actual.len() / 2,
            actual_tail.len() / 2,
            sample_digest(&actual),
        );
    }
}

#[test]
#[ignore = "AUD132 rejected candidate: removing the HR input delay preserves the N=2048 tone residual above the fixed 1% ceiling"]
fn above_512_hr_delay_baseline_and_source_aligned_candidate_tone() {
    const INPUT_FRAMES: usize = 24_576;
    const WINDOW_START: usize = 8_192;
    const WINDOW_FRAMES: usize = 4_096;
    let partitions: [(&str, &[usize]); 4] = [
        ("p1", &[1]),
        ("p17_137_256", &[17, 137, 256]),
        ("p512", &[512]),
        ("pwhole", &[INPUT_FRAMES]),
    ];

    for fft_size in [512_usize, 1_024, 2_048] {
        // An exact HR bin avoids fractional-bin STFT modulation; the paired
        // impulse probe independently identifies the integer-frame delay.
        let tone_bin: f64 = 57.0;
        let omega = std::f64::consts::TAU * tone_bin / 512.0;
        let input: Vec<f32> = (0..INPUT_FRAMES)
            .flat_map(|frame| {
                let phase = omega * frame as f64;
                [(0.4 * phase.sin()) as f32, (0.2 * phase.cos()) as f32]
            })
            .collect();

        for (route, remove_hr_input_delay) in [("delayed_control", false), ("source_aligned", true)]
        {
            let delay_frames = if remove_hr_input_delay {
                0
            } else {
                fft_size.saturating_sub(512) / 2
            };
            let mut first_hr: Option<Vec<f32>> = None;

            for (partition_name, callback_sizes) in partitions {
                let make_plugin = |include_hr: bool| {
                    let mut plugin = neutral(fft_size);
                    plugin.params.enable_hr_direct = true;
                    plugin.hr_state.hr_direct_envelope = 1.0;
                    plugin.hr_state.hr_transient_env = 1.0;
                    // Reconstruct the pre-AUD132 drain route for this rejected
                    // delay-removal candidate. Keep its original 256-frame
                    // startup discard so the unchanged >1% result remains
                    // attributable to removing the input delay alone.
                    plugin.output.hr_use_pre_correction_mixer_for_test = true;
                    plugin.hr_buffers.hr_startup_discard_remaining = plugin.fft.hr_fft_size / 2;
                    plugin.hr_buffers.hr_next_add_source_frame =
                        -(plugin.fft.hr_fft_size as i64 / 2);
                    let hr_gain = (fft_size as f32 / 512.0).sqrt()
                        * plugin.gains.hr_sharpen.current()
                        * std::f32::consts::SQRT_2
                        / 512.0;
                    plugin.hr_state.prev_hr_scale = hr_gain;
                    let gain_hops = (INPUT_FRAMES + fft_size * 4) / plugin.core.hop_size + 16;
                    plugin.output.hr_test_gain_sequence = vec![hr_gain; gain_hops];
                    if remove_hr_input_delay {
                        plugin.hr_buffers.hr_delay_buffer.clear();
                        plugin.hr_buffers.hr_delay_cursor = 0;
                    } else {
                        plugin
                            .hr_buffers
                            .hr_delay_buffer
                            .resize(delay_frames * 2, 0.0);
                        plugin.hr_buffers.hr_delay_buffer.fill(0.0);
                        plugin.hr_buffers.hr_delay_cursor = 0;
                    }
                    if !include_hr {
                        plugin.panning.cached_hr_active_channels.clear();
                    }
                    plugin
                };

                let mut with_hr = make_plugin(true);
                let mut main_only = make_plugin(false);
                let mut full_output = render(&mut with_hr, &input, callback_sizes);
                full_output.extend(finish_with_capacity(&mut with_hr, fft_size));
                let mut main_output = render(&mut main_only, &input, callback_sizes);
                main_output.extend(finish_with_capacity(&mut main_only, fft_size));
                assert_eq!(full_output.len(), main_output.len());
                assert!(full_output.iter().all(|sample| sample.is_finite()));
                assert!(main_output.iter().all(|sample| sample.is_finite()));

                let hr_left: Vec<f32> = full_output
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .zip(main_output.as_chunks::<2>().0.iter())
                    .map(|(full, main)| full[0] - main[0])
                    .collect();
                let (hr_projection, hr_residual) =
                    project_tone(&hr_left, WINDOW_START, WINDOW_FRAMES, omega);
                let (main_projection, main_residual) = project_tone(
                    &main_output
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|frame| frame[0])
                        .collect::<Vec<_>>(),
                    WINDOW_START,
                    WINDOW_FRAMES,
                    omega,
                );
                let hr_amplitude = 2.0 * hr_projection.norm() / WINDOW_FRAMES as f64;
                let phase = (hr_projection * main_projection.conj()).arg();
                let expected_phase = -omega * delay_frames as f64;
                let phase_error = (phase - expected_phase + std::f64::consts::PI)
                    .rem_euclid(std::f64::consts::TAU)
                    - std::f64::consts::PI;
                if partition_name == "p1" {
                    capture_samples(fft_size, route, "full", &full_output);
                    capture_samples(fft_size, route, "main", &main_output);
                    capture_samples(fft_size, route, "hr", &hr_left);
                }
                let partition_delta = if let Some(reference) = &first_hr {
                    assert_eq!(hr_left.len(), reference.len());
                    hr_left
                        .iter()
                        .zip(reference)
                        .map(|(actual, expected)| (actual - expected).abs())
                        .fold(0.0_f32, f32::max)
                } else {
                    first_hr = Some(hr_left.clone());
                    0.0
                };

                eprintln!(
                    "AUD132 tone N={fft_size} route={route} delay={delay_frames} callbacks={partition_name} frames={} hr_amp={hr_amplitude:.9} hr_residual={hr_residual:.7} main_residual={main_residual:.7} phase={phase:.9} expected_phase={expected_phase:.9} phase_error={phase_error:.9} partition_delta={partition_delta:.9e} full_fnv={:016x} main_fnv={:016x} hr_fnv={:016x}",
                    full_output.len() / 2,
                    sample_digest(&full_output),
                    sample_digest(&main_output),
                    sample_digest(&hr_left),
                );
                assert!(
                    hr_amplitude > 1.0e-6,
                    "silent HR projection at N={fft_size}"
                );
                assert!(
                    hr_residual <= 0.01,
                    "HR tone residual at N={fft_size}: {hr_residual}"
                );
                assert!(
                    main_residual <= 0.01,
                    "main tone residual at N={fft_size}: {main_residual}"
                );
                assert!(
                    phase_error.abs() < 0.02,
                    "phase mismatch at N={fft_size}: {phase_error}"
                );
                assert!(
                    partition_delta <= 1.0e-6,
                    "HR tone changed across callbacks at N={fft_size}, route={route}: {partition_delta}"
                );
            }
        }
    }
}

#[test]
fn aud132_live_source_tags_align_above_512_exact_and_noninteger_tones() {
    const INPUT_FRAMES: usize = 24_576;
    const WINDOW_START: usize = 8_192;
    const WINDOW_FRAMES: usize = 4_096;
    let partitions: [(&str, &[usize]); 3] = [
        ("p17_137_256", &[17, 137, 256]),
        ("p512", &[512]),
        ("pwhole", &[INPUT_FRAMES]),
    ];

    for (fft_size, tone_bin) in [
        (1_024_usize, 57.0_f64),
        (1_024, 57.5),
        (2_048, 57.0),
        (2_048, 57.125),
    ] {
        let omega = std::f64::consts::TAU * tone_bin / 512.0;
        let input: Vec<f32> = (0..INPUT_FRAMES)
            .flat_map(|frame| {
                let phase = omega * frame as f64;
                [(0.4 * phase.sin()) as f32, (0.2 * phase.cos()) as f32]
            })
            .collect();
        for (partition_name, callback_sizes) in partitions {
            let make_plugin = |include_hr: bool| {
                let mut plugin = neutral(fft_size);
                plugin.params.enable_hr_direct = true;
                plugin.hr_state.hr_direct_envelope = 1.0;
                plugin.hr_state.hr_transient_env = 1.0;
                let hr_gain = (fft_size as f32 / 512.0).sqrt()
                    * plugin.gains.hr_sharpen.current()
                    * std::f32::consts::SQRT_2
                    / 512.0;
                plugin.hr_state.prev_hr_scale = hr_gain;
                plugin.output.hr_test_gain_sequence = vec![hr_gain; 256];
                if !include_hr {
                    plugin.panning.cached_hr_active_channels.clear();
                }
                plugin
            };

            let mut with_hr = make_plugin(true);
            let mut main_only = make_plugin(false);
            assert_eq!(with_hr.latency_samples(), fft_size);
            let mut full_output = render(&mut with_hr, &input, callback_sizes);
            full_output.extend(finish_with_capacity(&mut with_hr, fft_size));
            let mut main_output = render(&mut main_only, &input, callback_sizes);
            main_output.extend(finish_with_capacity(&mut main_only, fft_size));
            assert_eq!(full_output.len(), main_output.len());

            let hr_left: Vec<f32> = full_output
                .as_chunks::<2>()
                .0
                .iter()
                .zip(main_output.as_chunks::<2>().0.iter())
                .map(|(full, main)| full[0] - main[0])
                .collect();
            let (hr_projection, hr_residual) =
                project_tone(&hr_left, WINDOW_START, WINDOW_FRAMES, omega);
            let main_left: Vec<f32> = main_output
                .as_chunks::<2>()
                .0
                .iter()
                .map(|frame| frame[0])
                .collect();
            let (main_projection, main_residual) =
                project_tone(&main_left, WINDOW_START, WINDOW_FRAMES, omega);
            let hr_amplitude = 2.0 * hr_projection.norm() / WINDOW_FRAMES as f64;
            let phase = (hr_projection * main_projection.conj()).arg();
            let phase_error = (phase + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU)
                - std::f64::consts::PI;
            eprintln!(
                "AUD132 live tone N={fft_size} k={tone_bin} callbacks={partition_name} frames={} hr_amp={hr_amplitude:.9} hr_residual={hr_residual:.8} main_residual={main_residual:.8} relative_phase={phase:.9} main_fnv={:016x} full_fnv={:016x}",
                full_output.len() / 2,
                sample_digest(&main_output),
                sample_digest(&full_output),
            );
            assert!(
                hr_amplitude > 1.0e-6,
                "silent HR tone at N={fft_size}, k={tone_bin}"
            );
            assert!(
                hr_residual <= 0.01,
                "live tagged HR tone residual at N={fft_size}, k={tone_bin}: {hr_residual}"
            );
            assert!(
                main_residual <= 0.01,
                "main tone residual at N={fft_size}, k={tone_bin}: {main_residual}"
            );
            assert!(
                phase_error.abs() < 0.02,
                "source-tagged HR/main phase at N={fft_size}, k={tone_bin}: {phase_error}"
            );
        }
    }
}

#[test]
fn aud132_nonstationary_hr_gain_stays_with_its_source_frame() {
    const INPUT_FRAMES: usize = 8_192;
    let partitions: [(&str, &[usize]); 4] = [
        ("p1", &[1]),
        ("p17_137_256", &[17, 137, 256]),
        ("p512", &[512]),
        ("pwhole", &[INPUT_FRAMES]),
    ];
    let source_gains = [
        0.001_f32, 0.008, 0.002, 0.012, 0.003, 0.010, 0.0015, 0.009, 0.0025, 0.011, 0.001, 0.007,
    ];
    let gain_sequence = source_gains.repeat(16);

    for fft_size in [1_024_usize, 2_048] {
        let hop = fft_size / 2;
        let delay_frames = (fft_size - 512) / 2;
        let input: Vec<f32> = (0..INPUT_FRAMES)
            .flat_map(|frame| {
                let burst = if [0, 513, 1_733, 4_091, 6_888].contains(&frame) {
                    0.2
                } else {
                    0.0
                };
                let phase = std::f32::consts::TAU * 7_000.0 * frame as f32 / 48_000.0;
                let sample = 0.08 * phase.sin() + burst;
                [sample, -0.6 * sample]
            })
            .collect();

        let make_plugin = |route: &str| {
            let mut plugin = neutral(fft_size);
            plugin.params.enable_hr_direct = true;
            plugin.hr_state.hr_direct_envelope = 1.0;
            plugin.hr_state.hr_transient_env = 1.0;
            plugin.hr_state.prev_hr_scale = source_gains[0];
            plugin.output.hr_test_gain_sequence = gain_sequence.clone();
            if route == "legacy" {
                plugin.output.hr_use_pre_correction_mixer_for_test = true;
                plugin.hr_buffers.hr_startup_discard_remaining = plugin.fft.hr_fft_size / 2;
                plugin.hr_buffers.hr_next_add_source_frame = -(plugin.fft.hr_fft_size as i64 / 2);
            }
            plugin
        };

        let gain_for_source = |source_frame: usize| {
            let analysis_block = 1 + source_frame / hop;
            let within_hop = source_frame % hop;
            let start = gain_sequence[analysis_block - 1];
            let target = gain_sequence[analysis_block];
            start + (target - start) * ((within_hop + 1) as f32 / hop as f32)
        };

        for (partition_name, callback_sizes) in partitions {
            let mut tagged = make_plugin("tagged");
            let mut legacy = make_plugin("legacy");
            let mut main_only = make_plugin("main");
            main_only.panning.cached_hr_active_channels.clear();

            let mut tagged_output = render(&mut tagged, &input, callback_sizes);
            tagged_output.extend(finish_with_capacity(&mut tagged, fft_size));
            let mut legacy_output = render(&mut legacy, &input, callback_sizes);
            legacy_output.extend(finish_with_capacity(&mut legacy, fft_size));
            let mut main_output = render(&mut main_only, &input, callback_sizes);
            main_output.extend(finish_with_capacity(&mut main_only, fft_size));
            assert_eq!(tagged_output.len(), legacy_output.len());
            assert_eq!(tagged_output.len(), main_output.len());

            let mut maximum_delta = 0.0_f32;
            let mut expected_nonstationary_delta = 0.0_f32;
            let mut maximum_hr = 0.0_f32;
            for source_frame in 0..INPUT_FRAMES {
                let tagged_output_frame = fft_size + source_frame;
                let legacy_output_frame = tagged_output_frame + delay_frames;
                if legacy_output_frame >= tagged_output.len() / 2 {
                    break;
                }
                let current_gain = gain_for_source(source_frame);
                let delayed_gain = gain_for_source(source_frame + delay_frames);
                for channel in 0..2 {
                    let tagged_sample = tagged_output[tagged_output_frame * 2 + channel]
                        - main_output[tagged_output_frame * 2 + channel];
                    let legacy_sample = legacy_output[legacy_output_frame * 2 + channel]
                        - main_output[legacy_output_frame * 2 + channel];
                    let expected = legacy_sample * current_gain / delayed_gain;
                    maximum_delta = maximum_delta.max((tagged_sample - expected).abs());
                    expected_nonstationary_delta =
                        expected_nonstationary_delta.max((legacy_sample - expected).abs());
                    maximum_hr = maximum_hr.max(tagged_sample.abs());
                }
            }

            eprintln!(
                "AUD132 gain pairing N={fft_size} D={delay_frames} callbacks={partition_name} frames={} HR_peak={maximum_hr:.9} source_gain_effect={expected_nonstationary_delta:.9e} tagged_vs_source_gain_delta={maximum_delta:.9e}",
                tagged_output.len() / 2,
            );
            assert!(
                maximum_hr > 1.0e-8,
                "silent nonstationary HR fixture at N={fft_size}"
            );
            assert!(
                expected_nonstationary_delta > 1.0e-6,
                "gain schedule did not distinguish source-time from drain-time pairing at N={fft_size}"
            );
            assert!(
                maximum_delta <= 1.0e-6,
                "N={fft_size} HR source sample received a gain from a different main source frame: {maximum_delta}"
            );
        }
    }
}

#[test]
fn aud132_tagged_hr_impulse_hits_source_latency_and_compares_legacy_samples() {
    const INPUT_FRAMES: usize = 8_192;
    let partitions: [(&str, &[usize]); 4] = [
        ("p1", &[1]),
        ("p17_137_256", &[17, 137, 256]),
        ("p512", &[512]),
        ("pwhole", &[INPUT_FRAMES]),
    ];
    for fft_size in [512_usize, 1_024, 2_048, 4_096, 8_192] {
        let delay_frames = neutral(fft_size).hr_buffers.hr_delay_buffer.len() / 2;
        let mut partition_reference: Option<Vec<f32>> = None;
        for (partition_name, callback_sizes) in partitions {
            if fft_size >= 4_096 && partition_name == "p1" {
                continue;
            }

            let input_peak = if fft_size == 8_192 { 0.005 } else { 0.25 };
            let mut input = vec![0.0; INPUT_FRAMES * 2];
            input[0] = input_peak;
            input[1] = -input_peak * 0.5;

            let make_plugin = |legacy_drain: bool, include_hr: bool| {
                let mut plugin = neutral(fft_size);
                if fft_size == 8_192 {
                    // Keep this full-output retiming oracle below the production
                    // safety cap so limiter behavior cannot mask route differences.
                    plugin.safety.safety_cap_db = 3.0;
                    plugin
                        .param_smoothers
                        .safety_cap_db_smoother
                        .set_target(3.0);
                    plugin
                        .param_smoothers
                        .safety_cap_db_smoother
                        .next_n(4_096);
                    plugin.update_safety_cap_cache();
                }
                plugin.params.enable_hr_direct = true;
                plugin.hr_state.hr_direct_envelope = 1.0;
                plugin.hr_state.hr_transient_env = 1.0;
                let hr_gain = (fft_size as f32 / 512.0).sqrt()
                    * plugin.gains.hr_sharpen.current()
                    * std::f32::consts::SQRT_2
                    / 512.0;
                plugin.hr_state.prev_hr_scale = hr_gain;
                plugin.output.hr_test_gain_sequence = vec![hr_gain; 256];
                if legacy_drain {
                    plugin.output.hr_use_pre_correction_mixer_for_test = true;
                    plugin.hr_buffers.hr_startup_discard_remaining = plugin.fft.hr_fft_size / 2;
                    plugin.hr_buffers.hr_next_add_source_frame =
                        -(plugin.fft.hr_fft_size as i64 / 2);
                }
                if !include_hr {
                    plugin.panning.cached_hr_active_channels.clear();
                }
                plugin
            };

            let mut tagged = make_plugin(false, true);
            let mut legacy = make_plugin(true, true);
            let mut main_only = make_plugin(false, false);

            let mut tagged_output = render(&mut tagged, &input, callback_sizes);
            tagged_output.extend(finish_with_capacity(&mut tagged, fft_size));
            let mut legacy_output = render(&mut legacy, &input, callback_sizes);
            legacy_output.extend(finish_with_capacity(&mut legacy, fft_size));
            let mut main_output = render(&mut main_only, &input, callback_sizes);
            main_output.extend(finish_with_capacity(&mut main_only, fft_size));
            assert_eq!(tagged_output.len(), legacy_output.len());
            assert_eq!(tagged_output.len(), main_output.len());
            assert_eq!(
                tagged_output.len() / 2,
                INPUT_FRAMES + fft_size * 3 / 2,
                "N={fft_size} impulse render/EOS frame count"
            );

            let tagged_hr: Vec<f32> = tagged_output
                .iter()
                .zip(&main_output)
                .map(|(full, main)| full - main)
                .collect();
            let legacy_hr: Vec<f32> = legacy_output
                .iter()
                .zip(&main_output)
                .map(|(full, main)| full - main)
                .collect();
            let mut maximum_retimed_delta = 0.0_f32;
            let mut maximum_retimed_sample = (0_usize, 0_usize, 0.0_f32, 0.0_f32);
            let mut reconstructed = main_output.clone();
            for frame in 0..tagged_output.len() / 2 {
                let destination = frame * 2;
                let source = (frame + delay_frames) * 2;
                for channel in 0..2 {
                    let retimed_hr = legacy_hr.get(source + channel).copied().unwrap_or(0.0);
                    if frame >= fft_size {
                        reconstructed[destination + channel] += retimed_hr;
                    }
                    let tagged_sample = tagged_output[destination + channel];
                    let expected_sample = reconstructed[destination + channel];
                    let sample_delta = (tagged_sample - expected_sample).abs();
                    if sample_delta > maximum_retimed_delta {
                        maximum_retimed_delta = sample_delta;
                        maximum_retimed_sample = (frame, channel, tagged_sample, expected_sample);
                    }
                }
            }

            if partition_name == "p1"
                && let (Some(preedit_full), Some(preedit_main)) = (
                    aud132_preedit_impulse(fft_size, false),
                    aud132_preedit_impulse(fft_size, true),
                )
            {
                assert_eq!(tagged_output.len(), preedit_full.len());
                assert_eq!(main_output.len(), preedit_main.len());
                let mut maximum_main_baseline_delta = 0.0_f32;
                let mut maximum_saved_baseline_delta = 0.0_f32;
                for frame in 0..tagged_output.len() / 2 {
                    for channel in 0..2 {
                        let sample = frame * 2 + channel;
                        maximum_main_baseline_delta = maximum_main_baseline_delta
                            .max((main_output[sample] - preedit_main[sample]).abs());
                        let delayed_sample = (frame + delay_frames) * 2 + channel;
                        let saved_hr = if frame >= fft_size {
                            preedit_full
                                .get(delayed_sample)
                                .zip(preedit_main.get(delayed_sample))
                                .map(|(full, main)| full - main)
                                .unwrap_or(0.0)
                        } else {
                            0.0
                        };
                        let expected = preedit_main[sample] + saved_hr;
                        maximum_saved_baseline_delta = maximum_saved_baseline_delta
                            .max((tagged_output[sample] - expected).abs());
                    }
                }
                eprintln!(
                    "AUD132 saved pre-edit N={fft_size} main_delta={maximum_main_baseline_delta:.9e} retimed_full_delta={maximum_saved_baseline_delta:.9e} full_sha_expected={}",
                    sample_digest(&preedit_full),
                );
                assert!(
                    maximum_main_baseline_delta <= 1.0e-7,
                    "main-only output changed from captured pre-edit N={fft_size}: {maximum_main_baseline_delta}"
                );
                assert!(
                    maximum_saved_baseline_delta <= 1.0e-6,
                    "live tagged output differs from the captured pre-edit N={fft_size} HR contribution shifted by D={delay_frames}: {maximum_saved_baseline_delta}"
                );
            }

            let hr_peak_samples: Vec<f32> = tagged_hr
                .as_chunks::<2>()
                .0
                .iter()
                .map(|frame| frame[0].abs().max(frame[1].abs()))
                .collect();
            let max_hr = hr_peak_samples.iter().copied().fold(0.0_f32, f32::max);
            assert!(max_hr > 1.0e-8, "silent tagged HR impulse at N={fft_size}");
            let peak = hr_peak_samples
                .iter()
                .position(|value| *value == max_hr)
                .unwrap();
            assert_eq!(peak, fft_size, "tagged HR impulse peak at N={fft_size}");
            let legacy_hr_peak = legacy_hr
                .as_chunks::<2>()
                .0
                .iter()
                .map(|frame| frame[0].abs().max(frame[1].abs()))
                .fold(0.0_f32, f32::max);
            let legacy_peak = legacy_hr
                .as_chunks::<2>()
                .0
                .iter()
                .position(|frame| frame[0].abs().max(frame[1].abs()) == legacy_hr_peak)
                .unwrap();

            let partition_delta = if let Some(reference) = &partition_reference {
                tagged_hr
                    .iter()
                    .zip(reference)
                    .map(|(actual, expected)| (actual - expected).abs())
                    .fold(0.0_f32, f32::max)
            } else {
                partition_reference = Some(tagged_hr.clone());
                0.0
            };
            let partition_delta_frame = partition_reference
                .as_ref()
                .map(|reference| {
                    tagged_hr
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .zip(reference.as_chunks::<2>().0.iter())
                        .enumerate()
                        .map(|(frame, (actual, expected))| {
                            (
                                frame,
                                (actual[0] - expected[0])
                                    .abs()
                                    .max((actual[1] - expected[1]).abs()),
                            )
                        })
                        .max_by(|left, right| left.1.total_cmp(&right.1))
                        .unwrap_or((0, 0.0))
                })
                .unwrap_or((0, 0.0));
            eprintln!(
                "AUD132 live retag impulse N={fft_size} D={delay_frames} callbacks={partition_name} peak={peak} legacy_peak={legacy_peak} legacy_hr_amplitude={legacy_hr_peak:.9} frames={} hr_amplitude={max_hr:.9} legacy_shift_array_delta={maximum_retimed_delta:.9e}@{} tagged_sample={:.9} expected_sample={:.9} partition_delta={partition_delta:.9e}@{} tagged_main_fnv={:016x} tagged_full_fnv={:016x} legacy_full_fnv={:016x}",
                tagged_output.len() / 2,
                maximum_retimed_sample.0,
                maximum_retimed_sample.2,
                maximum_retimed_sample.3,
                partition_delta_frame.0,
                sample_digest(&main_output),
                sample_digest(&tagged_output),
                sample_digest(&legacy_output),
            );
            if fft_size == 8_192 {
                let cap = tagged.safety.safety_cap_linear;
                let combined_peak = |samples: &[f32]| {
                    samples.iter().copied().map(f32::abs).fold(0.0_f32, f32::max)
                };
                let tagged_peak = combined_peak(&tagged_output);
                let legacy_peak = combined_peak(&legacy_output);
                let main_peak = combined_peak(&main_output);
                eprintln!(
                    "AUD132 N8192 safety-cap control callbacks={partition_name} cap={cap:.9} tagged_peak={tagged_peak:.9} legacy_peak={legacy_peak:.9} main_peak={main_peak:.9} tagged_scale={:.9} legacy_scale={:.9} main_scale={:.9}",
                    tagged.safety.final_safety_scale,
                    legacy.safety.final_safety_scale,
                    main_only.safety.final_safety_scale,
                );
                assert!(tagged_peak < cap, "tagged N8192 output reached safety cap");
                assert!(legacy_peak < cap, "legacy N8192 output reached safety cap");
                assert!(main_peak < cap, "main-only N8192 output reached safety cap");
                assert_eq!(tagged.safety.final_safety_scale, 1.0);
                assert_eq!(legacy.safety.final_safety_scale, 1.0);
                assert_eq!(main_only.safety.final_safety_scale, 1.0);
            }
            if fft_size <= 4_096 {
                assert!(
                    maximum_retimed_delta <= 1.0e-6,
                    "tagged output differs from the legacy HR contribution shifted by D={delay_frames} at N={fft_size}: {maximum_retimed_delta}"
                );
            } else {
                // The N8192 comparison uses reduced input and an enabled but
                // inactive safety cap so the old full-vector path remains an
                // independent sample-for-sample retiming oracle.
                assert!(
                    maximum_retimed_delta <= 1.0e-6,
                    "tagged N={fft_size} output differs from the legacy HR contribution shifted by D={delay_frames}: {maximum_retimed_delta}"
                );
                assert_eq!(legacy_peak, fft_size + delay_frames);
            }
            if partition_name == "p1" || (fft_size >= 4_096 && partition_name == "p17_137_256") {
                capture_samples(fft_size, "live_tagged", "impulse_full", &tagged_output);
                capture_samples(fft_size, "live_legacy", "impulse_full", &legacy_output);
                capture_samples(fft_size, "main_only", "impulse_full", &main_output);
            }
        }
    }
}

#[test]
fn aud132_n8192_raw_tag_zero_matches_the_legacy_hr_stream_at_delay_d() {
    const FFT_SIZE: usize = 8_192;
    const INPUT_FRAMES: usize = 8_192;
    let delay_frames = (FFT_SIZE - 512) / 2;
    let mut input = vec![0.0; INPUT_FRAMES * 2];
    input[0] = 0.25;
    input[1] = -0.125;

    let make_plugin = |legacy_route: bool| {
        let mut plugin = neutral(FFT_SIZE);
        plugin.params.enable_hr_direct = true;
        plugin.hr_state.hr_direct_envelope = 1.0;
        plugin.hr_state.hr_transient_env = 1.0;
        let gain = (FFT_SIZE as f32 / 512.0).sqrt()
            * plugin.gains.hr_sharpen.current()
            * std::f32::consts::SQRT_2
            / 512.0;
        plugin.hr_state.prev_hr_scale = gain;
        plugin.output.hr_test_gain_sequence = vec![gain; 64];
        if legacy_route {
            plugin.output.hr_use_pre_correction_mixer_for_test = true;
            plugin.hr_buffers.hr_startup_discard_remaining = plugin.fft.hr_fft_size / 2;
        }
        plugin
    };

    let mut tagged = make_plugin(false);
    let mut legacy = make_plugin(true);
    let mut tagged_output = vec![f32::NAN; INPUT_FRAMES * 2];
    let mut legacy_output = vec![f32::NAN; INPUT_FRAMES * 2];
    assert_eq!(
        tagged
            .process(
                &input,
                &mut tagged_output,
                &ProcessContext::new(48_000, INPUT_FRAMES),
            )
            .unwrap(),
        INPUT_FRAMES
    );
    assert_eq!(
        legacy
            .process(
                &input,
                &mut legacy_output,
                &ProcessContext::new(48_000, INPUT_FRAMES),
            )
            .unwrap(),
        INPUT_FRAMES
    );

    let tagged_mask = tagged.hr_buffers.hr_output_accumulator_mask;
    let legacy_mask = legacy.hr_buffers.hr_output_accumulator_mask;
    let tagged_read = tagged.hr_buffers.hr_output_read_position;
    let legacy_read = legacy.hr_buffers.hr_output_read_position;
    let tagged_tag_offset = (0..tagged.hr_buffers.hr_output_accumulator_fill)
        .find(|offset| {
            let position = (tagged_read + offset) & tagged_mask;
            tagged.hr_buffers.hr_output_source_tags[position] == 0
        })
        .expect("source tag zero was not ready after N accepted frames");
    let compare_frames = tagged.hr_buffers.hr_output_accumulator_fill - tagged_tag_offset;
    assert!(legacy.hr_buffers.hr_output_accumulator_fill >= delay_frames + compare_frames);

    let mut maximum_raw_delta = 0.0_f32;
    for offset in 0..compare_frames {
        let tagged_position = (tagged_read + tagged_tag_offset + offset) & tagged_mask;
        let legacy_position = (legacy_read + delay_frames + offset) & legacy_mask;
        assert_eq!(
            tagged.hr_buffers.hr_output_source_tags[tagged_position],
            offset as u64
        );
        let tagged_base = tagged_position * tagged.core.num_output_channels;
        let legacy_base = legacy_position * legacy.core.num_output_channels;
        for channel in 0..tagged.core.num_output_channels {
            maximum_raw_delta = maximum_raw_delta.max(
                (tagged.hr_buffers.hr_output_accumulator[tagged_base + channel]
                    - legacy.hr_buffers.hr_output_accumulator[legacy_base + channel])
                    .abs(),
            );
        }
    }
    eprintln!(
        "AUD132 raw HR queue N={FFT_SIZE} D={delay_frames} tagged_fill={} legacy_fill={} tag_zero_offset={tagged_tag_offset} compared={compare_frames} max_delta={maximum_raw_delta:.9e}",
        tagged.hr_buffers.hr_output_accumulator_fill, legacy.hr_buffers.hr_output_accumulator_fill,
    );
    assert!(
        maximum_raw_delta <= 1.0e-6,
        "source-tagged raw HR frames differ from the delayed HR stream at N={FFT_SIZE}: {maximum_raw_delta}"
    );
}

fn capture_samples(fft_size: usize, route: &str, signal: &str, samples: &[f32]) {
    let Ok(directory) = std::env::var("SOTF_AUD132_CAPTURE_DIR") else {
        return;
    };
    let directory = std::path::Path::new(&directory);
    std::fs::create_dir_all(directory).unwrap();
    let path = directory.join(format!("n{fft_size}_{route}_{signal}.f32le"));
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(samples));
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(&path, bytes).unwrap();
}

#[test]
#[ignore = "AUD132 diagnostic capture from the reviewed canonical input; no golden update"]
fn aud132_canonical_first_stage_trace() {
    let input_path = std::env::var("SOTF_AUD132_CANONICAL_INPUT")
        .expect("set SOTF_AUD132_CANONICAL_INPUT to the reviewed 32768-byte f32le fixture");
    let capture_path = std::env::var("SOTF_AUD132_CAPTURE_DIR")
        .expect("set SOTF_AUD132_CAPTURE_DIR to a fresh diagnostic directory");
    assert!(!capture_path.is_empty());
    let bytes = std::fs::read(input_path).expect("read reviewed canonical input");
    assert_eq!(bytes.len(), 4_096 * 2 * 4);
    let input: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|sample| f32::from_le_bytes(sample.try_into().unwrap()))
        .collect();
    assert!(input.iter().all(|sample| sample.is_finite()));

    for fft_size in [2_usize, 256, 512] {
        let first_block = &input[..fft_size * 2];
        let mut analysis = neutral(fft_size);
        capture_samples(fft_size, "canonical_stage", "window", &analysis.main_buffers.window);
        capture_samples(fft_size, "canonical_stage", "first_input", first_block);
        let mut windowed_left = vec![0.0_f32; fft_size];
        let mut windowed_right = vec![0.0_f32; fft_size];
        sotf_host::simd::deinterleave_stereo(
            first_block,
            &mut windowed_left,
            &mut windowed_right,
        );
        for channel in [&mut windowed_left, &mut windowed_right] {
            sotf_host::simd::window_mul_simd_inplace(channel, &analysis.main_buffers.window);
            sotf_host::simd::scale_add_simd_inplace(
                channel,
                std::f32::consts::FRAC_1_SQRT_2,
            );
        }
        capture_samples(fft_size, "canonical_stage", "reconstructed_windowed_left", &windowed_left);
        capture_samples(fft_size, "canonical_stage", "reconstructed_windowed_right", &windowed_right);
        analysis.apply_window_and_forward_fft(first_block);
        let left_spectrum: Vec<f32> = analysis
            .main_buffers
            .freq_domain_left
            .iter()
            .flat_map(|bin| [bin.re, bin.im])
            .collect();
        let right_spectrum: Vec<f32> = analysis
            .main_buffers
            .freq_domain_right
            .iter()
            .flat_map(|bin| [bin.re, bin.im])
            .collect();
        capture_samples(fft_size, "canonical_stage", "fft_left_complex", &left_spectrum);
        capture_samples(fft_size, "canonical_stage", "fft_right_complex", &right_spectrum);

        let mut synthesis = neutral(fft_size);
        let mut block = vec![0.0_f32; fft_size * synthesis.output_channels()];
        synthesis.process_fft_block(first_block, &mut block);
        for (channel, samples) in synthesis.main_buffers.time_out_channels.iter().enumerate() {
            capture_samples(
                fft_size,
                "canonical_stage",
                &format!("inverse_windowed_channel_{channel}"),
                samples,
            );
        }
        capture_samples(fft_size, "canonical_stage", "first_block_output", &block);

        let mut whole = neutral(fft_size);
        let mut output = render(&mut whole, &input, &[17, 137, 256]);
        output.extend(finish_with_capacity(&mut whole, 512));
        assert!(output.iter().all(|sample| sample.is_finite()));
        capture_samples(fft_size, "canonical_stage", "full_output", &output);
    }
}

#[test]
fn aud132_preserves_small_fft_and_512_pre_edit_full_output_controls() {
    const INPUT_FRAMES: usize = 4_096;

    let input: Vec<f32> = (0..INPUT_FRAMES)
        .flat_map(|frame| {
            let level = match frame / 512 % 4 {
                0 => 0.02,
                1 => 0.2,
                2 => 0.003,
                _ => 0.1,
            };
            let phase = std::f32::consts::TAU * 997.0 * frame as f32 / 48_000.0;
            let pulse = if frame == 0 || frame == 2_337 {
                0.25
            } else {
                0.0
            };
            [
                level * phase.sin() + pulse,
                -0.5 * level * phase.cos() - pulse / 2.0,
            ]
        })
        .collect();

    for fft_size in [2_usize, 256, 512] {
        for hr_enabled in [false, true] {
            let mut plugin = neutral(fft_size);
            plugin.params.enable_hr_direct = hr_enabled;
            plugin.hr_state.hr_direct_envelope = if hr_enabled { 1.0 } else { 0.0 };
            plugin.hr_state.hr_transient_env = 1.0;
            let gain = (fft_size as f32 / plugin.fft.hr_fft_size as f32).sqrt()
                * plugin.gains.hr_sharpen.current()
                * std::f32::consts::SQRT_2
                / plugin.fft.hr_fft_size as f32;
            plugin.hr_state.prev_hr_scale = if hr_enabled { gain } else { 0.0 };
            plugin.output.hr_test_gain_sequence = vec![if hr_enabled { gain } else { 0.0 }; 128];

            let mut output = render(&mut plugin, &input, &[17, 137, 256]);
            output.extend(finish_with_capacity(&mut plugin, 512));
            assert!(output.iter().all(|sample| sample.is_finite()));
            assert_eq!(plugin.latency_samples(), fft_size.max(512));

            let route = if hr_enabled {
                "aud132_pre_edit_hr_on"
            } else {
                "aud132_pre_edit_hr_off"
            };
            capture_samples(fft_size, route, "input", &input);
            capture_samples(fft_size, route, "full", &output);
            let digest = sample_digest(&output);
            let expected_digest = match (fft_size, hr_enabled) {
                (2, false) => 0x02dd_c527_02cb_7e9f,
                (2, true) => 0x5605_bd82_1b48_57ee,
                (256, false) => 0xb106_d5e5_45f3_b14f,
                (256, true) => 0x7aaf_f37f_8e9d_2070,
                (512, false) => 0xb6b9_e4ea_a0d6_739b,
                (512, true) => 0x941f_f0e4_37b3_e7f3,
                _ => unreachable!("unexpected AUD132 control size"),
            };
            eprintln!(
                "AUD132 pre-edit control N={fft_size} HR={hr_enabled} frames={} latency={} digest={digest:016x}",
                output.len() / plugin.output_channels(),
                plugin.latency_samples(),
            );
            assert_eq!(
                digest, expected_digest,
                "captured pre-edit control changed at N={fft_size}, HR={hr_enabled}"
            );
        }
    }
}

#[test]
fn hr_gain_ramp_is_scheduled_by_sample_before_future_analysis() {
    let n = 256;
    let hop = n / 2;
    for chunk in [1, 17, 256] {
        let mut plugin = neutral(n);
        plugin.hr_state.hr_direct_envelope = 1.0;
        plugin.hr_state.hr_transient_env = 0.75;
        plugin.gains.hr_sharpen = sotf_host::smoothing::Smoother::new(1.0, 5.0, 48_000);
        plugin.hr_state.prev_hr_scale = 0.0;
        plugin.output.next_add_position = 0;
        plugin.output.main_next_add_source_frame = 0;
        plugin.prepare_hr_output_gains();
        // Later analysis may already be available when an old hop is read.
        // It must not change that hop's scheduled gain.
        plugin.output.next_add_position = hop;
        plugin.hr_state.hr_transient_env = 0.25;
        plugin.prepare_hr_output_gains();
        plugin.hr_buffers.hr_output_accumulator.fill(1.0);
        plugin.hr_buffers.hr_output_accumulator_fill = hop;
        plugin.hr_buffers.hr_output_read_position = 0;
        for frame in 0..hop {
            plugin.hr_buffers.hr_output_source_tags[frame] = frame as u64;
        }
        let mut actual = vec![0.0; hop * 2];
        let mut frame = 0;
        while frame < hop {
            let count = chunk.min(hop - frame);
            plugin.mix_hr_output_source_aligned(
                &mut actual[frame * 2..(frame + count) * 2],
                frame,
                frame as u64,
                count,
                2,
                false,
            );
            frame += count;
        }
        let target = (n as f64 / 512.0).sqrt() * std::f64::consts::SQRT_2 * 0.75 / 512.0;
        for frame in 0..hop {
            let expected = target * (frame + 1) as f64 / hop as f64;
            for channel in 0..2 {
                assert!((f64::from(actual[frame * 2 + channel]) - expected).abs() < 1.0e-9);
            }
        }
    }
}

#[test]
fn sub_512_hr_mix_uses_gain_for_matching_source_frame() {
    for fft_size in [2, 32, 64, 128, 256] {
        let mut plugin = neutral(fft_size);
        let hop = plugin.core.hop_size;
        let hr_position = 0;
        let source_frame = 0;
        let source_gain_position = hop;
        plugin.hr_state.hr_direct_envelope = 1.0;
        plugin.hr_state.hr_transient_env = 0.25;
        plugin.hr_state.prev_hr_scale = 0.0;
        plugin.output.main_next_add_source_frame = 0;
        plugin.output.next_add_position = source_gain_position;
        plugin.prepare_hr_output_gains();
        let source_gain = plugin.output.hr_mix_gains[source_gain_position];

        plugin.hr_state.hr_transient_env = 1.0;
        plugin.output.main_next_add_source_frame = hop as i64;
        plugin.output.next_add_position = 2 * hop;
        plugin.prepare_hr_output_gains();
        let future_gain = plugin.output.hr_mix_gains[2 * hop];

        let channels = plugin.output_channels();
        plugin.hr_buffers.hr_output_accumulator.fill(0.0);
        for &channel in &plugin.panning.cached_hr_active_channels {
            plugin.hr_buffers.hr_output_accumulator[hr_position * channels + channel] = 1.0;
        }
        plugin.hr_buffers.hr_output_source_tags[hr_position] = source_frame;
        plugin.hr_buffers.hr_output_accumulator_fill = 1;
        let mut mixed = vec![0.0; channels];
        plugin.mix_hr_output_source_aligned(
            &mut mixed,
            source_gain_position,
            source_frame,
            1,
            channels,
            false,
        );
        let observed_gain = mixed[0];

        eprintln!(
            "source-tag gain N={fft_size}: HR source={source_frame} uses main ring index={source_gain_position} gain={source_gain:.9}; future source gain={future_gain:.9}; observed={observed_gain:.9}"
        );
        assert!((observed_gain - source_gain).abs() < 1.0e-7);
        assert!(
            (future_gain - source_gain).abs() > 1.0e-7,
            "fixture did not distinguish adjacent source gain frames at N={fft_size}"
        );
    }
}

fn finish_with_capacity(plugin: &mut UpmixerPlugin, capacity: usize) -> Vec<f32> {
    finish_with_capacities(plugin, &[capacity])
}

fn finish_with_capacities(plugin: &mut UpmixerPlugin, capacities: &[usize]) -> Vec<f32> {
    let channels = plugin.output_channels();
    let capacity = *capacities.iter().max().unwrap();
    let mut output = vec![f32::NAN; capacity * channels];
    let mut rendered = Vec::new();
    for step in 0..1_000_000 {
        let capacity = capacities[step % capacities.len()];
        output.fill(f32::NAN);
        let result = plugin
            .drain(
                &mut output[..capacity * channels],
                &ProcessContext::new(48_000, capacity),
            )
            .unwrap();
        assert!(result.frames <= capacity && result.frames <= plugin.drain_output_frames_max());
        assert!(
            output[result.frames * channels..]
                .iter()
                .all(|x| x.is_nan())
        );
        rendered.extend_from_slice(&output[..result.frames * channels]);
        if result.complete {
            assert_eq!(
                plugin
                    .drain(&mut output, &ProcessContext::new(48_000, capacity))
                    .unwrap(),
                sotf_host::plugin::PluginDrainResult::COMPLETE
            );
            return rendered;
        }
        assert!(result.frames > 0);
    }
    panic!("upmixer drain did not complete");
}

fn expected_complete_stream_frames(plugin: &UpmixerPlugin, input_frames: usize) -> usize {
    let latency = plugin.latency_samples();
    let input_phase = input_frames % latency;
    let main_hop = plugin.core.hop_size;
    let main_padding = (main_hop - input_phase % main_hop) % main_hop;
    let mut tail = latency + plugin.core.fft_size - main_hop + main_padding;

    if plugin.params.enable_hr_direct || plugin.hr_state.hr_direct_envelope > 0.0 {
        let hr_fft_size = plugin.fft.hr_fft_size;
        let hr_hop = hr_fft_size / 2;
        let delay = plugin.hr_buffers.hr_delay_buffer.len() / 2;
        let hr_padding = (hr_hop - (input_phase + delay) % hr_hop) % hr_hop;
        tail = tail.max(latency + delay + hr_fft_size - hr_hop + hr_padding);
    }

    input_frames + tail
}

#[test]
fn small_fft_multi_second_callback_preserves_identity_tail_and_eos() {
    const INPUT_FRAMES: usize = 96_017;
    let input: Vec<f32> = (0..INPUT_FRAMES)
        .flat_map(|frame| {
            let phase = std::f32::consts::TAU * frame as f32 / 7.0;
            [0.03 * phase.sin(), -0.02 * phase.cos()]
        })
        .collect();

    let mut plugin = neutral(2);
    let latency = plugin.latency_samples();
    let mut rendered = render(&mut plugin, &input, &[INPUT_FRAMES]);
    assert_eq!(rendered.len(), input.len());
    rendered.extend(finish_with_capacity(&mut plugin, 17));

    let expected_frames = expected_complete_stream_frames(&plugin, INPUT_FRAMES);
    assert_eq!(rendered.len() / plugin.output_channels(), expected_frames);
    assert!(rendered.iter().all(|sample| sample.is_finite()));
    for frame in 0..expected_frames {
        for channel in 0..2 {
            let expected = if frame >= latency && frame < latency + INPUT_FRAMES {
                input[(frame - latency) * 2 + channel]
            } else {
                0.0
            };
            let actual = rendered[frame * 2 + channel];
            assert!(
                (actual - expected).abs() < 2.0e-6,
                "multi-second N=2 stream differs at frame {frame}, channel {channel}: {actual} vs {expected}"
            );
        }
    }
}

#[test]
fn small_fft_hr_autogain_and_surround_height_routes_preserve_frames() {
    const INPUT_FRAMES: usize = 6_000;
    let input: Vec<f32> = (0..INPUT_FRAMES)
        .flat_map(|frame| {
            let level = [0.02, 0.2, 0.003][frame / 2_000];
            let phase = std::f32::consts::TAU * 997.0 * frame as f32 / 48_000.0;
            let sample = level * phase.sin();
            [sample, -0.5 * sample]
        })
        .collect();

    for fft_size in [2, 256] {
        for speaker_config in ["5.1", "7.1.4"] {
            for hr_enabled in [false, true] {
                for auto_gain_enabled in [false, true] {
                    let params: UpmixerPluginParams = serde_json::from_value(serde_json::json!({
                        "fft_size": fft_size,
                        "speaker_config": speaker_config,
                        "gain_front_direct": 1.0,
                        "gain_front_ambient": 0.0,
                        "gain_rear_ambient": 0.0,
                        "stereo_width": 0.0,
                        "enable_hr_direct": hr_enabled,
                        "enable_subharmonic_synth": false,
                        "bypass_decorrelation": true,
                        "auto_gain_enabled": auto_gain_enabled,
                        "auto_gain_smoothing_ms": 100.0,
                        "safety_cap_db": -1.0,
                        "multi_source_extraction": false
                    }))
                    .unwrap();
                    let mut plugin = UpmixerPlugin::from_params(params);
                    plugin.initialize(48_000.0).unwrap();
                    assert_eq!(
                        plugin.safety.auto_gain.as_ref().unwrap().is_enabled(),
                        auto_gain_enabled
                    );
                    if hr_enabled {
                        plugin.hr_state.hr_direct_envelope = 1.0;
                        plugin.hr_state.hr_transient_env = 1.0;
                        plugin.hr_state.spectral_flux_smooth = 1.0e-6;
                    }

                    let channels = plugin.output_channels();
                    let mut rendered = render(&mut plugin, &input, &[137, 512, 17, 2_048]);
                    assert_eq!(rendered.len(), INPUT_FRAMES * channels);
                    assert!(rendered.iter().all(|sample| sample.is_finite()));
                    let expected_frames = expected_complete_stream_frames(&plugin, INPUT_FRAMES);
                    rendered.extend(finish_with_capacity(&mut plugin, 257));
                    assert_eq!(
                        rendered.len() / channels,
                        expected_frames,
                        "N={fft_size}, speakers={speaker_config}, HR={hr_enabled}, AutoGain={auto_gain_enabled}"
                    );
                    assert_eq!(
                        plugin.safety.auto_gain_reference_position,
                        expected_frames % (plugin.safety.auto_gain_reference.len() / 2)
                    );
                    if auto_gain_enabled {
                        let data = plugin.safety.auto_gain.as_ref().unwrap().data();
                        assert!(data.gain_db.is_finite());
                        assert!(data.input_lufs.is_finite());
                    }
                }
            }
        }
    }
}

#[test]
fn finite_drain_retains_first_and_final_impulses() {
    let n = 512;
    let hop = n / 2;
    for frames in [1, 255, 256, 257, 511, 512, 513] {
        let mut input = vec![0.0; frames * 2];
        input[0] = 0.25;
        input[(frames - 1) * 2 + 1] = -0.125;
        let end = n * 2 + ((frames - 1) / hop) * hop;
        let mut padded = input.clone();
        padded.resize(end * 2, 0.0);
        let reference = render(&mut neutral(n), &padded, &[17, 3, 97]);
        for capacity in [1, 13, 4096] {
            let mut plugin = neutral(n);
            let mut actual = render(&mut plugin, &input, &[1, 127, 3]);
            actual.extend(finish_with_capacity(&mut plugin, capacity));
            assert_eq!(actual.len(), end * 2, "stream frames={frames}");
            assert!(
                actual
                    .iter()
                    .zip(&reference)
                    .all(|(a, b)| (a - b).abs() < 2.0e-6)
            );
            for (index, &value) in actual.iter().enumerate() {
                let expected = index
                    .checked_sub(n * 2)
                    .and_then(|i| input.get(i))
                    .copied()
                    .unwrap_or(0.0);
                assert!((value - expected).abs() < 2.0e-6);
            }
        }
    }
}

#[test]
fn spatial_and_recursive_drain_match_fixed_hop_zero_continuation() {
    for (n, preview, release_ms) in [
        (512_usize, false, None),
        (1024, true, None),
        (512, false, Some(20.0_f32)),
        (1024, true, Some(50.0)),
    ] {
        let make = || {
            let params = serde_json::from_value(serde_json::json!({
                "fft_size": n, "speaker_config": "5.1", "binaural_preview": preview,
                "enable_subharmonic_synth": release_ms.is_some(),
                "subharmonic_release_ms": release_ms.unwrap_or(100.0),
                "auto_gain_enabled": true
            }))
            .unwrap();
            let mut plugin = UpmixerPlugin::from_params(params);
            plugin.initialize(48_000.0).unwrap();
            plugin
        };
        let frames = n * 4 + 17;
        let mut input = vec![0.0; frames * 2];
        for frame in 0..frames {
            input[frame * 2] = (std::f32::consts::TAU * 60.0 * frame as f32 / 48_000.0).sin() * 0.3;
            input[frame * 2 + 1] = input[frame * 2] * 0.8;
        }
        input[0] = 0.25;
        input[frames * 2 - 1] = -0.2;
        let mut baseline = Vec::new();
        for capacities in [&[1][..], &[13, 1, 4096][..], &[4096][..]] {
            let mut plugin = make();
            let channels = plugin.output_channels();
            let mut actual = render(&mut plugin, &input, &[17, 3, 97]);
            let tail = finish_with_capacities(&mut plugin, capacities);
            actual.extend_from_slice(&tail);
            if baseline.is_empty() {
                baseline.clone_from(&actual);
            }
            assert!(
                actual == baseline,
                "drain destination capacity changed output"
            );

            let main_end = n * 2 + ((frames - 1) / (n / 2)) * (n / 2);
            let delay = (n / 2).saturating_sub(256);
            let hr_end = n + ((frames + delay - 1) / 256) * 256 + 512;
            let finite_end = main_end.max(hr_end);
            let total_frames = actual.len() / channels;
            if let Some(release_ms) = release_ms {
                // Bound f32 exp/subtraction quantization independently from
                // the runtime coefficient. One ulp at unity bounds its error.
                let tau = f64::from(release_ms) * 48.0;
                let alpha = -(-1.0 / tau).exp_m1();
                let rounding = 2.0_f64.powi(-23);
                let min_steps = (14.0 / -(1.0 - alpha - rounding).ln()).ceil() as usize;
                let max_steps = (14.0 / -(1.0 - alpha + rounding).ln()).ceil() as usize;
                assert!(
                    (finite_end + n + min_steps..=finite_end + n + max_steps)
                        .contains(&total_frames)
                );
                assert_eq!(plugin.subharmonic.subharmonic_envelope, 0.0);
                assert_eq!(plugin.subharmonic.subharmonic_amp_envelope, 0.0);
            } else {
                assert_eq!(total_frames, finite_end);
            }
            let mut reference_plugin = make();
            let mut reference = render(&mut reference_plugin, &input, &[17, 3, 97]);
            reference.extend(render(
                &mut reference_plugin,
                &vec![0.0; (total_frames - frames) * 2],
                &[n / 2],
            ));
            assert!(
                actual == reference,
                "bounded tail differs from independent zero continuation"
            );
            assert!(tail.iter().any(|x| x.abs() > 1.0e-5));
        }
    }
}

#[test]
fn drain_errors_lifecycle_empty_bypass_and_reset_are_explicit() {
    let mut plugin = neutral(512);
    let mut reference = neutral(512);
    let context = ProcessContext::new(48_000, 1);
    assert_eq!(
        plugin.drain(&mut [], &context).unwrap(),
        sotf_host::plugin::PluginDrainResult::COMPLETE
    );
    assert_eq!(
        plugin
            .process(&[], &mut [], &ProcessContext::new(48_000, 0))
            .unwrap(),
        0
    );
    render(&mut plugin, &[0.2, -0.1], &[1]);
    render(&mut reference, &[0.2, -0.1], &[1]);
    for _ in 0..3 {
        for capacity in [0, 1, 3] {
            let mut invalid = vec![9.0; capacity];
            assert!(plugin.drain(&mut invalid, &context).is_err());
            assert!(invalid.iter().all(|&x| x == 9.0));
        }
        let mut actual = [0.0; 2];
        let mut expected = [0.0; 2];
        let a = plugin.drain(&mut actual, &context).unwrap();
        let b = reference.drain(&mut expected, &context).unwrap();
        assert_eq!(a, b);
        assert_eq!(actual, expected);
        assert_eq!(a.frames, 1);
        assert!(!a.complete);
    }
    let mut unchanged = [9.0; 2];
    assert!(
        plugin
            .process(&[0.1, 0.2], &mut unchanged, &context)
            .is_err()
    );
    assert_eq!(unchanged, [9.0; 2]);
    let id = sotf_host::ParameterId::from("subharmonic_release_ms");
    let before = plugin.get_parameter(&id);
    assert!(
        plugin
            .set_parameter(id.clone(), sotf_host::ParameterValue::Float(500.0))
            .is_err()
    );
    assert_eq!(plugin.get_parameter(&id), before);
    assert!(finish_with_capacity(&mut plugin, 1) == finish_with_capacity(&mut reference, 1024));
    assert!(
        plugin
            .process(&[0.1, 0.2], &mut unchanged, &context)
            .is_err()
    );
    plugin.reset();
    render(&mut plugin, &[0.2, -0.1], &[1]);
    let actual = finish_with_capacity(&mut plugin, 17);
    let mut fresh = neutral(512);
    render(&mut fresh, &[0.2, -0.1], &[1]);
    assert!(actual == finish_with_capacity(&mut fresh, 1024));
    plugin.reset();
    plugin
        .set_parameter(
            sotf_host::ParameterId::from("bypass_all_processing"),
            sotf_host::ParameterValue::Bool(true),
        )
        .unwrap();
    assert_eq!(render(&mut plugin, &[0.2, -0.1], &[1]), vec![0.2, -0.1]);
    assert_eq!(
        plugin.drain(&mut [], &context).unwrap(),
        sotf_host::plugin::PluginDrainResult::COMPLETE
    );
}
