//! Compare the public streaming plugin against direct time-domain convolution.

// Rust guideline compliant 2026-02-21

use sotf_host::{ParametricInPlacePlugin, ProcessContext, TailLength};
use sotf_plugin_convolution::{ConvolutionPlugin, ConvolutionPluginParams};
use std::path::PathBuf;

#[global_allocator]
static ALLOCATOR: sotf_host::CountingAlloc = sotf_host::CountingAlloc;

struct ImpulseFile(PathBuf);

impl Drop for ImpulseFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn write_impulse(channels: &[Vec<i16>], sample_rate: u32) -> ImpulseFile {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "sotf-convolution-oracle-{}-{unique}.wav",
        std::process::id()
    ));
    let channel_count = channels.len() as u16;
    let frames = channels[0].len();
    let data_bytes = (frames * channels.len() * 2) as u32;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&channel_count.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(sample_rate * u32::from(channel_count) * 2).to_le_bytes());
    bytes.extend_from_slice(&(channel_count * 2).to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_bytes.to_le_bytes());
    for frame in 0..frames {
        for channel in channels {
            bytes.extend_from_slice(&channel[frame].to_le_bytes());
        }
    }
    std::fs::write(&path, bytes).unwrap();
    ImpulseFile(path)
}

fn direct_convolution(input: &[f32], impulse: &[Vec<i16>]) -> Vec<f64> {
    let channels = 3;
    let mut output = vec![0.0; input.len() + (impulse[0].len() - 1) * channels];
    for (frame, samples) in input.as_chunks::<3>().0.iter().enumerate() {
        for (channel, &sample) in samples.iter().enumerate() {
            for (tap, &coefficient) in impulse[channel % impulse.len()].iter().enumerate() {
                output[(frame + tap) * channels + channel] +=
                    f64::from(sample) * f64::from(coefficient) / 32768.0;
            }
        }
    }
    output
}

#[test]
fn every_backend_matches_direct_convolution_with_full_tail_and_partial_callbacks() {
    let rate = 48_000;
    // An odd input length leaves a partial final UPC/NUPC partition. Distinct
    // channels expose accidental shared history and incorrect cyclic IR mapping.
    let input: Vec<f32> = (0..2051)
        .flat_map(|frame| {
            let t = frame as f32;
            [
                0.2 * (0.13 * t).sin(),
                0.1 * (0.037 * t).cos(),
                ((frame * 17 % 97) as f32 - 48.0) / 256.0,
            ]
        })
        .collect();
    // Dense IRs exercise every overlap sample; explicit signed taps straddle
    // FFT partition boundaries and keep the last response measurable.
    for ir_frames in [97, 2053, 8195] {
        let impulse: Vec<Vec<i16>> = (0..2)
            .map(|channel| {
                let mut ir: Vec<i16> = (0..ir_frames)
                    .map(|tap| ((tap * (31 + channel * 12) % 101) as i16 - 50) * 3)
                    .collect();
                for (tap, value) in [
                    (0, 16000),
                    (127, -9000),
                    (1023, 7000),
                    (1024, -6000),
                    (2047, 5000),
                    (2048, -4000),
                    (ir_frames - 1, 8000),
                ] {
                    if tap < ir.len() {
                        ir[tap] += if channel == 0 { value } else { -value / 2 };
                    }
                }
                ir
            })
            .collect();
        let file = write_impulse(&impulse, rate);
        let reference = direct_convolution(&input, &impulse);
        for (use_nupc, zero_latency_head, head_taps) in [
            (false, false, 128),
            (true, false, 128),
            (true, true, 32),
            (true, true, 128),
            (true, true, 512),
        ] {
            for mix in [1.0_f32, 0.375] {
                let gain_db = -3.0;
                let mut processor = ConvolutionPlugin::from_params(
                    3,
                    rate,
                    ConvolutionPluginParams {
                        ir_file: file.0.to_str().unwrap().into(),
                        mix,
                        gain_db,
                        use_nupc,
                        zero_latency_head,
                        head_taps,
                    },
                )
                .unwrap();
                processor.initialize(rate).unwrap();
                // Start playback after loading; reset also ends the deliberate
                // IR replacement fade, which is not part of the LTI transfer.
                processor.reset();
                let latency = processor.latency_samples();
                let mut output = vec![0.0_f32; reference.len() + (latency + 67) * 3];
                output[..input.len()].copy_from_slice(&input);
                let mut offset = 0;
                for frames in [1, 7, 31, 257, 1023, 2049, 64].into_iter().cycle() {
                    if offset == output.len() {
                        break;
                    }
                    let end = (offset + frames * 3).min(output.len());
                    processor
                        .process_in_place(
                            &mut output[offset..end],
                            &ProcessContext::new(rate, (end - offset) / 3),
                        )
                        .unwrap();
                    offset = end;
                }
                let gain = 10.0_f64.powf(f64::from(gain_db) / 20.0);
                let mut max_error = 0.0_f64;
                let mut squared_error = 0.0;
                for (index, &actual) in output.iter().enumerate() {
                    let expected = index.checked_sub(latency * 3).map_or(0.0, |i| {
                        let wet = reference.get(i).copied().unwrap_or(0.0);
                        let dry = input.get(i).copied().map(f64::from).unwrap_or(0.0);
                        wet * gain * f64::from(mix) + dry * f64::from(1.0 - mix)
                    });
                    let error = f64::from(actual) - expected;
                    max_error = max_error.max(error.abs());
                    squared_error += error * error;
                }
                let rms_error = (squared_error / output.len() as f64).sqrt();
                // This is an f32 convolution implementation. Require an error
                // below -100 dBFS peak and -120 dBFS RMS against the f64 sum.
                assert!(
                    max_error < 1e-5 && rms_error < 1e-6,
                    "ir={ir_frames} nupc={use_nupc} head={zero_latency_head}/{head_taps} mix={mix}: peak={max_error:e}, rms={rms_error:e}"
                );
            }
        }
    }
}

#[test]
fn tail_bound_retains_the_last_direct_convolution_tap_for_every_backend() {
    for rate in [44_100, 48_000, 96_000] {
        for ir_frames in [1, 255, 256, 257, 1023, 1024, 1025, 4097] {
            let mut impulse = vec![vec![0_i16; ir_frames]; 2];
            impulse[0][0] = 8192;
            impulse[1][0] = -4096;
            impulse[0][ir_frames - 1] = 16384;
            impulse[1][ir_frames - 1] = -8192;
            let file = write_impulse(&impulse, rate);
            for (use_nupc, zero_latency_head, head_taps) in [
                (false, false, 128),
                (true, false, 128),
                (true, true, 32),
                (true, true, 128),
                (true, true, 512),
            ] {
                let mut plugin = ConvolutionPlugin::from_params(
                    3,
                    rate,
                    ConvolutionPluginParams {
                        ir_file: file.0.to_str().unwrap().into(),
                        use_nupc,
                        zero_latency_head,
                        head_taps,
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
                let bound = latency + ir_frames - 1;
                assert_eq!(plugin.tail_length(), TailLength::Finite(bound as u64));
                let mut input = vec![0.0; 259 * 3];
                input[0..3].copy_from_slice(&[0.5, -0.25, 0.75]);
                input[258 * 3..].copy_from_slice(&[-0.75, 0.5, 0.25]);
                let reference = direct_convolution(&input, &impulse);
                let mut actual = vec![0.0; (259 + bound + 2049) * 3];
                actual[..input.len()].copy_from_slice(&input);
                let mut offset = 0;
                for frames in [1, 17, 255, 1031, 7, 509].into_iter().cycle() {
                    if offset == actual.len() {
                        break;
                    }
                    let end = (offset + frames * 3).min(actual.len());
                    plugin
                        .process_in_place(
                            &mut actual[offset..end],
                            &ProcessContext::new(rate, (end - offset) / 3),
                        )
                        .unwrap();
                    offset = end;
                }
                let max_error = actual
                    .iter()
                    .enumerate()
                    .map(|(i, &sample)| {
                        let expected = i
                            .checked_sub(latency * 3)
                            .and_then(|index| reference.get(index))
                            .copied()
                            .unwrap_or(0.0);
                        (f64::from(sample) - expected).abs()
                    })
                    .fold(0.0_f64, f64::max);
                assert!(
                    max_error < 1e-5,
                    "rate={rate}, length={ir_frames}, nupc={use_nupc}, head={head_taps}/{zero_latency_head}: {max_error:e}"
                );
                let final_frame = 258 + bound;
                assert!((actual[final_frame * 3] + 0.375).abs() < 1e-5);
                assert!(
                    actual[(final_frame + 1) * 3..]
                        .iter()
                        .all(|x| x.abs() < 1e-5)
                );
                // A bound describes the configured response, not elapsed silence.
                assert_eq!(plugin.tail_length(), TailLength::Finite(bound as u64));
            }
        }
    }
}

#[test]
fn tail_getter_does_not_allocate_on_a_fresh_thread() {
    let file = write_impulse(&[vec![16384; 4097]], 48_000);
    let plugin = ConvolutionPlugin::from_params(
        1,
        48_000,
        ConvolutionPluginParams {
            ir_file: file.0.to_str().unwrap().into(),
            ..ConvolutionPluginParams::default()
        },
    )
    .unwrap();
    let plugin = std::thread::spawn(move || {
        sotf_host::assert_no_allocs("Convolution cold tail getter", || {
            for _ in 0..32 {
                assert!(matches!(
                    std::hint::black_box(plugin.tail_length()),
                    TailLength::Finite(_)
                ));
            }
        });
        plugin
    })
    .join()
    .unwrap();
    drop(plugin);
}

#[test]
fn scalar_parameter_access_and_automation_do_not_allocate_on_a_fresh_thread() {
    use sotf_host::parameters::{ParameterId, ParameterValue};
    let plugin = ConvolutionPlugin::new(1, 48_000);
    let ids = [
        "mix",
        "gain_db",
        "use_nupc",
        "zero_latency_head",
        "head_taps",
        "missing",
    ]
    .map(ParameterId::from);
    let mix = ParameterId::from("mix");
    let gain = ParameterId::from("gain_db");
    let plugin = std::thread::spawn(move || {
        let mut plugin = plugin;
        sotf_host::assert_no_allocs("Convolution cold scalar getter", || {
            for id in &ids {
                std::hint::black_box(plugin.parametric_get_parameter(id));
            }
        });
        sotf_host::assert_no_allocs("Convolution scalar automation", || {
            plugin
                .parametric_validate_parameter(&mix, &ParameterValue::Float(0.375))
                .unwrap();
            plugin
                .parametric_set_parameter(mix, ParameterValue::Float(0.375))
                .unwrap();
            plugin
                .parametric_set_parameter(gain, ParameterValue::Float(-3.0))
                .unwrap();
        });
        plugin
    })
    .join()
    .unwrap();
    let current = plugin.current_values();
    for (key, expected) in [
        ("mix", ParameterValue::Float(0.375)),
        ("gain_db", ParameterValue::Float(-3.0)),
        ("use_nupc", ParameterValue::Bool(true)),
        ("zero_latency_head", ParameterValue::Bool(false)),
        ("head_taps", ParameterValue::Int(128)),
        ("ir_file", ParameterValue::String(String::new())),
    ] {
        let id = ParameterId::from(key);
        assert_eq!(plugin.parametric_get_parameter(&id), Some(expected.clone()));
        assert_eq!(current.get(&id), Some(&expected));
    }
}
