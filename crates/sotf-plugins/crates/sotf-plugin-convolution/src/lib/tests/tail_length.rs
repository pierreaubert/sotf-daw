// Rust guideline compliant 2026-02-21

use super::*;
use sotf_host::TailLength;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct ImpulseFile(PathBuf);

impl ImpulseFile {
    fn new(samples: &[i16], rate: u32) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "sotf-convolution-tail-{}-{}.wav",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let data_size = (samples.len() * 2) as u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_size).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_size.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(&path, bytes).unwrap();
        Self(path)
    }

    fn path(&self) -> &str {
        self.0.to_str().unwrap()
    }
}

impl Drop for ImpulseFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn render(plugin: &mut ConvolutionPlugin, samples: &mut [f32], rate: u32) {
    let mut offset = 0;
    for frames in [1, 7, 127, 1031, 33].into_iter().cycle() {
        if offset == samples.len() {
            break;
        }
        let end = (offset + frames).min(samples.len());
        plugin
            .process_in_place(
                &mut samples[offset..end],
                &ProcessContext::new(rate, end - offset),
            )
            .unwrap();
        offset = end;
    }
}

#[test]
fn inactive_tail_covers_the_declared_dry_delay() {
    for (use_nupc, zero_latency_head) in [(false, false), (true, false), (true, true)] {
        let mut plugin = ConvolutionPlugin::from_params(
            1,
            48_000,
            ConvolutionPluginParams {
                use_nupc,
                zero_latency_head,
                ..ConvolutionPluginParams::default()
            },
        )
        .unwrap();
        let latency = if use_nupc && zero_latency_head {
            0
        } else {
            1024
        };
        assert_eq!(plugin.tail_length(), TailLength::Finite(latency));
        let mut samples = vec![0.0; latency as usize + 33];
        samples[0] = 0.75;
        render(&mut plugin, &mut samples, 48_000);
        for (index, sample) in samples.into_iter().enumerate() {
            assert_eq!(sample, if index == latency as usize { 0.75 } else { 0.0 });
        }
        plugin.reset();
        assert_eq!(plugin.tail_length(), TailLength::Finite(latency));
    }
}

#[test]
fn resampled_ir_tail_uses_prepared_length_and_matches_direct_sum() {
    for (source_rate, target_rate) in [(44_100_u32, 48_000_u32), (48_000, 96_000), (96_000, 44_100)]
    {
        for length in [257, 2053] {
            let mut ir = vec![0_i16; length];
            ir[0] = 16384;
            ir[length / 2] = -4096;
            ir[length - 1] = 8192;
            let file = ImpulseFile::new(&ir, source_rate);
            // The resampled coefficients are fixture data for an independent
            // time-domain sum. This tests convolution/support, not SRC quality.
            let coefficients = ConvolutionPlugin::resample_ir(
                &[ir.iter().map(|&x| f32::from(x) / 32768.0).collect()],
                source_rate,
                target_rate,
            )
            .unwrap()
            .remove(0);
            let expected_length =
                (length as u64 * u64::from(target_rate)).div_ceil(u64::from(source_rate)) as usize;
            assert_eq!(coefficients.len(), expected_length);
            for (use_nupc, zero_latency_head) in [(false, false), (true, false), (true, true)] {
                let mut plugin = ConvolutionPlugin::from_params(
                    1,
                    target_rate,
                    ConvolutionPluginParams {
                        ir_file: file.path().into(),
                        use_nupc,
                        zero_latency_head,
                        ..ConvolutionPluginParams::default()
                    },
                )
                .unwrap();
                plugin.reset();
                let latency = if use_nupc && zero_latency_head {
                    0
                } else {
                    1024
                };
                let bound = latency + expected_length - 1;
                assert_eq!(plugin.tail_length(), TailLength::Finite(bound as u64));
                let mut samples = vec![0.0; 18 + bound + 2049];
                samples[0] = 0.5;
                samples[17] = -0.25;
                render(&mut plugin, &mut samples, target_rate);
                for (frame, &sample) in samples.iter().enumerate() {
                    let tap = |delay| {
                        frame
                            .checked_sub(latency + delay)
                            .and_then(|index| coefficients.get(index))
                            .copied()
                            .map(f64::from)
                            .unwrap_or(0.0)
                    };
                    let expected = 0.5 * tap(0) - 0.25 * tap(17);
                    assert!(
                        (f64::from(sample) - expected).abs() < 1e-5,
                        "{source_rate}->{target_rate}, length={length}, nupc={use_nupc}, head={zero_latency_head}, frame={frame}"
                    );
                }
                assert!(samples[18 + bound..].iter().all(|x| x.abs() < 1e-5));
            }
        }
    }
}

#[test]
fn pending_completion_at_old_final_tap_keeps_the_fade_and_history_bound() {
    let rate = 48_000;
    let mut old_ir = vec![0_i16; 4097];
    old_ir[4096] = 16384;
    let long = ImpulseFile::new(&old_ir, rate);
    let short = ImpulseFile::new(&[8192], rate);
    for (use_nupc, zero_latency_head) in [(false, false), (true, false), (true, true)] {
        let mut plugin = ConvolutionPlugin::from_params(
            1,
            rate,
            ConvolutionPluginParams {
                ir_file: long.path().into(),
                use_nupc,
                zero_latency_head,
                ..ConvolutionPluginParams::default()
            },
        )
        .unwrap();
        plugin.reset();
        let latency = if use_nupc && zero_latency_head {
            0
        } else {
            1024
        };
        let old_bound = latency + 4096;
        assert_eq!(plugin.tail_length(), TailLength::Finite(old_bound));
        let replacement = ConvolutionPlugin::build_ir_state(
            short.path(),
            1,
            rate,
            use_nupc,
            zero_latency_head,
            128,
        )
        .unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        plugin.desired_generation += 1;
        let generation = plugin.desired_generation;
        plugin.ir_load_result_rx = Some(receiver);
        plugin.completion_pending = true;
        plugin.ir_load_result_keepalive = Some(sender.clone());
        assert_eq!(plugin.tail_length(), TailLength::Unknown);
        let mut input = vec![0.0; old_bound as usize + 1];
        input[0] = 1.0;
        render(&mut plugin, &mut input, rate);
        assert!((input[old_bound as usize] - 0.5).abs() < 1e-5);
        assert_eq!(plugin.tail_length(), TailLength::Unknown);
        sender
            .send(IrLoadCompletion {
                generation,
                result: Ok(replacement),
            })
            .unwrap();
        let mut fade = [0.0; 160];
        render(&mut plugin, &mut fade, rate);
        for (frame, &sample) in fade.iter().enumerate() {
            let expected = 0.5 * (1.0 - (frame + 1).min(128) as f32 / 128.0);
            assert!(
                (sample - expected).abs() < 1e-5,
                "frame={frame}: {sample} != {expected}"
            );
        }
        assert_eq!(plugin.tail_length(), TailLength::Finite(old_bound + 128));
        render(&mut plugin, &mut [0.0; 300], rate);
        assert_eq!(plugin.tail_length(), TailLength::Finite(old_bound + 128));
        // Clearing the IR can hold the current last output too, so its bound
        // must not shrink until an explicit history reset.
        plugin
            .parametric_set_parameter(
                ParameterId::from("ir_file"),
                ParameterValue::String(String::new()),
            )
            .unwrap();
        assert_eq!(plugin.tail_length(), TailLength::Finite(old_bound + 256));
        plugin.reset();
        assert_eq!(plugin.tail_length(), TailLength::Finite(latency));
    }
}

#[test]
fn rate_change_is_unknown_until_resampled_state_is_accepted_and_reset() {
    let mut ir = vec![0_i16; 2053];
    ir[0] = 16384;
    ir[2052] = 8192;
    let file = ImpulseFile::new(&ir, 48_000);
    let mut plugin = ConvolutionPlugin::from_params(
        1,
        48_000,
        ConvolutionPluginParams {
            ir_file: file.path().into(),
            zero_latency_head: true,
            ..ConvolutionPluginParams::default()
        },
    )
    .unwrap();
    plugin.reset();
    assert_eq!(plugin.tail_length(), TailLength::Finite(2052));
    plugin.initialize(96_000).unwrap();
    assert_eq!(plugin.tail_length(), TailLength::Unknown);

    // Replace the real loader mailbox from the control thread with an already
    // prepared result. This makes completion timing deterministic; construction
    // still exercises the production file decoding/resampling path.
    let replacement =
        ConvolutionPlugin::build_ir_state(file.path(), 1, 96_000, true, true, 128).unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    plugin.ir_load_result_rx = Some(receiver);
    plugin.completion_pending = true;
    plugin.ir_load_result_keepalive = Some(sender.clone());
    sender
        .send(IrLoadCompletion {
            generation: plugin.desired_generation,
            result: Ok(replacement),
        })
        .unwrap();
    plugin
        .process_in_place(&mut [], &ProcessContext::new(96_000, 0))
        .unwrap();
    assert_eq!(plugin.tail_length(), TailLength::Finite(4105));
    plugin.reset();
    assert_eq!(plugin.tail_length(), TailLength::Finite(4105));
}
