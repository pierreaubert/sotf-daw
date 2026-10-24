//! Center leakage and onset observations for provenance-tracked recorded sources.

// Rust guideline compliant 2026-02-21
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_upmixer::{UpmixerPlugin, UpmixerPluginParams};

const RECORDINGS: [(&str, &[u8]); 5] = [
    (
        "speech0",
        include_bytes!("../../sotf-plugin-speech-denoiser/tests/fixtures/recorded/speech-0.pcm"),
    ),
    (
        "speech1",
        include_bytes!("../../sotf-plugin-speech-denoiser/tests/fixtures/recorded/speech-1.pcm"),
    ),
    (
        "speech2",
        include_bytes!("../../sotf-plugin-speech-denoiser/tests/fixtures/recorded/speech-2.pcm"),
    ),
    (
        "violin",
        include_bytes!("../../sotf-plugin-de-esser/tests/fixtures/recorded/violin.pcm"),
    ),
    (
        "cymbal",
        include_bytes!("../../sotf-plugin-de-esser/tests/fixtures/recorded/cymbal.pcm"),
    ),
];

#[test]
fn recorded_center_sources_retain_the_existing_surround_leakage_limit() {
    let mut maximum_leakage = 0.0_f64;
    let mut cases = 0;
    for (name, bytes) in RECORDINGS {
        let source: Vec<_> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0)
            .collect();
        for layout in ["5.1", "9.1.6"] {
            for hr in [false, true] {
                let params: UpmixerPluginParams = serde_json::from_value(serde_json::json!({"fft_size":2048,"speaker_config":layout,"enable_hr_direct":hr})).unwrap();
                let mut plugin = UpmixerPlugin::from_params(params);
                plugin.initialize(48_000.0).unwrap();
                let channels = plugin.output_channels();
                let latency = plugin.latency_samples();
                let frames = source.len() + latency + 4096;
                let mut output = vec![0.0_f32; frames * channels];
                let input: Vec<_> = (0..frames)
                    .flat_map(|i| [source.get(i).copied().unwrap_or(0.0); 2])
                    .collect();
                // Include cold onset and complete source in ordinary callback sizes.
                let mut offset = 0;
                let mut call = 0;
                let pattern = [127, 512, 31, 257];
                while offset < frames {
                    let count = pattern[call % pattern.len()].min(frames - offset);
                    plugin
                        .process(
                            &input[offset * 2..(offset + count) * 2],
                            &mut output[offset * channels..(offset + count) * channels],
                            &ProcessContext::new(48_000, count),
                        )
                        .unwrap();
                    offset += count;
                    call += 1;
                }
                assert!(output.iter().all(|x| x.is_finite()));
                let energy = |range: std::ops::Range<usize>, channel: usize| -> f64 {
                    range
                        .map(|i| f64::from(output[i * channels + channel]).powi(2))
                        .sum()
                };
                let full_center = energy(latency..frames, 2);
                assert!(
                    full_center > 1e-12,
                    "missing center source {name}/{layout}/{hr}"
                );
                let full_surround = energy(latency..frames, 4);
                let ratio = full_surround / full_center;
                maximum_leakage = maximum_leakage.max(ratio);
                let onset_end = (latency + 4800).min(frames);
                let onset_center = energy(latency..onset_end, 2);
                let onset_surround = energy(latency..onset_end, 4);
                println!(
                    "RECORDED_CENTER name={name} layout={layout} hr={hr} frames={} latency={latency} center_energy={full_center:e} surround_center_ratio={ratio:e} onset_center_energy={onset_center:e} onset_surround_energy={onset_surround:e}",
                    source.len()
                );
                // Preserve the existing coherent-source SL/C energy contract.
                // Record onset energies separately; no audibility threshold is inferred.
                assert!(
                    ratio < 0.05,
                    "recorded center leakage {name}/{layout}/{hr}: {ratio}"
                );
                cases += 1;
            }
        }
    }
    println!(
        "RECORDED_CENTER_SUMMARY cases={cases} maximum_surround_center_ratio={maximum_leakage:e}"
    );
}
