#![cfg(feature = "iamf")]

use sotf_audio::{
    EmbeddedAudioEngine, EngineConfig, PluginSettings, create_decoder,
    get_speaker_config_by_channels,
};
use sotf_plugin_ambisonics::decode_matrix::DecodeMatrix;
use std::io::Write;
use tempfile::NamedTempFile;

const TEST_SAMPLE_RATE: u32 = 48_000;
const TEST_FRAMES: u32 = 512;

fn make_aiff(channels: u16, frames: u32, sparse_channel_impulses: bool) -> NamedTempFile {
    let data_bytes = usize::from(channels) * frames as usize * 2;
    let form_size = 4 + (8 + 18) + (8 + 8 + data_bytes);
    let mut bytes = Vec::with_capacity(8 + form_size);

    bytes.extend_from_slice(b"FORM");
    bytes.extend_from_slice(&(form_size as u32).to_be_bytes());
    bytes.extend_from_slice(b"AIFF");

    bytes.extend_from_slice(b"COMM");
    bytes.extend_from_slice(&18_u32.to_be_bytes());
    bytes.extend_from_slice(&channels.to_be_bytes());
    bytes.extend_from_slice(&frames.to_be_bytes());
    bytes.extend_from_slice(&16_u16.to_be_bytes());
    // IEEE 80-bit extended encoding of 48,000 Hz.
    bytes.extend_from_slice(&[0x40, 0x0e, 0xbb, 0x80, 0, 0, 0, 0, 0, 0]);

    bytes.extend_from_slice(b"SSND");
    bytes.extend_from_slice(&((8 + data_bytes) as u32).to_be_bytes());
    bytes.extend_from_slice(&0_u32.to_be_bytes()); // offset
    bytes.extend_from_slice(&0_u32.to_be_bytes()); // block size
    for frame in 0..frames {
        for channel in 0..channels {
            let sample = if sparse_channel_impulses && frame == u32::from(channel) {
                512 + i16::try_from(channel).unwrap() * 192
            } else {
                0
            };
            bytes.extend_from_slice(&sample.to_be_bytes());
        }
    }

    let mut file = tempfile::Builder::new()
        .prefix("sotf-order7-route-")
        .suffix(".aiff")
        .tempfile()
        .unwrap();
    file.write_all(&bytes).unwrap();
    file
}

#[test]
fn sixty_four_channel_aiff_reaches_order_seven_engine_plugin() {
    let file = make_aiff(64, TEST_FRAMES, true);
    let mut decoder = create_decoder(file.path()).unwrap();
    let decoded = decoder.decode_next().unwrap().unwrap();

    assert_eq!(decoded.spec.sample_rate, TEST_SAMPLE_RATE);
    assert_eq!(decoded.spec.channels, 64);
    assert_eq!(decoded.frame_count(), TEST_FRAMES as usize);
    for channel in 0..64 {
        for (frame_index, frame) in decoded.samples.as_chunks::<64>().0.iter().enumerate() {
            let expected_pcm = if frame_index == channel {
                (512 + channel * 192) as f32 / 32768.0
            } else {
                0.0
            };
            assert!(
                (frame[channel] - expected_pcm).abs() <= 1.0 / 32768.0,
                "decoded AIFF channel {channel} frame {frame_index}: got {}, expected {expected_pcm}",
                frame[channel]
            );
        }
    }

    let ambisonics = PluginSettings::AmbisonicsDecoder {
        order: 7,
        target_layout: "9.1.6".to_string(),
        max_re_weighting: false,
        dual_band: false,
        algorithm: "allrad".to_string(),
    }
    .to_plugin_config(TEST_SAMPLE_RATE as f64);
    let config = EngineConfig {
        frame_size: TEST_FRAMES as usize,
        input_channels: 64,
        output_channels: 16,
        plugins: vec![ambisonics],
        ..EngineConfig::default()
    };
    assert!(config.validate().is_ok());

    let (mut engine, diagnostics) = EmbeddedAudioEngine::new(&config).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(engine.input_channels(), 64);
    assert_eq!(engine.output_channels(), 16);

    let speaker_config = get_speaker_config_by_channels(16).unwrap();
    let reference = DecodeMatrix::build_allrad(7, speaker_config, false).unwrap();
    assert_eq!(reference.ambi_channels, 64);
    assert_eq!(reference.speaker_count, 16);

    let mut output = vec![0.0; engine.output_frames_for_input(decoded.frame_count()) * 16];
    let written = engine.process_at(0, &decoded.samples, &mut output).unwrap();
    assert_eq!(written, TEST_FRAMES as usize);
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert!(output.iter().any(|sample| sample.abs() > 1.0e-7));

    // The channel-major input impulses are distinct and sparse. Compare the
    // complete output timeline against that quantized AIFF input multiplied
    // by the order-7 AllRAD matrix column set. The matrix construction and
    // its 64-channel coefficients are independently checked by the AUD133
    // numerical oracle suite.
    for (frame_index, input_frame) in decoded.samples.as_chunks::<64>().0.iter().enumerate() {
        for speaker in 0..16 {
            let expected: f32 = (0..64)
                .map(|channel| input_frame[channel] * reference.matrix[speaker * 64 + channel])
                .sum();
            let actual = output[frame_index * 16 + speaker];
            assert!(
                (actual - expected).abs() < 2.0e-5,
                "frame {frame_index}, output {speaker}: got {actual}, expected {expected}"
            );
        }
    }
}

#[test]
fn decoded_sixty_five_channel_aiff_is_rejected_at_engine_admission() {
    let file = make_aiff(65, 16, false);
    let mut decoder = create_decoder(file.path()).unwrap();
    assert_eq!(decoder.spec().channels, 65);
    let decoded = decoder.decode_next().unwrap().unwrap();
    assert_eq!(decoded.frame_count(), 16);
    assert!(decoded.samples.iter().all(|sample| sample.abs() < 1.0e-6));

    let config = EngineConfig {
        input_channels: usize::from(decoder.spec().channels),
        output_channels: 16,
        ..EngineConfig::default()
    };
    let error = match EmbeddedAudioEngine::new(&config) {
        Ok(_) => panic!("65-channel file unexpectedly passed engine admission"),
        Err(error) => error,
    };
    assert!(
        error
            .message
            .contains("input_channels 65 exceeds allocation-free maximum 64")
    );
}
