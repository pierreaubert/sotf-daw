//! Resource admission follows channel-expanded prepared convolution storage.

// Rust guideline compliant 2026-02-21
use super::*;

#[test]
fn long_mono_ir_rejects_channel_expanded_storage_over_budget() {
    let ir = vec![vec![0.0; 48_000 * 30]];
    let upc_fdl_lower_bound =
        ir[0].len().div_ceil(PARTITION_SIZE) * 32 * FFT_SIZE * std::mem::size_of::<Complex<f32>>();
    assert!(upc_fdl_lower_bound > MAX_IR_MEMORY_BYTES);
    let nupc_fdl_lower_bound: usize = nupc::plan_partitions(ir[0].len(), PARTITION_SIZE)
        .iter()
        .map(|spec| spec.count * (spec.fft_size / 2 + 1) * 32 * std::mem::size_of::<Complex<f32>>())
        .sum();
    // Half-spectrum FDL storage alone no longer exceeds the cap. Admission
    // deliberately retains the conservative full-complex backend estimate.
    assert!(nupc_fdl_lower_bound < MAX_IR_MEMORY_BYTES);
    assert!(
        ConvolutionPlugin::estimated_ordinary_backend_bytes(&[ir[0].len()], true, 32, false, 128)
            > MAX_IR_MEMORY_BYTES
    );
    for use_nupc in [false, true] {
        assert!(
            ConvolutionPlugin::validate_ir_limits_for_routing(
                &ir,
                48_000,
                IrValidationOptions {
                    target_sample_rate: 48_000.0,
                    output_channels: 32,
                    use_nupc,
                    true_stereo: false,
                    zero_latency_head: false,
                    head_taps: 128
                }
            )
            .is_err(),
            "admitted {use_nupc} backend above the conservative storage budget"
        );
    }
}

#[test]
fn ordinary_admission_boundaries_follow_resampled_geometry() {
    for outputs in [1, 2, 12, 32] {
        for ir_channels in [1, 3, 32] {
            for (use_nupc, head) in [(false, false), (true, false), (true, true)] {
                let estimate = |source_frames| {
                    ConvolutionPlugin::estimated_ordinary_backend_bytes(
                        &vec![source_frames * 8; ir_channels],
                        use_nupc,
                        outputs,
                        head,
                        257,
                    )
                };
                let mut low = 1_usize;
                let mut high = 48_000_usize * 30;
                while low < high {
                    let middle = low + (high - low).div_ceil(2);
                    if estimate(middle) <= MAX_IR_MEMORY_BYTES {
                        low = middle;
                    } else {
                        high = middle - 1;
                    }
                }
                let options = IrValidationOptions {
                    target_sample_rate: 384_000.0,
                    output_channels: outputs,
                    use_nupc,
                    true_stereo: false,
                    zero_latency_head: head,
                    head_taps: 257,
                };
                let mut ir = vec![vec![0.0; low]; ir_channels];
                assert!(
                    ConvolutionPlugin::validate_ir_limits_for_routing(&ir, 48_000, options).is_ok(),
                    "last admitted source length {low}: {outputs}/{ir_channels}/{use_nupc}/{head}"
                );
                ir.iter_mut().for_each(|channel| channel.push(0.0));
                assert!(
                    ConvolutionPlugin::validate_ir_limits_for_routing(&ir, 48_000, options)
                        .is_err(),
                    "next source length must fail memory or duration: {outputs}/{ir_channels}/{use_nupc}/{head}"
                );
            }
        }
    }
}

#[test]
fn ordinary_shared_kernel_is_counted_once_and_overflow_refuses() {
    for head in [false, true] {
        let estimate = |lengths: &[usize], outputs| {
            ConvolutionPlugin::estimated_ordinary_backend_bytes(lengths, true, outputs, head, 257)
        };
        assert!(estimate(&[8197], 2) < 2 * estimate(&[8197], 1));
        assert!(estimate(&[8197; 3], 12) < estimate(&[8197; 12], 12));
        for lengths in [&[][..], &[0][..], &[usize::MAX][..]] {
            assert_eq!(estimate(lengths, 32), usize::MAX);
        }
        assert_eq!(estimate(&[8197], 0), usize::MAX);
        assert_eq!(estimate(&[8197], usize::MAX), usize::MAX);
    }
    assert_eq!(
        ConvolutionPlugin::estimated_ordinary_backend_bytes(&[usize::MAX], false, 32, false, 128),
        usize::MAX
    );
}

#[test]
fn decoding_rejects_duration_before_returning_oversized_samples() {
    use std::io::Write;
    let clock = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "sotf-decode-duration-{}-{clock}.wav",
        std::process::id()
    ));
    // One Hz keeps real complete duration-limit fixtures tiny.
    fn write_wav(path: &Path, frames: u32) {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .unwrap();
        file.write_all(b"RIFF").unwrap();
        file.write_all(&(36 + frames * 2).to_le_bytes()).unwrap();
        file.write_all(b"WAVEfmt ").unwrap();
        file.write_all(&16_u32.to_le_bytes()).unwrap();
        file.write_all(&1_u16.to_le_bytes()).unwrap();
        file.write_all(&1_u16.to_le_bytes()).unwrap();
        file.write_all(&1_u32.to_le_bytes()).unwrap();
        file.write_all(&2_u32.to_le_bytes()).unwrap();
        file.write_all(&2_u16.to_le_bytes()).unwrap();
        file.write_all(&16_u16.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&(frames * 2).to_le_bytes()).unwrap();
        for _ in 0..frames {
            file.write_all(&8192_i16.to_le_bytes()).unwrap();
        }
    }
    write_wav(&path, 31);
    let result = ConvolutionPlugin::load_audio_file(path.to_str().unwrap());
    assert!(
        ConvolutionPlugin::validate_ir_resource_for_routing(
            path.to_str().unwrap(),
            Some(1.0),
            1,
            true,
            false,
            false,
            128,
        )
        .is_err()
    );
    let valid_path = path.with_extension("valid.wav");
    write_wav(&valid_path, 30);
    let mut plugin = ConvolutionPlugin::new(1, 1);
    let mut reference = ConvolutionPlugin::new(1, 1);
    plugin.load_ir(valid_path.to_str().unwrap()).unwrap();
    reference.load_ir(valid_path.to_str().unwrap()).unwrap();
    for active in [&mut plugin, &mut reference] {
        active
            .process_in_place(&mut [0.25; 17], &ProcessContext::new(1, 17))
            .unwrap();
    }
    let state = Arc::clone(&plugin.state);
    let generation = plugin.desired_generation;
    assert!(plugin.load_ir(path.to_str().unwrap()).is_err());
    assert!(Arc::ptr_eq(&state, &plugin.state));
    assert_eq!(plugin.desired_generation, generation);
    assert_eq!(plugin.max_ir_frames, 30);
    let mut actual = [0.125; 2048];
    let mut expected = actual;
    plugin
        .process_in_place(&mut actual, &ProcessContext::new(1, 2048))
        .unwrap();
    reference
        .process_in_place(&mut expected, &ProcessContext::new(1, 2048))
        .unwrap();
    assert_eq!(
        actual, expected,
        "decode refusal changed live audio history"
    );
    std::fs::remove_file(path).unwrap();
    std::fs::remove_file(valid_path).unwrap();
    assert!(
        result.is_err(),
        "raw decoding accepted 31 seconds of samples before resource admission"
    );
}
