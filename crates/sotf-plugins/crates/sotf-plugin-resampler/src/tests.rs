#![allow(clippy::needless_range_loop)]
use super::cutoff_bank::CutoffBank;
use super::resampler_plugin::ResamplerPlugin;
use super::resampler_quality::ResamplerQuality;
use rubato::Resampler;
use sotf_host::PluginHost;
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_host::test_utils::assert_no_allocs_or_deallocs;

#[test]
fn test_resampler_creation() {
    let resampler = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();
    assert_eq!(resampler.input_channels(), 2);
    assert_eq!(resampler.output_channels(), 2);
    assert!(resampler.ratio() > 1.0); // Upsampling
}

#[test]
fn test_resampler_44100_to_48000() {
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();
    resampler.initialize(44100).unwrap();

    // Create test signal: 1kHz sine wave at 44.1kHz
    let num_frames = 1024;
    let mut input = vec![0.0_f32; num_frames * 2];
    for i in 0..num_frames {
        let phase = 2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 44100.0;
        let sample = phase.sin() * 0.5;
        input[i * 2] = sample;
        input[i * 2 + 1] = sample;
    }

    // Calculate maximum output buffer size (conservative)
    let max_output_frames = resampler.output_frames_for_input(num_frames);
    let mut output = vec![0.0_f32; max_output_frames * 2];

    let context = ProcessContext::new(44100, num_frames);

    // Process
    resampler.process(&input, &mut output, &context).unwrap();

    log::info!("Input frames: {}", num_frames);
    log::info!("Max output frames (buffer size): {}", max_output_frames);
    log::info!("Expected ratio: {:.4}", 48000.0 / 44100.0);

    // Check that output contains signal (actual frames may be less than max)
    // We check the first portion of the output buffer
    let expected_frames = (num_frames as f64 * 48000.0 / 44100.0) as usize;
    let check_samples = expected_frames * 2;
    let rms: f32 =
        output[..check_samples].iter().map(|x| x * x).sum::<f32>() / check_samples as f32;
    let rms = rms.sqrt();
    log::info!("Output RMS (first {} frames): {:.4}", expected_frames, rms);
    assert!(rms > 0.1, "Output should contain signal");
}

#[test]
fn test_output_frame_estimate_covers_multi_chunk_input() {
    let chunk_size = 1024;
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, chunk_size).unwrap();
    resampler.initialize(44100).unwrap();

    let num_frames = chunk_size * 3;
    let input = vec![0.25_f32; num_frames * 2];
    let max_output_frames = resampler.output_frames_for_input(num_frames);
    let mut output = vec![0.0_f32; max_output_frames * 2];
    let context = ProcessContext::new(44100, num_frames);

    let produced = resampler.process(&input, &mut output, &context).unwrap();

    assert!(
        produced <= max_output_frames,
        "produced {produced} frames, estimate only allowed {max_output_frames}"
    );
    assert!(produced > chunk_size);
}

#[test]
fn test_resampler_48000_to_44100() {
    let mut resampler = ResamplerPlugin::new(2, 48000, 44100, 1024).unwrap();
    resampler.initialize(48000).unwrap();

    // Create test signal at 48kHz
    let num_frames = 1024;
    let mut input = vec![0.0_f32; num_frames * 2];
    for i in 0..num_frames {
        let phase = 2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48000.0;
        let sample = phase.sin() * 0.5;
        input[i * 2] = sample;
        input[i * 2 + 1] = sample;
    }

    let max_output_frames = resampler.output_frames_for_input(num_frames);
    let mut output = vec![0.0_f32; max_output_frames * 2];

    let context = ProcessContext::new(48000, num_frames);

    resampler.process(&input, &mut output, &context).unwrap();

    log::info!("Input frames: {}", num_frames);
    log::info!("Max output frames (buffer size): {}", max_output_frames);
    log::info!("Expected ratio: {:.4}", 44100.0 / 48000.0);

    // Check signal (actual frames may be less than max buffer)
    let expected_frames = (num_frames as f64 * 44100.0 / 48000.0) as usize;
    let check_samples = expected_frames * 2;
    let rms: f32 =
        output[..check_samples].iter().map(|x| x * x).sum::<f32>() / check_samples as f32;
    let rms = rms.sqrt();
    log::info!("Output RMS (first {} frames): {:.4}", expected_frames, rms);
    assert!(rms > 0.1);
}

#[test]
fn test_resampler_multichannel() {
    // Test with 5 channels (5.0 surround)
    let mut resampler = ResamplerPlugin::new(5, 44100, 48000, 1024).unwrap();
    resampler.initialize(44100).unwrap();

    let num_frames = 1024;
    let mut input = vec![0.0_f32; num_frames * 5];

    // Different frequency on each channel
    for i in 0..num_frames {
        let t = i as f32 / 44100.0;
        input[i * 5] = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.2; // FL
        input[i * 5 + 1] = (2.0 * std::f32::consts::PI * 550.0 * t).sin() * 0.2; // FR
        input[i * 5 + 2] = (2.0 * std::f32::consts::PI * 660.0 * t).sin() * 0.2; // C
        input[i * 5 + 3] = (2.0 * std::f32::consts::PI * 220.0 * t).sin() * 0.2; // RL
        input[i * 5 + 4] = (2.0 * std::f32::consts::PI * 330.0 * t).sin() * 0.2;
        // RR
    }

    let max_output_frames = resampler.output_frames_for_input(num_frames);
    let mut output = vec![0.0_f32; max_output_frames * 5];

    let context = ProcessContext::new(44100, num_frames);

    resampler.process(&input, &mut output, &context).unwrap();

    log::info!(
        "5-channel resampling: {} input frames, {} max output frames",
        num_frames,
        max_output_frames
    );

    // Check each channel has signal (check expected number of frames)
    let expected_frames = (num_frames as f64 * 48000.0 / 44100.0) as usize;
    for ch in 0..5 {
        let channel_samples: Vec<f32> = (0..expected_frames).map(|i| output[i * 5 + ch]).collect();
        let rms: f32 =
            channel_samples.iter().map(|x| x * x).sum::<f32>() / channel_samples.len() as f32;
        let rms = rms.sqrt();
        log::info!("Channel {} RMS: {:.4}", ch, rms);
        assert!(rms > 0.05, "Channel {} should have signal", ch);
    }
}

#[test]
fn test_resampler_reset() {
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();
    resampler.initialize(44100).unwrap();

    let num_frames = 1024;
    let input = vec![0.5_f32; num_frames * 2];
    let output_frames = resampler.output_frames_for_input(num_frames);
    let mut output = vec![0.0_f32; output_frames * 2];

    let context = ProcessContext::new(44100, num_frames);

    // Process
    resampler.process(&input, &mut output, &context).unwrap();

    // Reset
    resampler.reset();

    // Process again - should work
    resampler.process(&input, &mut output, &context).unwrap();

    // Should still have output
    let rms: f32 = output.iter().map(|x| x * x).sum::<f32>() / output.len() as f32;
    assert!(rms.sqrt() > 0.1);
}

/// After reset(), processing silence should produce silence (no residual
/// from previously processed audio leaking through).
#[test]
fn test_reset_clears_residual() {
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();
    resampler.initialize(44100).unwrap();

    let num_frames = 1024;
    let ctx = ProcessContext::new(44100, num_frames);

    // Process a loud signal
    let loud_input: Vec<f32> = (0..num_frames * 2)
        .map(|i| 0.9 * (2.0 * std::f32::consts::PI * 1000.0 * (i / 2) as f32 / 44100.0).sin())
        .collect();
    let max_output = resampler.output_frames_for_input(num_frames);
    let mut output = vec![0.0_f32; max_output * 2];
    resampler.process(&loud_input, &mut output, &ctx).unwrap();

    // Reset
    resampler.reset();

    // Process silence
    let silence = vec![0.0_f32; num_frames * 2];
    let mut output2 = vec![0.0_f32; max_output * 2];
    resampler.process(&silence, &mut output2, &ctx).unwrap();

    // After reset + processing silence, output RMS should be very low
    let rms: f32 = (output2.iter().map(|x| x * x).sum::<f32>() / output2.len() as f32).sqrt();
    assert!(
        rms < 0.01,
        "After reset and processing silence, output should be near-silent, \
             but RMS={rms:.6}"
    );
}

#[test]
fn test_quality_presets() {
    // Fast
    let r_fast =
        ResamplerPlugin::with_quality(2, 44100, 48000, 1024, ResamplerQuality::Fast).unwrap();
    assert_eq!(r_fast.quality(), ResamplerQuality::Fast);
    // latency_samples() now returns rubato's output_delay(), which includes the full FIR
    // group delay and internal offsets — not the old sinc_len/2 heuristic.
    // For Fast (64-tap sinc), the real delay is > 0.
    assert!(r_fast.latency_samples() > 0);
    // Latency = rubato delay + chunk_size - 1.  For Fast quality the rubato
    // delay is small, but chunk_size (1024) dominates.
    assert!(r_fast.latency_samples() >= 1024);

    // Medium
    let r_med =
        ResamplerPlugin::with_quality(2, 44100, 48000, 1024, ResamplerQuality::Medium).unwrap();
    assert_eq!(r_med.quality(), ResamplerQuality::Medium);
    assert!(r_med.latency_samples() > 0);
    assert!(r_med.latency_samples() >= 1024);

    // High
    let r_high =
        ResamplerPlugin::with_quality(2, 44100, 48000, 1024, ResamplerQuality::High).unwrap();
    assert_eq!(r_high.quality(), ResamplerQuality::High);
    assert!(r_high.latency_samples() > 0);
    // Latency = rubato delay + chunk_size - 1.  All qualities share the same
    // chunk_size, so the base latency is similar; only the rubato delay differs.
    assert!(r_high.latency_samples() >= 1024);

    // Higher quality → slightly higher rubato delay (all have same chunk_size)
    assert!(
        r_high.latency_samples() >= r_fast.latency_samples(),
        "High quality should have at least as much latency as Fast: {} vs {}",
        r_high.latency_samples(),
        r_fast.latency_samples()
    );
}

#[test]
fn test_latency_uses_rubato_output_delay() {
    // Verify that latency_samples() returns rubato's output_delay(), not the old
    // sinc_len/2 heuristic.  For Medium quality (128-tap), the heuristic would be 64;
    // rubato accounts for ratio scaling so the values must differ.
    let resampler =
        ResamplerPlugin::with_quality(2, 44100, 48000, 1024, ResamplerQuality::Medium).unwrap();
    let reported = resampler.latency_samples();
    let old_heuristic = 128 / 2; // sinc_len / 2
    // The rubato delay for upsampling (ratio > 1) is scaled by the ratio, so it is
    // larger than sinc_len/2.  Assert that we are NOT returning the old heuristic.
    assert_ne!(
        reported, old_heuristic,
        "latency_samples() should not return the old sinc_len/2 heuristic ({old_heuristic}), \
             got {reported}"
    );
    // And it must be a positive number.
    assert!(reported > 0, "latency must be > 0, got {reported}");
}

#[test]
fn test_quality_parameter_change() {
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();
    assert_eq!(resampler.quality(), ResamplerQuality::Medium);

    // Switch to fast
    resampler
        .set_parameter(
            ParameterId::from("quality"),
            ParameterValue::String("fast".to_string()),
        )
        .unwrap();
    assert_eq!(resampler.quality(), ResamplerQuality::Fast);

    // Switch to high
    resampler
        .set_parameter(
            ParameterId::from("quality"),
            ParameterValue::String("high".to_string()),
        )
        .unwrap();
    assert_eq!(resampler.quality(), ResamplerQuality::High);

    // Invalid quality should fail
    assert!(
        resampler
            .set_parameter(
                ParameterId::from("quality"),
                ParameterValue::String("ultra".to_string()),
            )
            .is_err()
    );
    resampler.initialize(44100).unwrap();
    assert!(
        resampler
            .set_parameter(ParameterId::from("quality"), ParameterValue::Int(1))
            .is_err(),
        "quality changes after activation must require a structural rebuild"
    );
}

#[test]
fn test_quality_affects_processing() {
    // Verify that bridge-visible quality is not cosmetic: presets build
    // measurably different FIR responses.
    let num_frames = 1024;
    let mut input = vec![0.0_f32; num_frames * 2];
    for i in 0..num_frames {
        let phase = 2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 44100.0;
        let sample = phase.sin() * 0.5;
        input[i * 2] = sample;
        input[i * 2 + 1] = sample;
    }

    let mut renders = Vec::new();
    for quality in [
        ResamplerQuality::Fast,
        ResamplerQuality::Medium,
        ResamplerQuality::High,
    ] {
        let mut resampler = ResamplerPlugin::with_quality(2, 44100, 48000, 1024, quality).unwrap();
        resampler.initialize(44100).unwrap();

        let max_output = resampler.output_frames_for_input(num_frames);
        let mut output = vec![0.0_f32; max_output * 2];

        let context = ProcessContext::new(44100, num_frames);
        resampler.process(&input, &mut output, &context).unwrap();

        let expected_frames = (num_frames as f64 * 48000.0 / 44100.0) as usize;
        let check_samples = expected_frames * 2;
        let rms: f32 =
            output[..check_samples].iter().map(|x| x * x).sum::<f32>() / check_samples as f32;
        assert!(
            rms.sqrt() > 0.1,
            "Quality {:?} should produce valid output",
            quality
        );
        renders.push(output[..check_samples].to_vec());
    }
    assert!(
        renders[0]
            .iter()
            .zip(&renders[2])
            .any(|(fast, high)| (fast - high).abs() > 1.0e-6),
        "Fast and High must produce different filter responses"
    );
}

#[test]
fn test_dynamic_ratio() {
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();
    resampler.initialize(44100).unwrap();

    // Dynamic ratio should be disabled by default
    assert!(!resampler.is_dynamic_ratio());

    // Setting ratio should fail when disabled
    assert!(resampler.set_ratio(1.1, true).is_err());

    // Enable dynamic ratio
    resampler
        .set_parameter(
            ParameterId::from("dynamic_ratio"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    assert!(resampler.is_dynamic_ratio());

    // Now setting ratio should succeed
    let nominal = resampler.ratio();
    let new_ratio = nominal * 1.01;
    resampler.set_ratio(new_ratio, true).unwrap();
    assert!((resampler.current_ratio() - new_ratio).abs() < 1e-10);

    // Process should still work
    let num_frames = 1024;
    let input = vec![0.5_f32; num_frames * 2];
    let max_output = resampler.output_frames_for_input(num_frames);
    let mut output = vec![0.0_f32; max_output * 2];
    let context = ProcessContext::new(44100, num_frames);
    resampler.process(&input, &mut output, &context).unwrap();
}

#[test]
fn test_dynamic_ratio_relative() {
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();
    resampler.initialize(44100).unwrap();

    // Enable dynamic ratio
    resampler
        .set_parameter(
            ParameterId::from("dynamic_ratio"),
            ParameterValue::Bool(true),
        )
        .unwrap();

    let original = resampler.current_ratio();
    resampler.set_ratio_relative(1.01, true).unwrap();
    let expected = original * 1.01;
    assert!(
        (resampler.current_ratio() - expected).abs() < 1e-10,
        "Relative ratio should multiply: {} vs {}",
        resampler.current_ratio(),
        expected
    );
}

#[test]
fn test_dynamic_ratio_via_parameter() {
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();
    resampler.initialize(44100).unwrap();

    // Trying to set ratio parameter when dynamic is off should fail
    assert!(
        resampler
            .set_parameter(ParameterId::from("ratio"), ParameterValue::Float(1.1),)
            .is_err()
    );

    // Enable dynamic ratio
    resampler
        .set_parameter(
            ParameterId::from("dynamic_ratio"),
            ParameterValue::Bool(true),
        )
        .unwrap();

    // Now setting ratio via parameter should work
    let nominal = resampler.ratio() as f32;
    let new_ratio = nominal * 1.01;
    resampler
        .set_parameter(ParameterId::from("ratio"), ParameterValue::Float(new_ratio))
        .unwrap();
    assert!(
        (resampler.current_ratio() - new_ratio as f64).abs() < 1e-4,
        "Ratio via parameter: {} vs {}",
        resampler.current_ratio(),
        new_ratio
    );
}

#[test]
fn test_parameter_getset() {
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();

    // Quality
    assert_eq!(
        resampler.get_parameter(&ParameterId::from("quality")),
        Some(ParameterValue::Int(1))
    );
    resampler
        .set_parameter(ParameterId::from("quality"), ParameterValue::Int(0))
        .unwrap();
    assert_eq!(resampler.quality(), ResamplerQuality::Fast);

    // Dynamic ratio
    assert_eq!(
        resampler.get_parameter(&ParameterId::from("dynamic_ratio")),
        Some(ParameterValue::Bool(false))
    );

    // Ratio
    let ratio_val = resampler.get_parameter(&ParameterId::from("ratio"));
    assert!(ratio_val.is_some());
    if let Some(ParameterValue::Float(r)) = ratio_val {
        assert!((r as f64 - 48000.0 / 44100.0).abs() < 1e-4);
    }

    // Unknown
    assert_eq!(
        resampler.get_parameter(&ParameterId::from("nonexistent")),
        None
    );
}

/// flush() on a plugin with no buffered residual should return 0 frames.
#[test]
fn test_flush_empty_residual() {
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();
    resampler.initialize(44100).unwrap();

    let max_out = resampler.flush_output_frames_max();
    let mut flush_out = vec![0.0_f32; (max_out + 64) * 2];
    let (flushed, _) = resampler.flush(&mut flush_out).unwrap();
    assert_eq!(flushed, 0, "flush on empty residual should return 0");
}

/// flush() should recover trailing frames that process() buffered but could not emit.
#[test]
fn test_flush_recovers_trailing_frames() {
    // Use a block size that does NOT equal chunk_size so a residual is guaranteed.
    let chunk_size = 1024;
    let block_size = 300; // 300 < 1024 — will never complete a chunk in one call
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, chunk_size).unwrap();
    resampler.initialize(44100).unwrap();

    // Process exactly one small block — it will be buffered, producing 0 output frames.
    let input: Vec<f32> = (0..block_size * 2)
        .map(|i| 0.5 * (2.0 * std::f32::consts::PI * 1000.0 * (i / 2) as f32 / 44100.0).sin())
        .collect();
    let max_out = resampler.output_frames_for_input(block_size);
    let mut output = vec![0.0_f32; max_out * 2];
    let ctx = ProcessContext::new(44100, block_size);
    let produced = resampler.process(&input, &mut output, &ctx).unwrap();
    assert_eq!(
        produced, 0,
        "A block smaller than chunk_size should produce 0 output (buffered)"
    );

    // Now flush — should produce output for the buffered frames.
    let max_flush_out = resampler.flush_output_frames_max();
    let mut flush_out = vec![0.0_f32; (max_flush_out + 64) * 2];
    let (flushed, _) = resampler.flush(&mut flush_out).unwrap();
    assert!(
        flushed > 0,
        "flush() should produce output for the {block_size} buffered frames, got 0"
    );

    // Subsequent flush with no residual should be 0.
    let mut flush_out2 = vec![0.0_f32; (max_flush_out + 64) * 2];
    let (flushed2, _) = resampler.flush(&mut flush_out2).unwrap();
    assert_eq!(flushed2, 0, "second flush with no residual should return 0");
}

/// Variable block sizes: blocks smaller than chunk_size accumulate, and processing
/// enough blocks eventually produces output.
#[test]
fn test_variable_block_size_small() {
    let chunk_size = 1024;
    let block_size = 256; // 4 blocks fill one chunk
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, chunk_size).unwrap();
    resampler.initialize(44100).unwrap();

    let input = vec![0.5_f32; block_size * 2];
    let max_out = resampler.output_frames_for_input(chunk_size);
    let mut output = vec![0.0_f32; max_out * 2];
    let ctx = ProcessContext::new(44100, block_size);

    // First 3 blocks: buffered, no output.
    for i in 0..3 {
        let produced = resampler.process(&input, &mut output, &ctx).unwrap();
        assert_eq!(
            produced, 0,
            "block {i}: expected 0 output frames while filling residual"
        );
    }

    // 4th block completes the chunk — should produce output.
    let produced = resampler.process(&input, &mut output, &ctx).unwrap();
    assert!(
        produced > 0,
        "4th block (completing the chunk) should produce output, got 0"
    );
}

/// Variable block sizes: block larger than chunk_size but not a multiple.
#[test]
fn test_variable_block_size_non_multiple() {
    let chunk_size = 1024;
    let block_size = 1500; // spans one full chunk + 476 leftover
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, chunk_size).unwrap();
    resampler.initialize(44100).unwrap();

    let input = vec![0.5_f32; block_size * 2];
    let max_out = resampler.output_frames_for_input(block_size);
    let mut output = vec![0.0_f32; max_out * 2];
    let ctx = ProcessContext::new(44100, block_size);

    let produced = resampler.process(&input, &mut output, &ctx).unwrap();
    // 1500 frames = 1 full chunk (1024) + 476 residual.
    // Should produce output for the 1 complete chunk.
    assert!(
        produced > 0,
        "block of 1500 should produce output for its complete chunk, got 0"
    );

    // Flush the remaining 476 residual frames.
    let max_flush = resampler.flush_output_frames_max();
    let mut flush_out = vec![0.0_f32; (max_flush + 64) * 2];
    let (flushed, _) = resampler.flush(&mut flush_out).unwrap();
    assert!(flushed > 0, "flush should recover the 476 residual frames");
}

/// Empty block (num_frames == 0) should succeed and return 0 output frames.
#[test]
fn test_zero_frame_block() {
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();
    resampler.initialize(44100).unwrap();

    let input: Vec<f32> = vec![];
    let mut output = vec![0.0_f32; 256 * 2];
    let ctx = ProcessContext::new(44100, 0);
    let produced = resampler.process(&input, &mut output, &ctx).unwrap();
    assert_eq!(
        produced, 0,
        "zero-frame block should produce 0 output frames"
    );
}

/// Over a long run (10 s at 44.1 kHz → 48 kHz), total output frames should be
/// within a small margin of the expected count.
#[test]
fn test_cumulative_frame_count() {
    let chunk_size = 1024;
    let total_input_frames = 44100 * 10; // 10 seconds
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, chunk_size).unwrap();
    resampler.initialize(44100).unwrap();

    let input = vec![0.5_f32; chunk_size * 2];
    let max_out = resampler.output_frames_for_input(chunk_size);
    let mut output = vec![0.0_f32; max_out * 2];
    let ctx = ProcessContext::new(44100, chunk_size);

    let mut total_output = 0usize;
    let num_blocks = total_input_frames / chunk_size;
    for _ in 0..num_blocks {
        let produced = resampler.process(&input, &mut output, &ctx).unwrap();
        total_output += produced;
    }

    let expected = (total_input_frames as f64 * 48000.0 / 44100.0) as usize;
    // Allow ±2 frames of rubato's intrinsic phase jitter per chunk.
    let tolerance = num_blocks * 2;
    assert!(
        total_output.abs_diff(expected) <= tolerance,
        "Cumulative output frames: expected ~{expected}, got {total_output} \
             (tolerance ±{tolerance})"
    );
}

#[test]
fn test_disable_dynamic_ratio_resets() {
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();
    resampler.initialize(44100).unwrap();

    let nominal = resampler.ratio();

    // Enable, change ratio, then disable
    resampler
        .set_parameter(
            ParameterId::from("dynamic_ratio"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    resampler.set_ratio(nominal * 1.05, true).unwrap();
    assert!((resampler.current_ratio() - nominal).abs() > 0.01);

    // Disable should reset to nominal
    resampler
        .set_parameter(
            ParameterId::from("dynamic_ratio"),
            ParameterValue::Bool(false),
        )
        .unwrap();
    assert!(
        (resampler.current_ratio() - nominal).abs() < 1e-10,
        "Disabling dynamic_ratio should reset to nominal"
    );
}

/// f_cutoff must be quality-dependent — a fixed 0.95 for all presets leaves
/// short sinc kernels (Fast) with an unrealistically narrow transition band,
/// reducing stopband attenuation and risking aliasing on downsampling.
#[test]
fn test_f_cutoff_is_quality_dependent() {
    let fast = ResamplerQuality::Fast.f_cutoff();
    let medium = ResamplerQuality::Medium.f_cutoff();
    let high = ResamplerQuality::High.f_cutoff();

    // All values must be in (0, 1)
    assert!(
        fast > 0.0 && fast < 1.0,
        "Fast f_cutoff out of range: {}",
        fast
    );
    assert!(
        medium > 0.0 && medium < 1.0,
        "Medium f_cutoff out of range: {}",
        medium
    );
    assert!(
        high > 0.0 && high < 1.0,
        "High f_cutoff out of range: {}",
        high
    );

    // Shorter kernels need lower cutoffs to keep the transition band feasible
    assert!(
        fast < medium,
        "Fast cutoff ({}) should be lower than Medium ({})",
        fast,
        medium
    );
    assert!(
        medium < high,
        "Medium cutoff ({}) should be lower than High ({})",
        medium,
        high
    );

    // Sanity-check against rubato's calculate_cutoff for BlackmanHarris2
    assert!(
        (fast - 0.7891).abs() < 0.01,
        "Fast f_cutoff ({}) unexpectedly far from rubato's ~0.789",
        fast
    );
    assert!(
        (high - 0.9471).abs() < 0.01,
        "High f_cutoff ({}) unexpectedly far from rubato's ~0.947",
        high
    );
}

/// Downsampling must attenuate frequencies above the output Nyquist to prevent aliasing.
/// Regression test: with a fixed 0.95 cutoff, Fast quality (64 taps) had
/// insufficient transition-band width, risking aliasing.
#[test]
fn test_downsampling_anti_aliasing_fast() {
    let input_sr = 96000;
    let output_sr = 44100;
    let chunk_size = 1024;
    let mut resampler =
        ResamplerPlugin::with_quality(1, input_sr, output_sr, chunk_size, ResamplerQuality::Fast)
            .unwrap();
    resampler.initialize(input_sr).unwrap();

    // 22.5 kHz — just above output Nyquist (22.05 kHz).
    // With a short filter and a too-high cutoff, this frequency leaks through;
    // with a quality-adjusted cutoff it is strongly attenuated.
    let freq = 22500.0;
    let num_frames = chunk_size * 8;
    let mut input = vec![0.0_f32; num_frames];
    for i in 0..num_frames {
        let phase = 2.0 * std::f32::consts::PI * freq * i as f32 / input_sr as f32;
        input[i] = phase.sin() * 0.5;
    }

    let max_output = resampler.output_frames_for_input(num_frames);
    let mut output = vec![0.0_f32; max_output];
    let ctx = ProcessContext::new(input_sr, num_frames);
    let produced = resampler.process(&input, &mut output, &ctx).unwrap();

    let max_flush = resampler.flush_output_frames_max();
    let mut flush_out = vec![0.0_f32; max_flush];
    let (flushed, _) = resampler.flush(&mut flush_out).unwrap();

    let total_out = produced + flushed;
    let all_output: Vec<f32> = output[..produced]
        .iter()
        .chain(&flush_out[..flushed])
        .copied()
        .collect();

    let skip = (total_out / 5).max(1);
    let steady_state = &all_output[skip.min(total_out)..];
    assert!(
        !steady_state.is_empty(),
        "need non-empty steady-state output to compute RMS"
    );

    let rms: f32 =
        (steady_state.iter().map(|x| x * x).sum::<f32>() / steady_state.len() as f32).sqrt();
    assert!(
        rms < 0.05,
        "Aliasing detected: RMS should be strongly attenuated for freq above output Nyquist, got {:.6}",
        rms
    );
}

#[test]
fn test_latency_exact_value() {
    let resampler =
        ResamplerPlugin::with_quality(2, 44100, 48000, 1024, ResamplerQuality::Medium).unwrap();
    let latency = resampler.latency_samples();
    // Rubato reports 69 output frames. The 1023 input-frame priming interval
    // converts to 1114 output frames at 44.1 -> 48 kHz.
    assert_eq!(
        latency, 1183,
        "latency_samples should use one output-domain unit"
    );
}

/// Regression: latency_samples() must include the chunking buffer latency.
/// The plugin buffers up to chunk_size - 1 frames before calling rubato,
/// so the maximum end-to-end latency is output_delay() + chunk_size - 1.
#[test]
fn test_latency_includes_chunking_buffer() {
    let chunk_size = 1024;
    let resampler =
        ResamplerPlugin::with_quality(2, 44100, 48000, chunk_size, ResamplerQuality::Medium)
            .unwrap();
    let reported = resampler.latency_samples();
    let rubato_delay = resampler
        .resampler
        .as_ref()
        .map(|r| r.output_delay())
        .unwrap_or(0);
    assert!(
        reported >= rubato_delay + chunk_size - 1,
        "latency_samples()={reported} must include chunking buffer ({}), \
         rubato_delay={rubato_delay}",
        chunk_size - 1
    );
}

#[test]
fn test_flush_after_full_chunk_preserves_filter_tail() {
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();
    resampler.initialize(44100).unwrap();
    let input = vec![0.5_f32; 1024 * 2];
    let max_out = resampler.output_frames_for_input(1024);
    let mut output = vec![0.0_f32; max_out * 2];
    let produced = resampler
        .process(&input, &mut output, &ProcessContext::new(44100, 1024))
        .unwrap();
    assert!(produced > 0);
    let mut flush_out = vec![0.0_f32; max_out * 2];
    let (flushed, _) = resampler.flush(&mut flush_out).unwrap();
    assert_eq!(
        produced + flushed,
        resampler.output_delay_frames() + (1024_f64 * 48000.0 / 44100.0).ceil() as usize,
        "complete stream includes the output-domain leading delay and matching final tail"
    );
    assert!(flushed > 0, "a full final chunk still has a sinc tail");
}

#[test]
fn test_flush_output_buffer_too_small_returns_err() {
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();
    resampler.initialize(44100).unwrap();
    let input = vec![0.5_f32; 512 * 2];
    let max_out = resampler.flush_output_frames_max().max(1);
    let mut output = vec![0.0_f32; max_out * 2];
    resampler
        .process(&input, &mut output, &ProcessContext::new(44100, 512))
        .unwrap();
    let mut tiny_flush = vec![0.0_f32; 1];
    let result = resampler.flush(&mut tiny_flush);
    assert!(result.is_err(), "flush with too-small buffer must error");
}

#[test]
fn test_flush_after_reset_returns_zero() {
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();
    resampler.initialize(44100).unwrap();
    let input = vec![0.5_f32; 512 * 2];
    let max_out = resampler.flush_output_frames_max().max(1);
    let mut output = vec![0.0_f32; max_out * 2];
    resampler
        .process(&input, &mut output, &ProcessContext::new(44100, 512))
        .unwrap();
    resampler.reset();
    let mut flush_out = vec![0.0_f32; max_out * 2];
    let (flushed, _) = resampler.flush(&mut flush_out).unwrap();
    assert_eq!(flushed, 0, "flush after reset should return 0");
}

#[test]
fn test_process_input_length_mismatch_returns_err() {
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();
    resampler.initialize(44100).unwrap();
    let input = vec![0.5_f32; 99];
    let mut output = vec![0.0_f32; 256 * 2];
    let result = resampler.process(&input, &mut output, &ProcessContext::new(44100, 10));
    assert!(result.is_err(), "input length mismatch must error");
}

#[test]
fn test_process_output_buffer_too_small_returns_err() {
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();
    resampler.initialize(44100).unwrap();
    let input = vec![0.5_f32; 1024 * 2];
    let mut output = vec![0.0_f32; 1];
    let result = resampler.process(&input, &mut output, &ProcessContext::new(44100, 1024));
    assert!(result.is_err(), "output buffer too small must error");
}

#[test]
fn test_process_output_error_is_transactional() {
    let input = vec![0.5_f32; 1024 * 2];
    let context = ProcessContext::new(44100, 1024);
    let mut retried = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();
    let mut fresh = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();
    retried.initialize(44100).unwrap();
    fresh.initialize(44100).unwrap();

    assert!(retried.process(&input, &mut [0.0], &context).is_err());
    let frames = retried.output_frames_for_input(1024);
    let mut retry_output = vec![0.0; frames * 2];
    let mut fresh_output = vec![0.0; frames * 2];
    let retry_frames = retried
        .process(&input, &mut retry_output, &context)
        .unwrap();
    let fresh_frames = fresh.process(&input, &mut fresh_output, &context).unwrap();
    assert_eq!(retry_frames, fresh_frames);
    assert_eq!(
        &retry_output[..retry_frames * 2],
        &fresh_output[..fresh_frames * 2]
    );
}

#[test]
fn test_flush_output_error_preserves_residual_for_retry() {
    let input = vec![0.5_f32; 512 * 2];
    let context = ProcessContext::new(44100, 512);
    let mut retried = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();
    let mut fresh = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();
    retried.process(&input, &mut [], &context).unwrap();
    fresh.process(&input, &mut [], &context).unwrap();
    assert!(retried.flush(&mut [0.0]).is_err());
    let frames = retried.flush_output_frames_max();
    let mut retry_output = vec![0.0; frames * 2];
    let mut fresh_output = vec![0.0; frames * 2];
    let retry = retried.flush(&mut retry_output).unwrap();
    let uninterrupted = fresh.flush(&mut fresh_output).unwrap();
    assert_eq!(retry, uninterrupted);
    assert_eq!(
        &retry_output[..retry.0 * 2],
        &fresh_output[..uninterrupted.0 * 2]
    );
}

#[test]
fn test_initialize_rejects_mismatched_host_rate() {
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();
    assert!(resampler.initialize(48000).is_err());
    assert!(resampler.initialize(44100).is_ok());
}

#[test]
fn test_unity_rate_is_bit_exact_for_irregular_blocks() {
    let mut resampler = ResamplerPlugin::new(2, 48000, 48000, 1024).unwrap();
    resampler.initialize(48000).unwrap();
    assert_eq!(resampler.latency_samples(), 0);
    for frames in [1, 127, 256, 300, 1024, 1500] {
        let input: Vec<f32> = (0..frames * 2).map(|i| f32::from_bits(i as u32)).collect();
        let mut output = vec![0.0; input.len()];
        let produced = resampler
            .process(&input, &mut output, &ProcessContext::new(48000, frames))
            .unwrap();
        assert_eq!(produced, frames);
        assert_eq!(output, input);
    }
    let input = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.0];
    let mut output = [0.0; 4];
    resampler
        .process(&input, &mut output, &ProcessContext::new(48000, 2))
        .unwrap();
    assert_eq!(
        output.map(f32::to_bits),
        input.map(f32::to_bits),
        "unity plugin path is deliberately bit-transparent; DawHost owns non-finite rejection"
    );
}

#[test]
fn test_last_output_frames_reports_known_zero() {
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();
    assert_eq!(resampler.last_output_frames(), Some(0));
    resampler
        .process(&[0.0; 256 * 2], &mut [], &ProcessContext::new(44100, 256))
        .unwrap();
    assert_eq!(resampler.last_output_frames(), Some(0));
}

#[test]
fn test_frame_estimator_reports_only_complete_chunks() {
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();
    assert_eq!(resampler.available_output_frames(1), 0);
    resampler
        .process(&[0.0; 512 * 2], &mut [], &ProcessContext::new(44100, 512))
        .unwrap();
    assert_eq!(resampler.available_output_frames(511), 0);
    assert!(resampler.available_output_frames(512) > 0);
}

#[test]
fn estimator_capacity_and_availability_hold_for_one_to_sixteen_channels() {
    for channels in 1..=16 {
        let mut plugin = ResamplerPlugin::new(channels, 96_000, 22_050, 64).unwrap();
        for frames in [1, 2, 63, 64, 65, 128] {
            let available = plugin.available_output_frames(frames);
            let capacity = plugin.output_frames_for_input(frames);
            assert!(available <= capacity);
            if frames < 64 {
                assert_eq!(available, 0);
            }
        }
        plugin
            .process(
                &vec![0.0; 63 * channels],
                &mut [],
                &ProcessContext::new(96_000, 63),
            )
            .unwrap();
        assert_eq!(plugin.available_output_frames(0), 0);
        assert!(plugin.available_output_frames(1) > 0);
    }
}

#[test]
fn multi_chunk_capacity_error_is_transactional_before_second_chunk() {
    let input = vec![0.125; 2048 * 2];
    let context = ProcessContext::new(44_100, 2048);
    let mut retried = ResamplerPlugin::new(2, 44_100, 48_000, 1024).unwrap();
    let mut fresh = ResamplerPlugin::new(2, 44_100, 48_000, 1024).unwrap();
    let one_chunk_samples = retried.output_frames_for_input(1024) * 2;
    assert!(
        retried
            .process(&input, &mut vec![0.0; one_chunk_samples], &context)
            .is_err()
    );
    let samples = retried.output_frames_for_input(2048) * 2;
    let mut retry_output = vec![0.0; samples];
    let mut fresh_output = vec![0.0; samples];
    let retry_frames = retried
        .process(&input, &mut retry_output, &context)
        .unwrap();
    let fresh_frames = fresh.process(&input, &mut fresh_output, &context).unwrap();
    assert_eq!(retry_frames, fresh_frames);
    assert_eq!(
        &retry_output[..retry_frames * 2],
        &fresh_output[..fresh_frames * 2]
    );
}

#[test]
fn test_latency_converts_chunk_priming_to_output_frames() {
    let up = ResamplerPlugin::new(1, 22050, 44100, 1024).unwrap();
    let down = ResamplerPlugin::new(1, 96000, 44100, 1024).unwrap();
    assert_eq!(
        up.latency_samples(),
        up.output_delay_frames() + ((1023_f64 * 44100.0 / 22050.0).ceil() as usize)
    );
    assert_eq!(
        down.latency_samples(),
        down.output_delay_frames() + ((1023_f64 * 44100.0 / 96000.0).ceil() as usize)
    );
}

#[test]
fn signal_delay_matches_concatenated_impulse_without_callback_waiting() {
    let mut maximum_error = 0.0_f64;
    let mut cases = 0;
    for quality in [
        ResamplerQuality::Fast,
        ResamplerQuality::Medium,
        ResamplerQuality::High,
    ] {
        for (input_rate, output_rate) in [
            (48_000, 96_000),
            (96_000, 48_000),
            (44_100, 48_000),
            (48_000, 44_100),
        ] {
            for chunk in [64, 127, 513] {
                for partition in [1, 127, 257] {
                    for impulse_frame in [0, 1, 2, 17, 63, 127, 997] {
                        let mut plugin = ResamplerPlugin::with_quality(
                            1,
                            input_rate,
                            output_rate,
                            chunk,
                            quality,
                        )
                        .unwrap();
                        let physical_delay = plugin.signal_delay_samples();
                        assert!(plugin.latency_samples() as f64 > physical_delay);
                        let mut rendered = Vec::new();
                        let mut position = 0;
                        while position < 2_048 {
                            let frames = partition.min(2_048 - position);
                            let mut input = vec![0.0; frames];
                            if (position..position + frames).contains(&impulse_frame) {
                                input[impulse_frame - position] = 1.0;
                            }
                            let mut output = vec![0.0; plugin.output_frames_for_input(frames)];
                            let actual = plugin
                                .process(
                                    &input,
                                    &mut output,
                                    &ProcessContext::new(input_rate, frames),
                                )
                                .unwrap();
                            rendered.extend_from_slice(&output[..actual]);
                            position += frames;
                        }
                        let (peak_index, peak) = rendered
                            .iter()
                            .enumerate()
                            .max_by(|(_, a), (_, b)| a.abs().total_cmp(&b.abs()))
                            .unwrap();
                        assert!(peak.abs() > 0.2);
                        // Observe the integer peak independently. A sampled
                        // sinc peak lies within half a sample of its group
                        // center; the extra 0.02 allows table/window bias.
                        let center = impulse_frame as f64 * f64::from(output_rate)
                            / f64::from(input_rate)
                            + physical_delay;
                        let error = (peak_index as f64 - center).abs();
                        maximum_error = maximum_error.max(error);
                        cases += 1;
                        assert!(
                            error <= 0.52,
                            "{quality:?} {input_rate}->{output_rate}, offset={impulse_frame}, chunk={chunk}, partition={partition}: peak={peak_index}, center={center}, error={error}",
                        );
                    }
                }
            }
        }
    }
    eprintln!(
        "physical delay matrix: {cases} cases; maximum integer-peak error {maximum_error:.9} output frames"
    );
    let unity = ResamplerPlugin::new(1, 48_000, 48_000, 127).unwrap();
    assert_eq!(unity.signal_delay_samples(), 0.0);
}

#[test]
fn test_flush_produces_signal_not_silence() {
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();
    resampler.initialize(44100).unwrap();
    let num_frames = 512;
    let input: Vec<f32> = (0..num_frames * 2)
        .map(|i| 0.5 * (2.0 * std::f32::consts::PI * 1000.0 * (i / 2) as f32 / 44100.0).sin())
        .collect();
    let max_out = resampler.flush_output_frames_max().max(1);
    let mut output = vec![0.0_f32; max_out * 2];
    let produced = resampler
        .process(&input, &mut output, &ProcessContext::new(44100, num_frames))
        .unwrap();
    assert_eq!(produced, 0);
    let mut flush_out = vec![0.0_f32; resampler.flush_output_frames_max() * 2];
    let (flushed, _) = resampler.flush(&mut flush_out).unwrap();
    assert!(flushed > 0);
    let rms: f32 =
        (flush_out[..flushed * 2].iter().map(|s| s * s).sum::<f32>() / (flushed * 2) as f32).sqrt();
    assert!(
        rms > 0.01,
        "flush output should contain signal, but RMS was {rms}"
    );
}

#[test]
fn test_creation_rejects_zero_channels() {
    assert!(ResamplerPlugin::new(0, 44100, 48000, 1024).is_err());
}

#[test]
fn test_creation_rejects_zero_sample_rate() {
    assert!(ResamplerPlugin::new(2, 0, 48000, 1024).is_err());
    assert!(ResamplerPlugin::new(2, 44100, 0, 1024).is_err());
}

#[test]
fn test_creation_rejects_zero_chunk_size() {
    assert!(ResamplerPlugin::new(2, 44100, 48000, 0).is_err());
}

#[test]
fn realtime_quantum_exposes_chunk_boundary_work_budget() {
    let resampler = ResamplerPlugin::new(2, 96_000, 44_100, 64).unwrap();
    assert_eq!(resampler.realtime_quantum_frames(), 64);
    assert_eq!(Plugin::realtime_quantum_frames(&resampler), 64);

    let mut host = PluginHost::new(2, 96_000);
    host.add_plugin(Box::new(resampler)).unwrap();
    host.build().unwrap();
    assert_eq!(host.realtime_quantum_frames(), 64);
}

#[test]
fn cutoff_bank_ramps_protect_the_narrower_endpoint() {
    let bank = CutoffBank::new(1.0);
    let narrow = bank.target_slot(0.5, 0.5);
    assert!((bank.cutoff_ratio(narrow) - 0.5).abs() < 1e-12);
    // During an upward ramp the backend still reports the narrow ratio, so
    // both endpoints resolve to the narrow table. A downward ramp resolves
    // to its narrow target however it was requested.
    assert_eq!(bank.target_slot(0.5, 2.0), narrow);
    assert_eq!(bank.target_slot(2.0, 0.5), narrow);
    let wide = bank.target_slot(2.0, 2.0);
    assert!((bank.cutoff_ratio(wide) - 1.0).abs() < 1e-12);
    assert_eq!(bank.target_slot(1.0, 1.0), wide);
}

#[test]
fn cutoff_bank_narrowing_jumps_immediately_in_both_modes() {
    for smoothing in [false, true] {
        let mut bank = CutoffBank::new(1.0);
        bank.set_smoothing(smoothing);
        assert_eq!(bank.smoothing(), smoothing);
        assert_eq!(bank.current_slot(), 0);
        let narrow = bank.target_slot(0.5, 0.5);
        assert_eq!(bank.advance_toward(narrow), narrow);
        assert_eq!(bank.current_slot(), narrow);
        // Holding the target is a no-op in both modes.
        assert_eq!(bank.advance_toward(narrow), narrow);
    }
}

#[test]
fn cutoff_smoothing_slews_upward_one_table_per_step() {
    let mut bank = CutoffBank::new(1.0);
    assert_eq!(bank.table_count(), 10);
    bank.set_smoothing(true);
    let narrow = bank.target_slot(0.5, 0.5);
    let wide = bank.target_slot(2.0, 2.0);
    assert_eq!(bank.cutoff_rank(narrow), 0);
    assert_eq!(bank.cutoff_rank(wide), bank.table_count() - 1);
    assert_eq!(bank.advance_toward(narrow), narrow);

    let step_limit = 2.0_f64.powf(1.0 / 8.0) * (1.0 + 1e-9);
    let mut previous = narrow;
    let mut steps = 0;
    loop {
        let slot = bank.advance_toward(wide);
        steps += 1;
        assert_eq!(
            bank.cutoff_rank(slot),
            bank.cutoff_rank(previous) + 1,
            "smoothed widening must advance exactly one prepared table"
        );
        let ratio = bank.cutoff_ratio(slot);
        let before = bank.cutoff_ratio(previous);
        assert!(ratio > before, "smoothed widening must increase cutoff");
        assert!(
            ratio / before <= step_limit,
            "one grid interval bounds each widening step"
        );
        assert!(steps <= bank.table_count(), "slew must terminate");
        previous = slot;
        if slot == wide {
            break;
        }
    }
    assert_eq!(steps, bank.table_count() - 1);
    assert_eq!(bank.current_slot(), wide);

    // Without smoothing the same widening jumps in one call.
    let mut legacy = CutoffBank::new(1.0);
    assert!(!legacy.smoothing());
    assert_eq!(legacy.advance_toward(narrow), narrow);
    assert_eq!(legacy.advance_toward(wide), wide);
}

#[test]
fn cutoff_smoothing_downward_change_mid_slew_jumps_immediately() {
    let mut bank = CutoffBank::new(1.0);
    bank.set_smoothing(true);
    let narrow = bank.target_slot(0.5, 0.5);
    let wide = bank.target_slot(2.0, 2.0);
    bank.advance_toward(narrow);
    bank.advance_toward(wide);
    bank.advance_toward(wide);
    bank.advance_toward(wide);
    assert_ne!(bank.current_slot(), wide);
    // A new downward target preempts the slew in a single call, so the
    // narrower filter is active before the next backend chunk is rendered.
    let lower = bank.target_slot(2.0, 0.6);
    assert!(bank.cutoff_ratio(lower) <= 0.6);
    assert!(bank.cutoff_ratio(lower) < bank.cutoff_ratio(bank.current_slot()));
    assert_eq!(bank.advance_toward(lower), lower);
    assert_eq!(bank.current_slot(), lower);
    // Reset tracks the backend's return to slot zero and keeps the flag.
    bank.reset();
    assert_eq!(bank.current_slot(), 0);
    assert!(bank.smoothing());
}

#[test]
fn cutoff_smoothing_parameter_defaults_off_and_round_trips() {
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();
    assert_eq!(resampler.parameters().len(), 4);
    assert_eq!(
        resampler.get_parameter(&ParameterId::from("cutoff_smoothing")),
        Some(ParameterValue::Bool(false))
    );
    resampler
        .set_parameter(
            ParameterId::from("cutoff_smoothing"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    assert_eq!(
        resampler.get_parameter(&ParameterId::from("cutoff_smoothing")),
        Some(ParameterValue::Bool(true))
    );
    assert!(
        resampler
            .set_parameter(
                ParameterId::from("cutoff_smoothing"),
                ParameterValue::Int(1)
            )
            .is_err(),
        "cutoff_smoothing must reject non-bool values"
    );
}

#[test]
fn signal_delay_matches_documented_equation() {
    // Independent transcription of the signal_delay_samples contract:
    // ((sinc_len / 2 - 1 / oversampling_factor) * ratio - 1), floored at
    // zero, with bit-exact unity passthrough reporting zero delay.
    fn expected(quality: ResamplerQuality, input_rate: u32, output_rate: u32) -> f64 {
        if input_rate == output_rate {
            return 0.0;
        }
        let (taps, phases) = match quality {
            ResamplerQuality::Fast => (64.0, 128.0),
            ResamplerQuality::Medium => (128.0, 256.0),
            ResamplerQuality::High => (256.0, 256.0),
        };
        ((taps / 2.0 - 1.0 / phases) * output_rate as f64 / input_rate as f64 - 1.0).max(0.0)
    }
    for quality in [
        ResamplerQuality::Fast,
        ResamplerQuality::Medium,
        ResamplerQuality::High,
    ] {
        for (input_rate, output_rate) in [
            (44_100, 48_000),
            (48_000, 44_100),
            (48_000, 96_000),
            (96_000, 48_000),
            (96_000, 24_000),
            (48_000, 48_000),
        ] {
            let narrow = ResamplerPlugin::with_quality(1, input_rate, output_rate, 64, quality)
                .unwrap()
                .signal_delay_samples();
            let wide = ResamplerPlugin::with_quality(1, input_rate, output_rate, 1024, quality)
                .unwrap()
                .signal_delay_samples();
            assert_eq!(narrow, wide, "physical delay must not depend on chunk size");
            let reference = expected(quality, input_rate, output_rate);
            assert!(
                (narrow - reference).abs() < 1e-9,
                "{quality:?} {input_rate}->{output_rate}: {narrow} != {reference}"
            );
            if input_rate != output_rate {
                let plugin =
                    ResamplerPlugin::with_quality(1, input_rate, output_rate, 1024, quality)
                        .unwrap();
                assert!(
                    plugin.latency_samples() as f64 > narrow,
                    "realtime latency must exceed physical delay"
                );
            }
        }
    }
}

#[test]
fn test_zero_frame_process_then_flush_returns_zero() {
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();
    resampler.initialize(44100).unwrap();
    let input: Vec<f32> = vec![];
    let mut output = vec![0.0_f32; 64 * 2];
    let produced = resampler
        .process(&input, &mut output, &ProcessContext::new(44100, 0))
        .unwrap();
    assert_eq!(produced, 0);
    let mut flush_out = vec![0.0_f32; 64 * 2];
    let (flushed, _) = resampler.flush(&mut flush_out).unwrap();
    assert_eq!(flushed, 0);
}

#[test]
fn cutoff_bank_nominal_half_has_eighteen_tables_and_slews() {
    // Nominal 0.5 (48->24): slot0 holds 0.5, grid intervals -8..-1 give 8
    // distinct cutoffs below (0.25..0.458), intervals 1..8 give 8 distinct
    // above (0.545..1.0), interval 0 duplicates slot0, and the 0.1% drift
    // anchor 0.4995 is distinct and inside nominal/2..nominal. Total 18.
    let mut bank = CutoffBank::new(0.5);
    assert_eq!(bank.table_count(), 18);
    bank.set_smoothing(true);
    let narrow = bank.target_slot(0.25, 0.25);
    let wide = bank.target_slot(1.0, 1.0);
    assert_eq!(bank.cutoff_rank(narrow), 0);
    assert_eq!(bank.cutoff_rank(wide), bank.table_count() - 1);
    assert!((bank.cutoff_ratio(narrow) - 0.25).abs() < 1e-12);
    assert!((bank.cutoff_ratio(wide) - 1.0).abs() < 1e-12);
    assert_eq!(bank.advance_toward(narrow), narrow);

    let step_limit = 2.0_f64.powf(1.0 / 8.0) * (1.0 + 1e-9);
    let mut previous = narrow;
    let mut steps = 0;
    loop {
        let slot = bank.advance_toward(wide);
        steps += 1;
        assert_eq!(
            bank.cutoff_rank(slot),
            bank.cutoff_rank(previous) + 1,
            "smoothed widening must advance exactly one prepared table"
        );
        let ratio = bank.cutoff_ratio(slot);
        let before = bank.cutoff_ratio(previous);
        assert!(ratio > before, "smoothed widening must increase cutoff");
        assert!(
            ratio / before <= step_limit,
            "one grid interval bounds each widening step"
        );
        assert!(steps <= bank.table_count(), "slew must terminate");
        previous = slot;
        if slot == wide {
            break;
        }
    }
    assert_eq!(steps, bank.table_count() - 1);
    assert_eq!(bank.current_slot(), wide);

    // A downward target preempts the converged wide table immediately.
    let lower = bank.target_slot(1.0, 0.3);
    assert!(bank.cutoff_ratio(lower) <= 0.3);
    assert!(bank.cutoff_ratio(lower) < bank.cutoff_ratio(bank.current_slot()));
    assert_eq!(bank.advance_toward(lower), lower);

    // Without smoothing the same widening jumps in one call.
    let mut legacy = CutoffBank::new(0.5);
    assert_eq!(legacy.advance_toward(narrow), narrow);
    assert_eq!(legacy.advance_toward(wide), wide);
}

#[test]
fn cutoff_bank_upsampling_nominal_is_single_table() {
    // Nominal 2.0 (48->96): every grid candidate caps at 1.0 (a duplicate
    // of slot0) and the drift anchor fails its nominal/2 guard
    // (0.999 < 1.0), so only the nominal table exists and every slew is
    // trivially immediate. Only ceilings at or above 1.0 are valid here;
    // a narrower ceiling would select no table.
    let mut bank = CutoffBank::new(2.0);
    assert_eq!(bank.table_count(), 1);
    assert_eq!(bank.target_slot(1.0, 1.0), 0);
    assert_eq!(bank.target_slot(2.0, 2.0), 0);
    assert_eq!(bank.target_slot(1.0, 2.0), 0);
    for smoothing in [false, true] {
        bank.set_smoothing(smoothing);
        assert_eq!(bank.advance_toward(0), 0);
        assert_eq!(bank.current_slot(), 0);
    }
    bank.reset();
    assert_eq!(bank.current_slot(), 0);
}

// ---------------------------------------------------------------------------
// R11 capacity envelopes: stream-independent upper bounds.
// ---------------------------------------------------------------------------

/// Fresh initialized plugin with backend residual exactly `residual_frames`.
///
/// Feeds three full chunks plus the residual so backend state is genuinely
/// mid-stream; asserts the residual (a miscounted prime would make the
/// grid below vacuous).
fn envelope_primed_plugin(
    channels: usize,
    input_rate: u32,
    output_rate: u32,
    chunk: usize,
    residual_frames: usize,
) -> ResamplerPlugin {
    assert!(residual_frames < chunk, "prime residual must be sub-chunk");
    let mut plugin = ResamplerPlugin::new(channels, input_rate, output_rate, chunk).unwrap();
    plugin.initialize(input_rate).unwrap();
    let total = 3 * chunk + residual_frames;
    let input = vec![0.25f32; total * channels];
    let capacity = plugin.output_frames_for_input(total);
    let mut output = vec![0.0f32; capacity * channels];
    let produced = plugin
        .process(&input, &mut output, &ProcessContext::new(input_rate, total))
        .unwrap();
    assert!(
        produced <= capacity,
        "prime produced {produced} past its {capacity}-frame live bound"
    );
    assert_eq!(
        plugin.residual_frames, residual_frames,
        "prime must leave exactly the requested residual"
    );
    plugin
}

#[test]
fn envelope_covers_process_production_over_residual_grid() {
    for (input_rate, output_rate) in [(48_000u32, 24_000u32), (24_000, 48_000)] {
        // Full residual sweep on a small chunk, sampled phases plus
        // near-max-block sizes on the production chunk.
        for (chunk, residuals, sizes) in [
            (
                64usize,
                (0..64usize).collect::<Vec<_>>(),
                vec![1usize, 63, 64, 65, 256],
            ),
            (
                1024usize,
                vec![0usize, 1, 511, 512, 1022, 1023],
                vec![1usize, 1023, 1024, 1025, 7813, 8191, 8192],
            ),
        ] {
            for residual in &residuals {
                for &size in &sizes {
                    let mut plugin =
                        envelope_primed_plugin(2, input_rate, output_rate, chunk, *residual);
                    let live = plugin.output_frames_for_input(size);
                    let envelope = plugin
                        .output_frames_envelope(size)
                        .expect("initialized backend publishes a process envelope");
                    assert!(
                        live <= envelope,
                        "{input_rate}->{output_rate} chunk {chunk} residual {residual} size {size}: \
                         live {live} exceeds envelope {envelope}"
                    );
                    // Callers size from the live declaration.
                    let input = vec![0.125f32; size * 2];
                    let mut output = vec![0.0f32; live * 2];
                    let produced = plugin
                        .process(&input, &mut output, &ProcessContext::new(input_rate, size))
                        .unwrap();
                    assert!(
                        produced <= live,
                        "{input_rate}->{output_rate} chunk {chunk} residual {residual} size {size}: \
                         produced {produced} past live {live}"
                    );
                    assert!(
                        produced <= envelope,
                        "{input_rate}->{output_rate} chunk {chunk} residual {residual} size {size}: \
                         produced {produced} past envelope {envelope}"
                    );
                }
            }
        }
    }

    // Unity is exact identity, both envelopes.
    let unity = ResamplerPlugin::new(2, 48_000, 48_000, 1024).unwrap();
    for size in [0, 1, 64, 1024, 8192] {
        assert_eq!(unity.output_frames_envelope(size), Some(size));
    }
    assert_eq!(unity.drain_frames_envelope(), Some(0));

    // The envelope contract requires monotonicity (host propagation
    // queries larger derived quanta and relies on dominance).
    let up = ResamplerPlugin::new(2, 24_000, 48_000, 1024).unwrap();
    let mut previous = 0;
    for size in 0..4096 {
        let envelope = up.output_frames_envelope(size).unwrap();
        assert!(
            envelope >= previous,
            "process envelope must be non-decreasing (fell at {size})"
        );
        previous = envelope;
    }
}

#[test]
fn drain_envelope_covers_every_drain_emission() {
    for (input_rate, output_rate) in [(48_000u32, 24_000u32), (24_000, 48_000)] {
        for (chunk, residuals) in [
            (64usize, (0..64usize).collect::<Vec<_>>()),
            (1024usize, vec![0usize, 1, 511, 512, 1022, 1023]),
        ] {
            for residual in &residuals {
                let mut plugin =
                    envelope_primed_plugin(2, input_rate, output_rate, chunk, *residual);
                let live_max = plugin.drain_output_frames_max();
                let bound = plugin
                    .drain_frames_envelope()
                    .expect("initialized backend publishes a drain envelope");
                assert!(
                    live_max <= bound,
                    "{input_rate}->{output_rate} chunk {chunk} residual {residual}: \
                     live drain bound {live_max} exceeds envelope {bound}"
                );
                let context = ProcessContext::new(input_rate, 0);
                plugin.begin_drain(&context).unwrap();
                // Size the destination from the envelope itself: an
                // understated envelope fails loudly here, not silently.
                let mut output = vec![0.0f32; bound * 2];
                let mut calls = 0u32;
                loop {
                    let step = plugin.drain(&mut output, &context).unwrap();
                    assert!(
                        step.frames <= bound,
                        "{input_rate}->{output_rate} chunk {chunk} residual {residual}: \
                         drain emitted {} past envelope {bound}",
                        step.frames
                    );
                    calls += 1;
                    assert!(calls < 1024, "drain must converge");
                    if step.complete {
                        break;
                    }
                }
                // EOF: further drains stay complete with zero frames.
                let step = plugin.drain(&mut output, &context).unwrap();
                assert!(
                    step.complete && step.frames == 0,
                    "drain after EOF must stay complete and empty"
                );
            }
        }
    }

    // Cold: a fresh stream drains complete immediately with zero frames.
    let mut cold = ResamplerPlugin::new(2, 48_000, 24_000, 1024).unwrap();
    cold.initialize(48_000).unwrap();
    let bound = cold.drain_frames_envelope().unwrap();
    let context = ProcessContext::new(48_000, 0);
    cold.begin_drain(&context).unwrap();
    let mut output = vec![0.0f32; bound * 2];
    let step = cold.drain(&mut output, &context).unwrap();
    assert!(
        step.complete && step.frames == 0,
        "cold drain must be empty"
    );
}

#[test]
fn drain_envelope_holds_across_dynamic_ratio_and_reset() {
    let chunk = 256;
    let mut plugin = envelope_primed_plugin(2, 48_000, 24_000, chunk, 100);
    plugin
        .set_parameter(
            ParameterId::from("dynamic_ratio"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    let nominal = plugin.ratio();
    let bound = plugin.drain_frames_envelope().unwrap();
    // Ratios across the covered range, ramped and jumped, each followed
    // by real input and a drain probe that must stay within the envelope.
    for ratio in [nominal / 2.0, nominal * 0.75, nominal * 1.5, nominal * 2.0] {
        for ramp in [false, true] {
            plugin.set_ratio(ratio, ramp).unwrap();
            let envelope_after = plugin.output_frames_envelope(chunk).unwrap();
            let input = vec![0.125f32; chunk * 2];
            let live = plugin.output_frames_for_input(chunk);
            let mut output = vec![0.0f32; live * 2];
            let produced = plugin
                .process(&input, &mut output, &ProcessContext::new(48_000, chunk))
                .unwrap();
            assert!(
                produced <= envelope_after,
                "ratio {ratio} ramp {ramp}: produced {produced} past envelope {envelope_after}"
            );
        }
    }
    assert_eq!(
        plugin.drain_frames_envelope().unwrap(),
        bound,
        "drain envelope must not move under ratio changes"
    );
    let context = ProcessContext::new(48_000, 0);
    plugin.begin_drain(&context).unwrap();
    let mut output = vec![0.0f32; bound * 2];
    let mut calls = 0u32;
    loop {
        let step = plugin.drain(&mut output, &context).unwrap();
        assert!(
            step.frames <= bound,
            "post-ratio drain emitted {} past envelope {bound}",
            step.frames
        );
        calls += 1;
        assert!(calls < 1024, "drain must converge");
        if step.complete {
            break;
        }
    }

    // Warm: reset starts a second stream whose drain stays covered.
    plugin.reset();
    let input = vec![0.125f32; 3 * chunk * 2];
    let live = plugin.output_frames_for_input(3 * chunk);
    let mut output = vec![0.0f32; live * 2];
    plugin
        .process(&input, &mut output, &ProcessContext::new(48_000, 3 * chunk))
        .unwrap();
    plugin.begin_drain(&context).unwrap();
    let mut output = vec![0.0f32; bound * 2];
    let step = plugin.drain(&mut output, &context).unwrap();
    assert!(
        step.frames <= bound,
        "warm drain emitted {} past envelope {bound}",
        step.frames
    );
}

#[test]
fn envelope_queries_are_allocation_free_and_quality_stable() {
    // Quality rebuilds the backend with identical chunk/ratio bounds, so
    // both envelopes keep unchanged values (not just still Some).
    // Quality locks at activation, so this runs pre-initialization.
    let mut plugin = ResamplerPlugin::new(2, 24_000, 48_000, 256).unwrap();
    let process_before = plugin.output_frames_envelope(1024).unwrap();
    let drain_before = plugin.drain_frames_envelope().unwrap();
    plugin
        .set_parameter(
            ParameterId::from("quality"),
            ParameterValue::String("high".to_string()),
        )
        .unwrap();
    assert_eq!(
        plugin.output_frames_envelope(1024),
        Some(process_before),
        "process envelope must survive quality rebuild unchanged"
    );
    assert_eq!(
        plugin.drain_frames_envelope(),
        Some(drain_before),
        "drain envelope must survive quality rebuild unchanged"
    );

    // Mid-stream state queries stay allocation-free.
    let mut plugin = envelope_primed_plugin(2, 24_000, 48_000, 256, 100);
    assert_no_allocs_or_deallocs("resampler envelope queries mid-stream", || {
        for size in [0, 1, 256, 1024, 8192] {
            let envelope = plugin.output_frames_envelope(size);
            assert!(envelope.is_some());
            assert!(plugin.output_frames_for_input(size) <= envelope.unwrap());
        }
        assert!(plugin.drain_frames_envelope().is_some());
    });

    // Draining state queries stay allocation-free too.
    let context = ProcessContext::new(24_000, 0);
    plugin.begin_drain(&context).unwrap();
    assert_no_allocs_or_deallocs("resampler envelope queries draining", || {
        assert!(plugin.output_frames_envelope(1024).is_some());
        assert!(plugin.drain_frames_envelope().is_some());
    });
}

#[test]
fn uninitialized_backend_answers_unknown_envelope() {
    let mut plugin = ResamplerPlugin::new(2, 48_000, 24_000, 1024).unwrap();
    plugin.resampler = None;
    assert_eq!(plugin.output_frames_envelope(1024), None);
    assert_eq!(plugin.drain_frames_envelope(), None);
}

#[test]
fn backend_next_never_exceeds_max_across_min_max_jump_with_debt() {
    // F3: both envelopes reduce to the fork invariant
    // output_frames_next() <= output_frames_max(). Exhausting
    // last_index x ramp corners is trust in the fork (which sizes its
    // own buffers by this max); this test samples the named uncovered
    // corner instead of claiming exhaustion: a min->max ratio jump (and
    // back) against deferred-step debt from a settled low-ratio stream
    // plus a partial-chunk residual. Any violation would surface
    // downstream as a loud capacity error, never silent corruption.
    for (input_rate, output_rate) in [(48_000u32, 24_000u32), (24_000, 48_000)] {
        let chunk = 256;
        // Genuinely mid-stream with a partial residual (deferred debt).
        let mut plugin = envelope_primed_plugin(2, input_rate, output_rate, chunk, 100);
        plugin
            .set_parameter(
                ParameterId::from("dynamic_ratio"),
                ParameterValue::Bool(true),
            )
            .unwrap();
        let nominal = plugin.ratio();
        let check_next = |plugin: &ResamplerPlugin, leg: &str| {
            let backend = plugin.resampler.as_ref().expect("backend must exist");
            let next = backend.output_frames_next();
            let max = backend.output_frames_max();
            assert!(
                next <= max,
                "{input_rate}->{output_rate} {leg}: next {next} exceeds max {max}"
            );
        };
        let run_chunk = |plugin: &mut ResamplerPlugin, leg: &str| {
            let live = plugin.output_frames_for_input(chunk);
            let input = vec![0.125f32; chunk * 2];
            let mut output = vec![0.0f32; live * 2];
            let context = ProcessContext::new(input_rate, chunk);
            let produced = plugin.process(&input, &mut output, &context).unwrap();
            assert!(
                produced <= live,
                "{input_rate}->{output_rate} {leg}: produced {produced} past live {live}"
            );
        };
        // Settle at the floor with real input, building deferred-step
        // debt at minimum output density.
        plugin.set_ratio(nominal / 2.0, false).unwrap();
        run_chunk(&mut plugin, "floor settle");
        check_next(&plugin, "settled at min ratio");
        // Jump straight across the covered range (no ramp), checking
        // immediately and after each subsequent backend block.
        for (target, leg) in [
            (nominal * 2.0, "min-to-max jump"),
            (nominal / 2.0, "max-to-min jump"),
        ] {
            plugin.set_ratio(target, false).unwrap();
            check_next(&plugin, leg);
            for _ in 0..3 {
                run_chunk(&mut plugin, leg);
                check_next(&plugin, leg);
            }
        }
    }
}
