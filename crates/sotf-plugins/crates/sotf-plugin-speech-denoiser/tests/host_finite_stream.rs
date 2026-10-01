// Rust guideline compliant 2026-02-21
use sotf_host::{DawHost, ParametricInPlacePluginAdapter};
use sotf_plugin_speech_denoiser::{SpeechDenoiserPlugin, SpeechDenoiserPluginParams};

const RATE: u32 = 48_000;
const CHANNELS: usize = 2;
const DRAIN_FRAME_CAPACITY: usize = 480;

fn host(sample_rate: u32) -> Result<DawHost, String> {
    host_with_strength(sample_rate, 1.0)
}

fn host_with_strength(sample_rate: u32, strength: f32) -> Result<DawHost, String> {
    let mut host = DawHost::new(CHANNELS, sample_rate);
    let plugin = SpeechDenoiserPlugin::from_params(
        CHANNELS,
        SpeechDenoiserPluginParams {
            enabled: true,
            strength,
            ..SpeechDenoiserPluginParams::default()
        },
    );
    host.add_plugin(Box::new(ParametricInPlacePluginAdapter::new(plugin)))?;
    Ok(host)
}

fn process(host: &mut DawHost, input: &[f32]) -> Vec<f32> {
    let frames = input.len() / CHANNELS;
    assert_eq!(input.len() % CHANNELS, 0);
    let mut output = vec![1234.0; input.len()];
    assert_eq!(host.process(input, &mut output).unwrap(), frames);
    output
}

fn drain_to_end(host: &mut DawHost) -> Vec<f32> {
    assert_eq!(host.drain_output_frames_max(), DRAIN_FRAME_CAPACITY);
    let mut output = Vec::with_capacity(2 * DRAIN_FRAME_CAPACITY * CHANNELS);
    let mut block = vec![1234.0; DRAIN_FRAME_CAPACITY * CHANNELS];
    for _ in 0..4 {
        let result = host.drain(&mut block).unwrap();
        assert!(result.frames <= DRAIN_FRAME_CAPACITY);
        output.extend_from_slice(&block[..result.frames * CHANNELS]);
        if result.complete {
            return output;
        }
        assert!(result.frames > 0);
    }
    panic!("Speech Denoiser host chain exceeded its bounded EOF drain");
}

fn signal(frames: usize) -> Vec<f32> {
    (0..frames * CHANNELS)
        .map(|index| {
            let frame = index / CHANNELS;
            let channel = index % CHANNELS;
            let time = frame as f32 / RATE as f32;
            let phase = std::f32::consts::TAU * (180.0 + 70.0 * channel as f32) * time;
            (0.23 * phase.sin() + 0.08 * (2.0 * phase).sin()) / (channel + 1) as f32
        })
        .collect()
}

#[test]
fn real_host_chain_drain_matches_ordinary_zero_continuation() {
    let input = signal(3 * DRAIN_FRAME_CAPACITY + 73);
    let zeros = vec![0.0; 960 * CHANNELS];

    let mut actual_host = host(RATE).unwrap();
    let mut actual = process(&mut actual_host, &input);
    actual.extend(drain_to_end(&mut actual_host));

    let mut reference_host = host(RATE).unwrap();
    let mut expected = process(&mut reference_host, &input);
    expected.extend(process(&mut reference_host, &zeros));

    assert_eq!(actual.len(), input.len() + zeros.len());
    assert_eq!(actual, expected);
    assert!(actual_host.drain(&mut []).unwrap().complete);
}

#[test]
fn real_host_chain_with_strength_matches_zero_continuation() {
    let input = signal(3 * DRAIN_FRAME_CAPACITY + 73);
    let zeros = vec![0.0; 960 * CHANNELS];

    let mut actual_host = host_with_strength(RATE, 0.5).unwrap();
    let mut actual = process(&mut actual_host, &input);
    actual.extend(drain_to_end(&mut actual_host));

    let mut reference_host = host_with_strength(RATE, 0.5).unwrap();
    let mut expected = process(&mut reference_host, &input);
    expected.extend(process(&mut reference_host, &zeros));

    assert_eq!(actual.len(), input.len() + zeros.len());
    assert_eq!(actual, expected);
    assert!(actual_host.drain(&mut []).unwrap().complete);
}

#[test]
fn direct_host_chain_rejects_non_48khz_without_claiming_rate_conversion() {
    assert!(matches!(
        host(44_100),
        Err(message) if message.contains("48 kHz")
    ));
}
