use super::super::dyn_eq_band_params::DynEqBandParams;
use super::super::dynamic_eq_plugin::DynamicEqPlugin;
use super::super::dynamic_eq_plugin_params::DynamicEqPluginParams;
use super::super::params::{DynEqPlacement, DynEqShape};
use sotf_host::param_specs::UpdateMode;
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;

fn make_tone(frequency: f64, sample_rate: u32, frames: usize, amplitude: f64) -> Vec<f32> {
    make_tone_from(frequency, sample_rate, 0, frames, amplitude)
}

fn make_tone_from(
    frequency: f64,
    sample_rate: u32,
    start_frame: usize,
    frames: usize,
    amplitude: f64,
) -> Vec<f32> {
    (0..frames)
        .map(|frame| {
            let absolute = start_frame + frame;
            let phase =
                2.0 * std::f64::consts::PI * frequency * absolute as f64 / sample_rate as f64;
            (amplitude * phase.sin()) as f32
        })
        .collect()
}

fn channel_rms(interleaved: &[f32], channels: usize, channel: usize) -> f64 {
    let mut sum = 0.0_f64;
    let mut count = 0_usize;
    for (index, sample) in interleaved.iter().enumerate() {
        if index % channels == channel {
            sum += f64::from(*sample).powi(2);
            count += 1;
        }
    }
    (sum / count as f64).sqrt()
}

fn routed_band(
    placement: DynEqPlacement,
    frequency: f32,
    gain: f32,
    threshold: f32,
    ratio: f32,
) -> DynEqBandParams {
    DynEqBandParams {
        shape: DynEqShape::Peak,
        placement,
        frequency,
        q: 1.0,
        gain,
        band_threshold: threshold,
        band_ratio: ratio,
        ..DynEqBandParams::default()
    }
}

fn make_routed_plugin(
    channels: usize,
    pairs: Option<Vec<[usize; 2]>>,
    bands: Vec<DynEqBandParams>,
    linked: bool,
    sample_rate: u32,
) -> DynamicEqPlugin {
    let num_bands = bands.len();
    DynamicEqPlugin::try_from_params_at_sample_rate(
        channels,
        DynamicEqPluginParams {
            num_bands,
            threshold: -60.0,
            ratio: 20.0,
            attack_ms: 0.5,
            release_ms: 20.0,
            knee: 0.0,
            link_channels: linked,
            mix: 1.0,
            bands,
            stereo_pairs: pairs,
        },
        sample_rate,
    )
    .unwrap()
}

#[test]
fn left_and_right_bands_leave_the_other_channel_bit_exact() {
    for sample_rate in [44_100_u32, 48_000, 96_000] {
        for linked in [true, false] {
            for (placement, hot_channel) in
                [(DynEqPlacement::Left, 0), (DynEqPlacement::Right, 1)]
            {
                let frames = 8_192;
                let left_tone = make_tone(1_000.0, sample_rate, frames, 0.5);
                let right_tone = make_tone(2_000.0, sample_rate, frames, 0.5);
                let mut input = vec![0.0_f32; frames * 2];
                for frame in 0..frames {
                    input[frame * 2] = left_tone[frame];
                    input[frame * 2 + 1] = right_tone[frame];
                }
                let hot_frequency = if hot_channel == 0 { 1_000.0 } else { 2_000.0 };
                let mut plugin = make_routed_plugin(
                    2,
                    None,
                    vec![routed_band(placement, hot_frequency, 12.0, -60.0, 20.0)],
                    linked,
                    sample_rate,
                );
                let mut actual = input.clone();
                plugin
                    .process_in_place(&mut actual, &ProcessContext::new(sample_rate, frames))
                    .unwrap();
                assert_eq!(actual.len(), input.len());
                assert!(actual.iter().all(|sample| sample.is_finite()));

                let quiet_channel = 1 - hot_channel;
                for frame in 0..frames {
                    assert_eq!(
                        actual[frame * 2 + quiet_channel],
                        input[frame * 2 + quiet_channel],
                        "{placement:?}, linked={linked}, Fs={sample_rate}: quiet channel touched at frame {frame}"
                    );
                }
                let hot_rms = channel_rms(&actual, 2, hot_channel);
                let hot_input_rms = channel_rms(&input, 2, hot_channel);
                assert!(
                    hot_rms > hot_input_rms * 1.5,
                    "{placement:?}, linked={linked}, Fs={sample_rate}: hot channel not boosted ({hot_rms} vs {hot_input_rms})"
                );
                assert!(
                    plugin.monitoring_gr[0] > 1.0,
                    "{placement:?}, linked={linked}, Fs={sample_rate}: detector did not engage"
                );
            }
        }
    }
}

#[test]
fn mid_band_passes_side_only_content_bit_exact_and_boosts_mid() {
    let sample_rate = 48_000_u32;
    let frames = 8_192;
    let tone = make_tone(1_000.0, sample_rate, frames, 0.5);

    // Fully correlated input carries only mid energy: both legs must rise.
    let mut correlated = vec![0.0_f32; frames * 2];
    for frame in 0..frames {
        correlated[frame * 2] = tone[frame];
        correlated[frame * 2 + 1] = tone[frame];
    }
    let mut plugin = make_routed_plugin(
        2,
        None,
        vec![routed_band(DynEqPlacement::Mid, 1_000.0, 12.0, -60.0, 20.0)],
        false,
        sample_rate,
    );
    let mut boosted = correlated.clone();
    plugin
        .process_in_place(&mut boosted, &ProcessContext::new(sample_rate, frames))
        .unwrap();
    assert!(boosted.iter().all(|sample| sample.is_finite()));
    assert!(
        channel_rms(&boosted, 2, 0) > channel_rms(&correlated, 2, 0) * 1.5,
        "mid band did not boost correlated content"
    );
    assert!(
        channel_rms(&boosted, 2, 1) > channel_rms(&correlated, 2, 1) * 1.5,
        "mid band must keep both legs moving together"
    );

    // Fully anti-correlated input carries only side energy: bit-exact rest.
    let mut anti_correlated = vec![0.0_f32; frames * 2];
    for frame in 0..frames {
        anti_correlated[frame * 2] = tone[frame];
        anti_correlated[frame * 2 + 1] = -tone[frame];
    }
    let mut plugin = make_routed_plugin(
        2,
        None,
        vec![routed_band(DynEqPlacement::Mid, 1_000.0, 12.0, -60.0, 20.0)],
        false,
        sample_rate,
    );
    let mut actual = anti_correlated.clone();
    plugin
        .process_in_place(&mut actual, &ProcessContext::new(sample_rate, frames))
        .unwrap();
    assert_eq!(actual, anti_correlated, "mid band touched side-only content");
    assert!(
        plugin.monitoring_gr[0].abs() < 1.0e-6,
        "mid detector saw side-only content: {} dB",
        plugin.monitoring_gr[0]
    );
}

#[test]
fn side_band_passes_correlated_content_bit_exact_and_boosts_side() {
    let sample_rate = 48_000_u32;
    let frames = 8_192;
    let tone = make_tone(1_000.0, sample_rate, frames, 0.5);

    let mut anti_correlated = vec![0.0_f32; frames * 2];
    for frame in 0..frames {
        anti_correlated[frame * 2] = tone[frame];
        anti_correlated[frame * 2 + 1] = -tone[frame];
    }
    let mut plugin = make_routed_plugin(
        2,
        None,
        vec![routed_band(DynEqPlacement::Side, 1_000.0, 12.0, -60.0, 20.0)],
        false,
        sample_rate,
    );
    let mut boosted = anti_correlated.clone();
    plugin
        .process_in_place(&mut boosted, &ProcessContext::new(sample_rate, frames))
        .unwrap();
    assert!(boosted.iter().all(|sample| sample.is_finite()));
    assert!(
        channel_rms(&boosted, 2, 0) > channel_rms(&anti_correlated, 2, 0) * 1.5,
        "side band did not boost anti-correlated content"
    );

    let mut correlated = vec![0.0_f32; frames * 2];
    for frame in 0..frames {
        correlated[frame * 2] = tone[frame];
        correlated[frame * 2 + 1] = tone[frame];
    }
    let mut plugin = make_routed_plugin(
        2,
        None,
        vec![routed_band(DynEqPlacement::Side, 1_000.0, 12.0, -60.0, 20.0)],
        false,
        sample_rate,
    );
    let mut actual = correlated.clone();
    plugin
        .process_in_place(&mut actual, &ProcessContext::new(sample_rate, frames))
        .unwrap();
    assert_eq!(actual, correlated, "side band touched correlated content");
    assert!(
        plugin.monitoring_gr[0].abs() < 1.0e-6,
        "side detector saw correlated content: {} dB",
        plugin.monitoring_gr[0]
    );
}

#[test]
fn explicit_pairs_route_each_pair_and_pair_order_is_deterministic() {
    let sample_rate = 48_000_u32;
    let channels = 4;
    let frames = 8_192;
    // Pair [0, 3] carries a loud 1 kHz tone; pair [1, 2] a quiet one.
    let loud = make_tone(1_000.0, sample_rate, frames, 0.5);
    let quiet = make_tone(1_000.0, sample_rate, frames, 0.02);
    let mut input = vec![0.0_f32; frames * channels];
    for frame in 0..frames {
        input[frame * channels] = loud[frame];
        input[frame * channels + 1] = quiet[frame];
        input[frame * channels + 2] = quiet[frame];
        input[frame * channels + 3] = loud[frame];
    }

    let render = |pairs: Vec<[usize; 2]>, linked: bool| {
        let mut plugin = make_routed_plugin(
            channels,
            Some(pairs),
            vec![routed_band(DynEqPlacement::Left, 1_000.0, 12.0, -30.0, 20.0)],
            linked,
            sample_rate,
        );
        let mut audio = input.clone();
        plugin
            .process_in_place(&mut audio, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        (audio, plugin.monitoring_gr[0])
    };

    // Unlinked: only the loud pair's left channel (channel 0) engages.
    let (unlinked, _) = render(vec![[0, 3], [1, 2]], false);
    assert!(unlinked.iter().all(|sample| sample.is_finite()));
    assert!(
        channel_rms(&unlinked, channels, 0) > channel_rms(&input, channels, 0) * 1.5,
        "loud pair left channel did not engage"
    );
    for channel in [1, 2, 3] {
        for frame in 0..frames {
            assert_eq!(
                unlinked[frame * channels + channel],
                input[frame * channels + channel],
                "unlinked: channel {channel} touched at frame {frame}"
            );
        }
    }

    // Linked: the loud pair drives the shared envelope, so the quiet pair's
    // left channel audibly moves too. This compares emitted audio, not meters.
    let (linked, _) = render(vec![[0, 3], [1, 2]], true);
    assert!(linked.iter().all(|sample| sample.is_finite()));
    let quiet_pair_movement = (0..frames)
        .map(|frame| {
            (f64::from(linked[frame * channels + 1]) - f64::from(input[frame * channels + 1]))
                .abs()
        })
        .fold(0.0_f64, f64::max);
    assert!(
        quiet_pair_movement > 1.0e-3,
        "linked mode did not share detection across pairs: {quiet_pair_movement}"
    );
    for channel in [2, 3] {
        for frame in 0..frames {
            assert_eq!(
                linked[frame * channels + channel],
                input[frame * channels + channel],
                "linked: right channel {channel} touched at frame {frame}"
            );
        }
    }

    // Pair order must not change the deterministic result.
    let (reordered, _) = render(vec![[1, 2], [0, 3]], false);
    assert_eq!(reordered, unlinked, "pair order changed the output");
    let (reordered_linked, _) = render(vec![[1, 2], [0, 3]], true);
    assert_eq!(reordered_linked, linked, "pair order changed linked output");
}

#[test]
fn disjoint_routed_bands_commute_and_repeat_bit_exactly() {
    let sample_rate = 48_000_u32;
    let frames = 8_192;
    let left_tone = make_tone(1_000.0, sample_rate, frames, 0.5);
    let right_tone = make_tone(3_000.0, sample_rate, frames, 0.5);
    let mut input = vec![0.0_f32; frames * 2];
    for frame in 0..frames {
        input[frame * 2] = left_tone[frame];
        input[frame * 2 + 1] = right_tone[frame];
    }

    let render = |bands: Vec<DynEqBandParams>| {
        let mut plugin = make_routed_plugin(2, None, bands, false, sample_rate);
        let mut audio = input.clone();
        plugin
            .process_in_place(&mut audio, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        audio
    };

    let band_left = routed_band(DynEqPlacement::Left, 1_000.0, 12.0, -60.0, 20.0);
    let band_right = routed_band(DynEqPlacement::Right, 3_000.0, -12.0, -60.0, 20.0);
    let forward = render(vec![band_left.clone(), band_right.clone()]);
    let swapped = render(vec![band_right, band_left]);
    assert!(forward.iter().all(|sample| sample.is_finite()));
    assert_eq!(
        forward, swapped,
        "disjoint routed bands must commute across band indices"
    );
    // Determinism: the identical configuration repeats bit-exactly.
    let repeat = render(vec![
        routed_band(DynEqPlacement::Left, 1_000.0, 12.0, -60.0, 20.0),
        routed_band(DynEqPlacement::Right, 3_000.0, -12.0, -60.0, 20.0),
    ]);
    assert_eq!(forward, repeat, "repeated render differed");

    // Irregular callback partitioning must not change routed audio.
    let mut plugin = make_routed_plugin(
        2,
        None,
        vec![
            routed_band(DynEqPlacement::Mid, 1_000.0, 9.0, -40.0, 6.0),
            routed_band(DynEqPlacement::Side, 3_000.0, -9.0, -40.0, 6.0),
        ],
        true,
        sample_rate,
    );
    let mut contiguous = input.clone();
    plugin
        .process_in_place(&mut contiguous, &ProcessContext::new(sample_rate, frames))
        .unwrap();
    let mut plugin = make_routed_plugin(
        2,
        None,
        vec![
            routed_band(DynEqPlacement::Mid, 1_000.0, 9.0, -40.0, 6.0),
            routed_band(DynEqPlacement::Side, 3_000.0, -9.0, -40.0, 6.0),
        ],
        true,
        sample_rate,
    );
    let mut partitioned = input.clone();
    let partitions = [1, 31, 509, 7, 1_021, 64, 3];
    let mut frame_offset = 0;
    let mut partition_index = 0;
    while frame_offset < frames {
        let block_frames =
            partitions[partition_index % partitions.len()].min(frames - frame_offset);
        let start = frame_offset * 2;
        let end = start + block_frames * 2;
        plugin
            .process_in_place(
                &mut partitioned[start..end],
                &ProcessContext::new(sample_rate, block_frames),
            )
            .unwrap();
        frame_offset += block_frames;
        partition_index += 1;
    }
    assert_eq!(contiguous, partitioned, "routed partition run differed");
}

#[test]
fn threshold_automation_moves_routed_audio_at_all_rates() {
    for sample_rate in [44_100_u32, 48_000, 96_000] {
        let frames = (sample_rate as usize / 6).min(16_000);
        let tone = make_tone(1_000.0, sample_rate, frames, 0.5);
        let mut input = vec![0.0_f32; frames * 2];
        for frame in 0..frames {
            input[frame * 2] = tone[frame];
            input[frame * 2 + 1] = tone[frame];
        }
        // Release-settling extension: the pre-automation envelope rides near
        // ~50 dB GR (threshold -60, ratio 20 on a -7 dB detected tone), about
        // 4x the 12 dB blend ceiling, so desaturation alone costs
        // 20*ln(50/12) ~= 29 ms before the audible blend even starts
        // decaying. Measuring inside the original second half therefore
        // reads ~5-9% residual blend, not dry audio. The extra 150 ms of
        // phase-continuous tone puts the measured quarter ~200 ms past the
        // automation point, eight-plus 20 ms release taus past desaturation.
        let extra_frames = sample_rate as usize * 3 / 20;
        let extra_tone = make_tone_from(1_000.0, sample_rate, frames, extra_frames, 0.5);
        let mut extra_input = vec![0.0_f32; extra_frames * 2];
        for frame in 0..extra_frames {
            extra_input[frame * 2] = extra_tone[frame];
            extra_input[frame * 2 + 1] = extra_tone[frame];
        }

        for linked in [true, false] {
            let mut plugin = make_routed_plugin(
                2,
                None,
                vec![routed_band(DynEqPlacement::Left, 1_000.0, 12.0, -60.0, 20.0)],
                linked,
                sample_rate,
            );
            let half = frames / 2;
            let mut first = input[..half * 2].to_vec();
            plugin
                .process_in_place(&mut first, &ProcessContext::new(sample_rate, half))
                .unwrap();
            // Automate the live band threshold above the tone level: the band
            // must release back toward dry while the right leg stays exact.
            plugin
                .set_parameter(
                    ParameterId::from("band_0_threshold"),
                    ParameterValue::Float(-5.0),
                )
                .unwrap();
            let mut second = input[half * 2..].to_vec();
            plugin
                .process_in_place(
                    &mut second,
                    &ProcessContext::new(sample_rate, frames - half),
                )
                .unwrap();
            let mut extra = extra_input.clone();
            plugin
                .process_in_place(&mut extra, &ProcessContext::new(sample_rate, extra_frames))
                .unwrap();
            assert!(first.iter().all(|sample| sample.is_finite()));
            assert!(second.iter().all(|sample| sample.is_finite()));
            assert!(extra.iter().all(|sample| sample.is_finite()));

            for frame in 0..half {
                assert_eq!(
                    first[frame * 2 + 1],
                    input[frame * 2 + 1],
                    "Fs={sample_rate}, linked={linked}: right leg touched before automation"
                );
            }
            for frame in 0..frames - half {
                assert_eq!(
                    second[frame * 2 + 1],
                    input[(half + frame) * 2 + 1],
                    "Fs={sample_rate}, linked={linked}: right leg touched after automation"
                );
            }
            for frame in 0..extra_frames {
                assert_eq!(
                    extra[frame * 2 + 1],
                    extra_input[frame * 2 + 1],
                    "Fs={sample_rate}, linked={linked}: right leg touched in settling extension"
                );
            }
            // Engaged half: the 0.5 ms attack settles immediately, so the
            // whole half measures full engagement.
            let engaged_rms = channel_rms(&first, 2, 0);
            let engaged_input_rms = channel_rms(&input[..half * 2], 2, 0);
            assert!(
                engaged_rms > engaged_input_rms * 1.5,
                "Fs={sample_rate}, linked={linked}: engaged half not boosted"
            );
            // Released tail: settled final quarter of the extension (frame
            // aligned), about eight release taus past desaturation, so the
            // decaying blend cannot dominate. Bound unchanged: 5% RMS.
            let tail_frames = extra_frames / 4;
            let released_rms = channel_rms(&extra[extra.len() - tail_frames * 2..], 2, 0);
            let released_input_rms =
                channel_rms(&extra_input[extra_input.len() - tail_frames * 2..], 2, 0);
            assert!(
                (released_rms / released_input_rms - 1.0).abs() < 0.05,
                "Fs={sample_rate}, linked={linked}: released tail not dry ({released_rms} vs {released_input_rms})"
            );
        }
    }
}

#[test]
fn placement_is_structural_and_validated() {
    let mut plugin = make_routed_plugin(
        2,
        None,
        vec![routed_band(DynEqPlacement::Stereo, 1_000.0, 6.0, -30.0, 4.0)],
        false,
        48_000,
    );
    assert_eq!(plugin.bands[0].placement, DynEqPlacement::Stereo);

    // Metadata exposes the appended placement control as structural.
    let placement = plugin
        .parameters()
        .into_iter()
        .find(|parameter| parameter.id.as_str() == "band_0_placement")
        .expect("band_0_placement must be registered");
    assert_eq!(placement.update_mode, UpdateMode::Structural);
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("band_0_placement")),
        Some(ParameterValue::Int(0))
    );
    assert!(
        plugin
            .current_values()
            .keys()
            .any(|id| id == &ParameterId::from("band_0_placement")),
        "band_0_placement must be saved in current values"
    );

    // Live placement edits are refused as structural; state is preserved.
    let error = plugin
        .set_parameter(ParameterId::from("band_0_placement"), ParameterValue::Int(3))
        .unwrap_err();
    assert!(error.contains("structural"), "unexpected error: {error}");
    assert_eq!(plugin.bands[0].placement, DynEqPlacement::Stereo);

    // Out-of-range choice indices fail validation instead.
    let error = plugin
        .set_parameter(ParameterId::from("band_0_placement"), ParameterValue::Int(5))
        .unwrap_err();
    assert!(!error.contains("structural"), "unexpected error: {error}");
    assert_eq!(plugin.bands[0].placement, DynEqPlacement::Stereo);

    // Tilt extended the shape range: index 3 is structural, 4 is invalid.
    let error = plugin
        .set_parameter(ParameterId::from("band_0_shape"), ParameterValue::Int(3))
        .unwrap_err();
    assert!(error.contains("structural"), "unexpected error: {error}");
    let error = plugin
        .set_parameter(ParameterId::from("band_0_shape"), ParameterValue::Int(4))
        .unwrap_err();
    assert!(!error.contains("structural"), "unexpected error: {error}");
}

#[test]
fn invalid_pair_geometry_is_rejected_and_clamping_falls_back() {
    let bands = || vec![routed_band(DynEqPlacement::Left, 1_000.0, 6.0, -30.0, 4.0)];
    let stereo_bands = || vec![routed_band(DynEqPlacement::Stereo, 1_000.0, 6.0, -30.0, 4.0)];

    // Overlapping, out-of-range, and degenerate pairs are rejected.
    for pairs in [
        Some(vec![[0, 1], [1, 0]]),
        Some(vec![[0, 0]]),
        Some(vec![[0, 4]]),
        Some(vec![]),
    ] {
        let params = DynamicEqPluginParams {
            num_bands: 1,
            bands: bands(),
            stereo_pairs: pairs,
            ..DynamicEqPluginParams::default()
        };
        assert!(
            DynamicEqPlugin::try_from_params_at_sample_rate(4, params, 48_000).is_err(),
            "invalid geometry must be rejected"
        );
    }
    // Routed bands need pairs: missing on 4ch and impossible on mono fail.
    for channels in [1, 4] {
        let params = DynamicEqPluginParams {
            num_bands: 1,
            bands: bands(),
            stereo_pairs: None,
            ..DynamicEqPluginParams::default()
        };
        assert!(
            DynamicEqPlugin::try_from_params_at_sample_rate(channels, params, 48_000).is_err(),
            "routed band on {channels}ch without pairs must be rejected"
        );
    }
    // Stereo bands never need pairs, on any channel count.
    for channels in [1, 2, 4] {
        let params = DynamicEqPluginParams {
            num_bands: 1,
            bands: stereo_bands(),
            stereo_pairs: None,
            ..DynamicEqPluginParams::default()
        };
        assert!(
            DynamicEqPlugin::try_from_params_at_sample_rate(channels, params, 48_000).is_ok(),
            "stereo band on {channels}ch must not need pairs"
        );
    }
    // The infallible constructor falls back to the channel default instead.
    let fallback = DynamicEqPlugin::from_params(
        2,
        DynamicEqPluginParams {
            num_bands: 1,
            bands: bands(),
            stereo_pairs: Some(vec![[0, 0]]),
            ..DynamicEqPluginParams::default()
        },
    );
    assert_eq!(fallback.stereo_pairs, vec![[0, 1]]);
}

#[test]
fn failed_tilt_reinitialize_preserves_populated_state_and_retries() {
    let original_rate = 48_000;
    let invalid_rate = 8_000;
    let make = |rate: u32| {
        DynamicEqPlugin::try_from_params_at_sample_rate(
            1,
            DynamicEqPluginParams {
                num_bands: 1,
                threshold: -30.0,
                ratio: 4.0,
                attack_ms: 2.0,
                release_ms: 60.0,
                knee: 0.0,
                link_channels: false,
                mix: 1.0,
                bands: vec![DynEqBandParams {
                    shape: DynEqShape::Tilt,
                    frequency: 10_000.0,
                    gain: 9.0,
                    ..DynEqBandParams::default()
                }],
                stereo_pairs: None,
            },
            rate,
        )
        .unwrap()
    };
    let mut candidate = make(original_rate);
    let mut twin = make(original_rate);

    let prefix = make_tone(500.0, original_rate, 4_096, 0.4);
    let mut candidate_prefix = prefix.clone();
    let mut twin_prefix = prefix.clone();
    candidate
        .process_in_place(&mut candidate_prefix, &ProcessContext::new(original_rate, 4_096))
        .unwrap();
    twin.process_in_place(&mut twin_prefix, &ProcessContext::new(original_rate, 4_096))
        .unwrap();
    assert!(candidate_prefix.iter().all(|sample| sample.is_finite()));
    assert_ne!(candidate_prefix, prefix);
    assert_eq!(candidate_prefix, twin_prefix);

    let values_before = candidate.current_values();
    let pivot_before = candidate.bands[0].frequency;
    let result = candidate.initialize(f64::from(invalid_rate));
    assert!(result.is_err(), "10 kHz tilt pivot must reject 8 kHz");
    assert_eq!(candidate.sample_rate, f64::from(original_rate));
    assert_eq!(candidate.bands[0].frequency, pivot_before);
    assert_eq!(candidate.current_values(), values_before);

    let suffix = make_tone(500.0, original_rate, 2_048, 0.4);
    let mut candidate_suffix = suffix.clone();
    let mut twin_suffix = suffix.clone();
    candidate
        .process_in_place(&mut candidate_suffix, &ProcessContext::new(original_rate, 2_048))
        .unwrap();
    twin.process_in_place(&mut twin_suffix, &ProcessContext::new(original_rate, 2_048))
        .unwrap();
    assert_eq!(candidate_suffix, twin_suffix);

    candidate.initialize(44_100.0).unwrap();
    let mut fresh = make(44_100);
    let retry = make_tone(500.0, 44_100, 2_048, 0.4);
    let mut candidate_retry = retry.clone();
    let mut fresh_retry = retry.clone();
    candidate
        .process_in_place(&mut candidate_retry, &ProcessContext::new(44_100, 2_048))
        .unwrap();
    fresh
        .process_in_place(&mut fresh_retry, &ProcessContext::new(44_100, 2_048))
        .unwrap();
    assert_eq!(candidate_retry, fresh_retry);
}

#[test]
fn routed_band_without_pairs_is_a_documented_silent_bypass_on_the_infallible_path() {
    // `from_params` keeps its legacy infallible contract: on 4 channels with
    // no explicit geometry the pair list is empty, so a routed band consumes
    // its slot while leaving the audio bit-identical and reporting 0 dB GR.
    // The strict constructors reject this same configuration instead.
    let channels = 4;
    let sample_rate = 44_100_u32;
    let frames = 4_096;
    let mut plugin = DynamicEqPlugin::from_params(
        channels,
        DynamicEqPluginParams {
            num_bands: 1,
            bands: vec![routed_band(DynEqPlacement::Left, 1_000.0, 12.0, -60.0, 20.0)],
            stereo_pairs: None,
            ..DynamicEqPluginParams::default()
        },
    );
    assert!(
        plugin.stereo_pairs.is_empty(),
        "4ch infallible construction without pairs must keep an empty pair list"
    );

    // Loud in-band tone on every channel: anything but a bypass would move it.
    let tone = make_tone(1_000.0, sample_rate, frames, 0.5);
    let mut input = vec![0.0_f32; frames * channels];
    for frame in 0..frames {
        for channel in 0..channels {
            input[frame * channels + channel] = tone[frame];
        }
    }
    let mut actual = input.clone();
    plugin
        .process_in_place(&mut actual, &ProcessContext::new(sample_rate, frames))
        .unwrap();
    assert_eq!(
        actual, input,
        "empty-pairs routed band must bypass bit-exactly, not engage"
    );
    assert_eq!(
        plugin.monitoring_gr[0], 0.0,
        "empty-pairs routed band must report exactly 0 dB GR"
    );

    // The strict constructor rejects the same configuration (fail-fast).
    let strict_params = DynamicEqPluginParams {
        num_bands: 1,
        bands: vec![routed_band(DynEqPlacement::Left, 1_000.0, 12.0, -60.0, 20.0)],
        stereo_pairs: None,
        ..DynamicEqPluginParams::default()
    };
    assert!(
        DynamicEqPlugin::try_from_params_at_sample_rate(channels, strict_params, sample_rate)
            .is_err(),
        "strict construction must reject a routed band without pairs"
    );
}
