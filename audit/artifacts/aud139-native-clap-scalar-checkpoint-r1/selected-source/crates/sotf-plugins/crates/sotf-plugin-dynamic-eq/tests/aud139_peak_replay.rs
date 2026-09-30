//! Clean-checkout replay of the immutable pre-AUD139 Peak audio captures.

use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_plugin_dynamic_eq::{DynamicEqPlugin, DynamicEqPluginParams};

const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: usize = 2;
const FRAMES: usize = 8_192;
const PARTITIONS: [usize; 9] = [1, 17, 251, 3, 509, 64, 1_021, 127, 5];

const INPUT: &[u8] = include_bytes!("data/aud139_pre_edit/audio/stereo-input.f32le");

const CASES: [(&str, &str, &[u8]); 6] = [
    (
        "linked_boost",
        include_str!("data/aud139_pre_edit/audio/linked_boost.params.json"),
        include_bytes!("data/aud139_pre_edit/audio/linked_boost.f32le"),
    ),
    (
        "linked_cut",
        include_str!("data/aud139_pre_edit/audio/linked_cut.params.json"),
        include_bytes!("data/aud139_pre_edit/audio/linked_cut.f32le"),
    ),
    (
        "unlinked_boost",
        include_str!("data/aud139_pre_edit/audio/unlinked_boost.params.json"),
        include_bytes!("data/aud139_pre_edit/audio/unlinked_boost.f32le"),
    ),
    (
        "unlinked_cut",
        include_str!("data/aud139_pre_edit/audio/unlinked_cut.params.json"),
        include_bytes!("data/aud139_pre_edit/audio/unlinked_cut.f32le"),
    ),
    (
        "inactive_peak",
        include_str!("data/aud139_pre_edit/audio/inactive_peak.params.json"),
        include_bytes!("data/aud139_pre_edit/audio/inactive_peak.f32le"),
    ),
    (
        "solo_peak",
        include_str!("data/aud139_pre_edit/audio/solo_peak.params.json"),
        include_bytes!("data/aud139_pre_edit/audio/solo_peak.f32le"),
    ),
];

fn decode_f32le(bytes: &[u8]) -> Vec<f32> {
    assert_eq!(bytes.len() % std::mem::size_of::<f32>(), 0);
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect()
}

fn replay(params_json: &str, input: &[f32]) -> Vec<f32> {
    let params: DynamicEqPluginParams =
        serde_json::from_str(params_json).expect("captured pre-edit params deserialize");
    let mut plugin = DynamicEqPlugin::try_from_params_at_sample_rate(CHANNELS, params, SAMPLE_RATE)
        .expect("captured pre-edit Peak params remain valid");
    let mut output = input.to_vec();
    let mut frame_offset = 0;
    let mut partition_index = 0;

    while frame_offset < FRAMES {
        let frames = PARTITIONS[partition_index % PARTITIONS.len()].min(FRAMES - frame_offset);
        let sample_offset = frame_offset * CHANNELS;
        let sample_end = sample_offset + frames * CHANNELS;
        plugin
            .process_in_place(
                &mut output[sample_offset..sample_end],
                &ProcessContext::new(SAMPLE_RATE, frames),
            )
            .expect("captured partition is accepted");
        frame_offset += frames;
        partition_index += 1;
    }
    output
}

#[test]
fn aud139_peak_vectors_replay_from_tracked_fixtures() {
    let input = decode_f32le(INPUT);
    assert_eq!(input.len(), FRAMES * CHANNELS, "fixture input length");
    assert!(input.iter().all(|sample| sample.is_finite()));

    for (name, params_json, expected_bytes) in CASES {
        let expected = decode_f32le(expected_bytes);
        let actual = replay(params_json, &input);
        assert_eq!(actual.len(), FRAMES * CHANNELS, "{name}: output length");
        assert!(
            actual.iter().all(|sample| sample.is_finite()),
            "{name}: output contains a non-finite sample"
        );
        assert_eq!(
            actual
                .iter()
                .map(|sample| sample.to_bits())
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|sample| sample.to_bits())
                .collect::<Vec<_>>(),
            "{name}: pre-edit Peak waveform changed"
        );
    }
}
