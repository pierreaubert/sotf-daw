//! Custom decoder factory, host geometry, high ACN and reset/partition checks.

// Rust guideline compliant 2026-02-21
use sotf_plugins::{DawHost, ProcessContext, create_plugin};

#[test]
fn serialized_custom_decoders_match_direct_audio_through_the_actual_host() {
    let geometry = serde_json::json!({"name":"audit-near-polar-octahedron","speakers":[
        {"label":"front","azimuth_deg":0.0,"elevation_deg":0.0,"is_lfe":false},
        {"label":"back","azimuth_deg":180.0,"elevation_deg":0.0,"is_lfe":false},
        {"label":"left","azimuth_deg":90.0,"elevation_deg":0.0,"is_lfe":false},
        {"label":"lfe","azimuth_deg":0.0,"elevation_deg":0.0,"is_lfe":true},
        {"label":"right","azimuth_deg":-90.0,"elevation_deg":0.0,"is_lfe":false},
        {"label":"up","azimuth_deg":37.0,"elevation_deg":89.99999,"is_lfe":false},
        {"label":"down","azimuth_deg":-73.0,"elevation_deg":-89.99999,"is_lfe":false}
    ]});
    for rate in [32_000, 48_000, 192_000] {
        for order in [1, 3, 7] {
            let inputs = (order + 1) * (order + 1);
            for algorithm in ["mode_matching", "allrad"] {
                for dual_band in [false, true] {
                    let preset = serde_json::json!({"order":order,"target_layout":"custom","max_re_weighting":true,
                        "dual_band":dual_band,"algorithm":algorithm,"custom_layout":geometry});
                    let saved = serde_json::to_string(&preset).unwrap();
                    let reloaded = serde_json::from_str(&saved).unwrap();
                    let mut direct =
                        create_plugin("ambisonics_decoder", &preset, inputs, rate).unwrap();
                    let mut host = DawHost::new(inputs, rate);
                    host.add_plugin(
                        create_plugin("ambisonics_decoder", &reloaded, inputs, rate).unwrap(),
                    )
                    .unwrap();
                    host.build().unwrap();
                    let frames = 257;
                    let mut input = vec![0.0; frames * inputs];
                    for n in 0..frames {
                        input[n * inputs + n % inputs] = 0.1 * (n as f32 * 0.137).cos();
                    }
                    let mut expected = vec![0.0; frames * 7];
                    let mut actual = expected.clone();
                    direct
                        .process(&input, &mut expected, &ProcessContext::new(rate, frames))
                        .unwrap();
                    let mut offset = 0;
                    for n in [1, 31, 7, 127, 91] {
                        host.process(
                            &input[offset * inputs..(offset + n) * inputs],
                            &mut actual[offset * 7..(offset + n) * 7],
                        )
                        .unwrap();
                        offset += n;
                    }
                    assert_eq!(offset, frames);
                    assert!(
                        actual == expected,
                        "{rate} order={order} {algorithm} dual={dual_band}: factory/host audio mismatch"
                    );
                    assert!(
                        actual.as_chunks::<7>().0.iter().all(|x| x[3] == 0.0),
                        "LFE row must remain silent"
                    );
                    assert!(
                        actual.iter().any(|x| x.abs() > 0.001),
                        "exercise decoded audio"
                    );
                    host.reset();
                    direct.reset();
                    host.process(&input, &mut actual).unwrap();
                    direct
                        .process(&input, &mut expected, &ProcessContext::new(rate, frames))
                        .unwrap();
                    assert!(
                        actual == expected,
                        "reset must establish the same decoder epoch"
                    );
                }
            }
        }
    }
}
