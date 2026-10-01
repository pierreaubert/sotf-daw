//! Dynamic band-update continuity, equivalence and lifecycle checks (R2/A2).

use super::drain_tests::count_allocs;
use super::linear_phase_eq_plugin::XFADE_FRAMES;
use super::placement_tests::CascadeRef;
use crate::{
    BandConfig, CommitRefusal, LinearPhaseEqBandPlacement as Placement, LinearPhaseEqPlugin,
    LinearPhaseEqPluginParams,
};
use sotf_host::plugin::ProcessContext;
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin};

const RATE: u32 = 48_000;
const CHANNELS: usize = 2;

fn band(
    filter_type: &str,
    frequency: f64,
    q: f64,
    gain_db: f64,
    placement: Option<Placement>,
) -> BandConfig {
    BandConfig {
        filter_type: filter_type.to_string(),
        frequency,
        q,
        gain_db,
        active: true,
        placement,
    }
}

fn params_for(bands: Vec<BandConfig>, mix: f32) -> LinearPhaseEqPluginParams {
    let num_filters = bands.len();
    LinearPhaseEqPluginParams {
        num_filters,
        fir_length_index: 0,
        phase_mode_index: 0,
        auto_gain: false,
        mix,
        filters: bands,
        stereo_pairs: None,
    }
}

fn pattern(frames: usize) -> Vec<f32> {
    (0..frames * CHANNELS)
        .map(|i| ((i * 7919 % 104729) as f32 / 104729.0 - 0.5) * 0.9)
        .collect()
}

fn stream_partitioned(
    plugin: &mut LinearPhaseEqPlugin,
    input: &[f32],
    rate: u32,
    block: usize,
) -> Vec<f32> {
    let channels = plugin.channels();
    let mut output = input.to_vec();
    let frames = input.len() / channels;
    let mut position = 0;
    while position < frames {
        let count = block.min(frames - position);
        plugin
            .process_in_place(
                &mut output[position * channels..(position + count) * channels],
                &ProcessContext::new(rate, count),
            )
            .unwrap();
        position += count;
    }
    output
}

fn drain_all(plugin: &mut LinearPhaseEqPlugin, rate: u32, capacity: usize) -> Vec<f32> {
    let channels = plugin.channels();
    let mut result = Vec::new();
    for _ in 0..20000 {
        let mut output = vec![0.0; capacity * channels];
        let drained = plugin
            .drain(&mut output, &ProcessContext::new(rate, 0))
            .unwrap();
        result.extend_from_slice(&output[..drained.frames * channels]);
        if drained.complete {
            return result;
        }
    }
    panic!("dynamic drain did not finish");
}

/// Run the canonical blend scenario: stream `prefix` frames, commit a
/// single-band change, stream the rest, and compare against twin references
/// (frozen old config, from-scratch new config) with the exact blend weights.
fn run_blend_case(
    old_bands: Vec<BandConfig>,
    band_index: usize,
    new_band: BandConfig,
    context: &str,
) {
    let total = 3000;
    let prefix = 800;
    let input = pattern(total);
    let mut new_bands = old_bands.clone();
    new_bands[band_index] = new_band.clone();

    let mut actual =
        LinearPhaseEqPlugin::from_params(CHANNELS, RATE, params_for(old_bands.clone(), 1.0))
            .unwrap();
    let mut old_ref =
        LinearPhaseEqPlugin::from_params(CHANNELS, RATE, params_for(old_bands, 1.0)).unwrap();
    let mut new_ref =
        LinearPhaseEqPlugin::from_params(CHANNELS, RATE, params_for(new_bands, 1.0)).unwrap();

    let mut actual_out = stream_partitioned(&mut actual, &input[..prefix * CHANNELS], RATE, 64);
    let mut old_out = stream_partitioned(&mut old_ref, &input[..prefix * CHANNELS], RATE, 64);
    let mut new_out = stream_partitioned(&mut new_ref, &input[..prefix * CHANNELS], RATE, 64);
    // Pre-commit output is bit-exact: same config, same input, same op order.
    assert_eq!(actual_out, old_out, "{context}: pre-commit");

    let latency_before = actual.latency_samples();
    let tail_before = actual.tail_length();
    let snapshot = actual.snapshot_config();
    let prepared =
        LinearPhaseEqPlugin::prepare_band_update(&snapshot, band_index, new_band).unwrap();
    actual.commit_prepared_update(prepared).unwrap();
    assert_eq!(actual.latency_samples(), latency_before, "{context}");
    assert_eq!(actual.tail_length(), tail_before, "{context}");
    assert!(actual.update_in_progress(), "{context}");

    actual_out.extend(stream_partitioned(
        &mut actual,
        &input[prefix * CHANNELS..],
        RATE,
        64,
    ));
    old_out.extend(stream_partitioned(
        &mut old_ref,
        &input[prefix * CHANNELS..],
        RATE,
        64,
    ));
    new_out.extend(stream_partitioned(
        &mut new_ref,
        &input[prefix * CHANNELS..],
        RATE,
        64,
    ));

    // Blend weights hit exactly 0 and 1 over XFADE_FRAMES + 1 frames; the
    // primed target matches a continuous engine within partitioned
    // block-phase rounding (~1e-6), so 1e-5 keeps a decade of margin.
    for frame in prefix..total {
        let j = frame - prefix;
        let w = if j <= XFADE_FRAMES {
            j as f64 / XFADE_FRAMES as f64
        } else {
            1.0
        };
        for ch in 0..CHANNELS {
            let index = frame * CHANNELS + ch;
            let expected =
                f64::from(old_out[index]) * (1.0 - w) + f64::from(new_out[index]) * w;
            let got = actual_out[index];
            assert!(
                (f64::from(got) - expected).abs() < 1e-5,
                "{context} frame{frame} ch{ch}: {got} vs {expected} (w={w})"
            );
        }
    }
    assert!(
        !actual.update_in_progress(),
        "{context}: blend must have completed"
    );
    assert!(actual.take_retired_route().is_some(), "{context}");
}

#[test]
fn committed_update_blends_old_to_new_without_steps() {
    run_blend_case(
        vec![
            band("Peak", 1000.0, 1.0, 0.0, None),
            band("Peak", 3000.0, 1.0, 0.0, None),
        ],
        0,
        band("Peak", 1000.0, 1.0, 9.0, None),
        "legacy",
    );
}

#[test]
fn ordered_route_update_blends_exactly() {
    run_blend_case(
        vec![
            band("Peak", 1000.0, 1.0, 0.0, Some(Placement::Left)),
            band("Lowshelf", 300.0, 1.0, 0.0, Some(Placement::Mid)),
        ],
        1,
        band("Lowshelf", 300.0, 1.0, -6.0, Some(Placement::Mid)),
        "ordered",
    );
}

fn render_with_update(block: usize) -> Vec<f32> {
    let total = 3000;
    let commit_at = 1024;
    let input = pattern(total);
    let old_bands = vec![
        band("Peak", 1000.0, 1.0, 0.0, None),
        band("Peak", 3000.0, 1.0, 0.0, None),
    ];
    let mut plugin =
        LinearPhaseEqPlugin::from_params(CHANNELS, RATE, params_for(old_bands, 1.0)).unwrap();
    let mut output = stream_partitioned(&mut plugin, &input[..commit_at * CHANNELS], RATE, block);
    let snapshot = plugin.snapshot_config();
    let prepared = LinearPhaseEqPlugin::prepare_band_update(
        &snapshot,
        0,
        band("Peak", 1000.0, 1.0, 9.0, None),
    )
    .unwrap();
    plugin.commit_prepared_update(prepared).unwrap();
    output.extend(stream_partitioned(
        &mut plugin,
        &input[commit_at * CHANNELS..],
        RATE,
        block,
    ));
    output.extend(drain_all(&mut plugin, RATE, 257));
    assert!(!plugin.update_in_progress());
    output
}

#[test]
fn update_sequence_is_block_partition_invariant() {
    // The commit frame is a multiple of every block size; per-sample op
    // order, the per-frame blend counter and partitioned convolution state
    // are all partition-independent, so renders must be bit-exact.
    let reference = render_with_update(1);
    for block in [8, 64, 512, 2048] {
        assert_eq!(render_with_update(block), reference, "block {block}");
    }
}

#[test]
fn dry_path_stays_sample_exact_through_updates() {
    // Dry-only output is a pure input delay independent of any update.
    let total = 2000;
    let commit_at = 500;
    let input = pattern(total);
    let mut plugin = LinearPhaseEqPlugin::from_params(
        CHANNELS,
        RATE,
        params_for(
            vec![
                band("Peak", 1000.0, 1.0, 0.0, None),
                band("Peak", 3000.0, 1.0, 0.0, None),
            ],
            0.0,
        ),
    )
    .unwrap();
    let latency = plugin.latency_samples();
    let mut output = stream_partitioned(&mut plugin, &input[..commit_at * CHANNELS], RATE, 63);
    let snapshot = plugin.snapshot_config();
    let prepared = LinearPhaseEqPlugin::prepare_band_update(
        &snapshot,
        0,
        band("Peak", 1000.0, 1.0, 12.0, None),
    )
    .unwrap();
    plugin.commit_prepared_update(prepared).unwrap();
    output.extend(stream_partitioned(
        &mut plugin,
        &input[commit_at * CHANNELS..],
        RATE,
        1024,
    ));
    let mut expected = vec![0.0; output.len()];
    for frame in latency..total {
        for ch in 0..CHANNELS {
            expected[frame * CHANNELS + ch] = input[(frame - latency) * CHANNELS + ch];
        }
    }
    assert_eq!(output, expected);

    // A flat retune keeps the latency-aligned impulse exactly in place.
    let mut plugin = LinearPhaseEqPlugin::from_params(
        1,
        RATE,
        params_for(vec![band("Peak", 1000.0, 1.0, 0.0, None)], 0.5),
    )
    .unwrap();
    let latency = plugin.latency_samples();
    let mut impulse = vec![0.0; 1500];
    impulse[0] = 1.0;
    let mut output = stream_partitioned(&mut plugin, &impulse[..700], RATE, 64);
    let snapshot = plugin.snapshot_config();
    let prepared = LinearPhaseEqPlugin::prepare_band_update(
        &snapshot,
        0,
        band("Peak", 2000.0, 1.0, 0.0, None),
    )
    .unwrap();
    plugin.commit_prepared_update(prepared).unwrap();
    output.extend(stream_partitioned(&mut plugin, &impulse[700..], RATE, 64));
    assert!(
        output[..latency].iter().all(|sample| sample.abs() < 1e-6),
        "a flat retune must not leak undelayed signal"
    );
    let peak = output
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .unwrap()
        .0;
    assert_eq!(peak, latency);
}

#[test]
fn updated_stream_drains_exactly() {
    let total = 2500;
    let commit_at = 200;
    let input = pattern(total);
    let old_bands = vec![
        band("Peak", 1000.0, 1.0, 0.0, None),
        band("Peak", 3000.0, 1.0, 0.0, None),
    ];
    let mut new_bands = old_bands.clone();
    new_bands[1] = band("Peak", 3000.0, 2.0, -9.0, None);
    let mut actual =
        LinearPhaseEqPlugin::from_params(CHANNELS, RATE, params_for(old_bands.clone(), 1.0))
            .unwrap();
    let mut old_ref =
        LinearPhaseEqPlugin::from_params(CHANNELS, RATE, params_for(old_bands, 1.0)).unwrap();
    let mut new_ref =
        LinearPhaseEqPlugin::from_params(CHANNELS, RATE, params_for(new_bands, 1.0)).unwrap();

    let mut actual_out = stream_partitioned(&mut actual, &input[..commit_at * CHANNELS], RATE, 64);
    let mut old_out = stream_partitioned(&mut old_ref, &input[..commit_at * CHANNELS], RATE, 64);
    let mut new_out = stream_partitioned(&mut new_ref, &input[..commit_at * CHANNELS], RATE, 64);
    assert_eq!(actual_out, old_out);
    let snapshot = actual.snapshot_config();
    let new_band = band("Peak", 3000.0, 2.0, -9.0, None);
    actual
        .commit_prepared_update(
            LinearPhaseEqPlugin::prepare_band_update(&snapshot, 1, new_band).unwrap(),
        )
        .unwrap();
    actual_out.extend(stream_partitioned(
        &mut actual,
        &input[commit_at * CHANNELS..],
        RATE,
        1024,
    ));
    old_out.extend(stream_partitioned(
        &mut old_ref,
        &input[commit_at * CHANNELS..],
        RATE,
        1024,
    ));
    new_out.extend(stream_partitioned(
        &mut new_ref,
        &input[commit_at * CHANNELS..],
        RATE,
        1024,
    ));
    actual_out.extend(drain_all(&mut actual, RATE, 257));
    old_out.extend(drain_all(&mut old_ref, RATE, 3));
    new_out.extend(drain_all(&mut new_ref, RATE, 257));

    let support = match actual.tail_length() {
        sotf_host::plugin::TailLength::Finite(frames) => frames as usize,
        _ => panic!("expected a finite tail"),
    };
    assert_eq!(actual_out.len(), (total + support) * CHANNELS);
    assert_eq!(new_out.len(), actual_out.len());
    for frame in commit_at..total + support {
        let j = frame - commit_at;
        let w = if j <= XFADE_FRAMES {
            j as f64 / XFADE_FRAMES as f64
        } else {
            1.0
        };
        for ch in 0..CHANNELS {
            let index = frame * CHANNELS + ch;
            let expected = f64::from(old_out[index]) * (1.0 - w) + f64::from(new_out[index]) * w;
            let got = actual_out[index];
            assert!(
                (f64::from(got) - expected).abs() < 1e-5,
                "frame{frame} ch{ch}: {got} vs {expected}"
            );
        }
    }
    // A second drain is complete and idempotent.
    assert!(
        actual
            .drain(&mut [], &ProcessContext::new(RATE, 0))
            .unwrap()
            .complete
    );
}

#[test]
fn refused_updates_retain_config_and_audio() {
    let old_bands = || {
        vec![
            band("Peak", 1000.0, 1.0, 0.0, None),
            band("Peak", 3000.0, 1.0, 0.0, None),
        ]
    };
    let mut plugin =
        LinearPhaseEqPlugin::from_params(CHANNELS, RATE, params_for(old_bands(), 1.0)).unwrap();
    let mut twin =
        LinearPhaseEqPlugin::from_params(CHANNELS, RATE, params_for(old_bands(), 1.0)).unwrap();
    let input = pattern(400);
    assert_eq!(
        stream_partitioned(&mut plugin, &input, RATE, 64),
        stream_partitioned(&mut twin, &input, RATE, 64)
    );

    // First update commits; a second commit while blending is refused.
    let snapshot = plugin.snapshot_config();
    plugin
        .commit_prepared_update(
            LinearPhaseEqPlugin::prepare_band_update(
                &snapshot,
                0,
                band("Peak", 1000.0, 1.0, 6.0, None),
            )
            .unwrap(),
        )
        .unwrap();
    let committed = plugin.snapshot_config();
    let remaining = plugin.xfade_remaining;
    // The twin takes the same update without any refusal attempt.
    twin
        .commit_prepared_update(
            LinearPhaseEqPlugin::prepare_band_update(
                &twin.snapshot_config(),
                0,
                band("Peak", 1000.0, 1.0, 6.0, None),
            )
            .unwrap(),
        )
        .unwrap();
    let retry = LinearPhaseEqPlugin::prepare_band_update(
        &committed,
        1,
        band("Peak", 3000.0, 1.0, 6.0, None),
    )
    .unwrap();
    assert!(
        plugin
            .commit_prepared_update(retry)
            .unwrap_err()
            .contains("already in progress")
    );
    assert_eq!(plugin.snapshot_config(), committed);
    assert_eq!(plugin.xfade_remaining, remaining);
    // Audio continues exactly as if the refusal never happened.
    let more = pattern(100);
    assert_eq!(
        stream_partitioned(&mut plugin, &more, RATE, 32),
        stream_partitioned(&mut twin, &more, RATE, 32)
    );

    // A retired route must be reclaimed before the next commit.
    let rest = pattern(600);
    stream_partitioned(&mut plugin, &rest, RATE, 512);
    stream_partitioned(&mut twin, &rest, RATE, 512);
    assert!(!plugin.update_in_progress());
    let next = LinearPhaseEqPlugin::prepare_band_update(
        &plugin.snapshot_config(),
        1,
        band("Peak", 3000.0, 1.0, 6.0, None),
    )
    .unwrap();
    assert!(
        plugin
            .commit_prepared_update(next)
            .unwrap_err()
            .contains("reclaim the retired route")
    );
    assert!(plugin.take_retired_route().is_some());
    let next = LinearPhaseEqPlugin::prepare_band_update(
        &plugin.snapshot_config(),
        1,
        band("Peak", 3000.0, 1.0, 6.0, None),
    )
    .unwrap();
    plugin.commit_prepared_update(next).unwrap();

    // A stale base snapshot is rejected after the live config advanced.
    let stale = committed;
    let current = plugin.snapshot_config();
    assert_ne!(stale, current);
    let outdated =
        LinearPhaseEqPlugin::prepare_band_update(&stale, 0, band("Peak", 500.0, 1.0, 3.0, None))
            .unwrap();
    // Finish the in-flight blend first so staleness is the only refusal cause.
    stream_partitioned(&mut plugin, &pattern(XFADE_FRAMES + 64), RATE, 1024);
    assert!(plugin.take_retired_route().is_some());
    assert!(
        plugin
            .commit_prepared_update(outdated)
            .unwrap_err()
            .contains("no longer matches")
    );
    assert_eq!(plugin.snapshot_config(), current);

    // Preparation-time errors never touch live state.
    for (index, new_band) in [
        (99, band("Peak", 1000.0, 1.0, 3.0, None)),
        (0, band("Notch", 1000.0, 1.0, 3.0, None)),
        (0, band("Peak", f64::NAN, 1.0, 3.0, None)),
        (0, band("Peak", 1000.0, 1.0, 3.0, Some(Placement::Left))),
    ] {
        assert!(
            LinearPhaseEqPlugin::prepare_band_update(&current, index, new_band).is_err(),
            "preparation must reject invalid band updates"
        );
    }
    assert_eq!(plugin.snapshot_config(), current);

    // A post-drain stream refuses commits until reset, then drains normally.
    stream_partitioned(&mut plugin, &pattern(50), RATE, 64);
    let mut partial = vec![0.0; 7 * CHANNELS];
    let first = plugin
        .drain(&mut partial, &ProcessContext::new(RATE, 0))
        .unwrap();
    assert!(!first.complete);
    let drained = LinearPhaseEqPlugin::prepare_band_update(
        &plugin.snapshot_config(),
        0,
        band("Peak", 1000.0, 1.0, 1.0, None),
    )
    .unwrap();
    assert!(
        plugin
            .commit_prepared_update(drained)
            .unwrap_err()
            .contains("before committing a band update after drain")
    );
    // The refused commit left drain state untouched: completion still works.
    let tail = drain_all(&mut plugin, RATE, 257);
    assert!(!tail.is_empty());
    plugin.reset();
    let drained = LinearPhaseEqPlugin::prepare_band_update(
        &plugin.snapshot_config(),
        0,
        band("Peak", 1000.0, 1.0, 1.0, None),
    )
    .unwrap();
    plugin.commit_prepared_update(drained).unwrap();
}

#[test]
fn reset_completes_update_with_cleared_state() {
    let old_bands = vec![
        band("Peak", 1000.0, 1.0, 0.0, None),
        band("Peak", 3000.0, 1.0, 0.0, None),
    ];
    let mut new_bands = old_bands.clone();
    new_bands[0] = band("Peak", 1000.0, 1.0, 9.0, None);
    let mut plugin =
        LinearPhaseEqPlugin::from_params(CHANNELS, RATE, params_for(old_bands, 1.0)).unwrap();
    let mut fresh =
        LinearPhaseEqPlugin::from_params(CHANNELS, RATE, params_for(new_bands, 1.0)).unwrap();
    let input = pattern(1500);
    stream_partitioned(&mut plugin, &input[..500 * CHANNELS], RATE, 64);
    let snapshot = plugin.snapshot_config();
    plugin
        .commit_prepared_update(
            LinearPhaseEqPlugin::prepare_band_update(
                &snapshot,
                0,
                band("Peak", 1000.0, 1.0, 9.0, None),
            )
            .unwrap(),
        )
        .unwrap();
    stream_partitioned(&mut plugin, &input[..100 * CHANNELS], RATE, 64);
    assert!(plugin.update_in_progress());
    plugin.reset();
    assert!(!plugin.update_in_progress());
    assert!(plugin.take_retired_route().is_some());
    // Both run the new config from cleared state: bit-exact agreement.
    assert_eq!(
        stream_partitioned(&mut plugin, &input, RATE, 127),
        stream_partitioned(&mut fresh, &input, RATE, 1024)
    );
}

#[test]
fn rate_change_discards_inflight_update() {
    let mut plugin = LinearPhaseEqPlugin::from_params(
        CHANNELS,
        RATE,
        params_for(
            vec![
                band("Peak", 1000.0, 1.0, 0.0, None),
                band("Peak", 3000.0, 1.0, 0.0, None),
            ],
            1.0,
        ),
    )
    .unwrap();
    stream_partitioned(&mut plugin, &pattern(300), RATE, 64);
    let snapshot = plugin.snapshot_config();
    plugin
        .commit_prepared_update(
            LinearPhaseEqPlugin::prepare_band_update(
                &snapshot,
                0,
                band("Peak", 1000.0, 1.0, 9.0, None),
            )
            .unwrap(),
        )
        .unwrap();
    assert!(plugin.update_in_progress());
    plugin.initialize(44_100).unwrap();
    assert!(!plugin.update_in_progress());
    assert!(plugin.take_retired_route().is_none());
    assert_eq!(plugin.sample_rate, 44_100);
    let output = stream_partitioned(&mut plugin, &pattern(500), 44_100, 64);
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert!(!drain_all(&mut plugin, 44_100, 257).is_empty());
}

#[test]
fn commit_xfade_drain_and_reset_do_not_allocate_or_free() {
    for ordered in [false, true] {
        let bands = if ordered {
            vec![
                band("Peak", 1000.0, 1.0, 0.0, Some(Placement::Left)),
                band("Lowshelf", 300.0, 1.0, 0.0, Some(Placement::Mid)),
            ]
        } else {
            vec![
                band("Peak", 1000.0, 1.0, 0.0, None),
                band("Peak", 3000.0, 1.0, 0.0, None),
            ]
        };
        let mut plugin =
            LinearPhaseEqPlugin::from_params(CHANNELS, RATE, params_for(bands, 0.375)).unwrap();
        let snapshot = plugin.snapshot_config();
        let new_band = if ordered {
            band("Lowshelf", 300.0, 1.0, -6.0, Some(Placement::Mid))
        } else {
            band("Peak", 1000.0, 1.0, 9.0, None)
        };
        let prepared =
            LinearPhaseEqPlugin::prepare_band_update(&snapshot, usize::from(ordered), new_band)
                .unwrap();
        let mut process_buf = vec![0.25; 2048 * CHANNELS];
        let mut drain_buf = vec![0.0; 257 * CHANNELS];
        let ((allocs, frees), _) = count_allocs(|| {
            plugin.commit_prepared_update(prepared).unwrap();
            // 2048 frames cover the full 513-frame blend plus post-blend audio.
            plugin
                .process_in_place(&mut process_buf, &ProcessContext::new(RATE, 2048))
                .unwrap();
            loop {
                let done = plugin
                    .drain(&mut drain_buf, &ProcessContext::new(RATE, 0))
                    .unwrap()
                    .complete;
                if done {
                    break;
                }
            }
            plugin.reset();
        });
        assert_eq!((allocs, frees), (0, 0), "ordered={ordered}");
        // Reclamation stays outside the counted section: dropping frees.
        assert!(plugin.take_retired_route().is_some());
    }
}

#[test]
fn cached_parameter_layout_matches_commit_indices() {
    let mut plugin = LinearPhaseEqPlugin::from_params(
        CHANNELS,
        RATE,
        params_for(
            vec![
                band("Peak", 1000.0, 1.0, 0.0, None),
                band("Peak", 3000.0, 1.0, 0.0, None),
            ],
            1.0,
        ),
    )
    .unwrap();
    let snapshot = plugin.snapshot_config();
    plugin
        .commit_prepared_update(
            LinearPhaseEqPlugin::prepare_band_update(
                &snapshot,
                1,
                band("Highshelf", 6000.0, 0.707, 4.5, None),
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("band_1_gain")),
        Some(ParameterValue::Float(4.5))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("band_1_type")),
        Some(ParameterValue::Int(2))
    );
    // band_1 entries sit at 5 + 1*6 + {0..4} by construction.
    let schema = plugin.parameter_schema();
    assert_eq!(schema.len(), 5 + 2 * 6);
    assert_eq!(schema[5 + 6 + 3].id.as_str(), "band_1_gain");
    assert_eq!(
        schema[5 + 6 + 3].default_value,
        ParameterValue::Float(4.5)
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("band_1_placement")),
        Some(ParameterValue::Int(0))
    );
}

#[test]
fn long_prefix_ordered_update_matches_independent_cascade_after_blend() {
    // The commit primes the full cascade support (2 stages * (1023 + 32) =
    // 2110 frames here), so with prefix 3000 the primed target reproduces a
    // continuously-run new route up to partitioned block-phase rounding.
    // Priming only N - 1 = 1023 samples would leave the oldest ~1087 samples
    // of cascade history unprimed and fail the bound below by orders of
    // magnitude. The reference shares FIR designs but convolves independently
    // (direct f64 cascade, no NUPC/FFT/block state), so this is the
    // independent post-blend + drain leg, not a twin comparison.
    let old_bands = vec![
        band("Peak", 1000.0, 1.0, 0.0, Some(Placement::Left)),
        band("Lowshelf", 300.0, 1.0, 0.0, Some(Placement::Mid)),
    ];
    let new_band = band("Lowshelf", 300.0, 1.0, -6.0, Some(Placement::Mid));
    let mut new_bands = old_bands.clone();
    new_bands[1] = new_band.clone();
    let total = 5000;
    let prefix = 3000;
    let input = pattern(total);
    let mut actual =
        LinearPhaseEqPlugin::from_params(CHANNELS, RATE, params_for(old_bands, 1.0)).unwrap();
    let mut actual_out = stream_partitioned(&mut actual, &input[..prefix * CHANNELS], RATE, 64);
    let snapshot = actual.snapshot_config();
    actual
        .commit_prepared_update(
            LinearPhaseEqPlugin::prepare_band_update(&snapshot, 1, new_band).unwrap(),
        )
        .unwrap();
    actual_out.extend(stream_partitioned(
        &mut actual,
        &input[prefix * CHANNELS..],
        RATE,
        64,
    ));
    actual_out.extend(drain_all(&mut actual, RATE, 257));
    assert!(!actual.update_in_progress());

    let fresh =
        LinearPhaseEqPlugin::from_params(CHANNELS, RATE, params_for(new_bands, 1.0)).unwrap();
    let support = match fresh.tail_length() {
        sotf_host::plugin::TailLength::Finite(frames) => frames as usize,
        _ => panic!("expected a finite tail"),
    };
    let mut reference = CascadeRef::new(&fresh, 1.0);
    let mut expected = Vec::with_capacity((total + support) * CHANNELS);
    for frame in input.as_chunks::<CHANNELS>().0 {
        expected.extend_from_slice(&reference.push_frame(frame));
    }
    for frame in vec![0.0; support * CHANNELS].chunks_exact(CHANNELS) {
        expected.extend_from_slice(&reference.push_frame(frame));
    }
    assert_eq!(actual_out.len(), (total + support) * CHANNELS);
    assert_eq!(actual_out.len(), expected.len());
    // Post-blend frames (weight exactly 1 onward, drain included) match the
    // independent cascade within 1e-5: primed-state rounding (~1e-6) plus two
    // stages of partitioned error (<= 2 * 2e-6) plus Mid/Side encode rounding.
    for frame in (prefix + XFADE_FRAMES + 1)..total + support {
        for ch in 0..CHANNELS {
            let index = frame * CHANNELS + ch;
            assert!(
                (f64::from(actual_out[index]) - f64::from(expected[index])).abs() < 1e-5,
                "frame{frame} ch{ch}: {} vs {}",
                actual_out[index],
                expected[index]
            );
        }
    }
    assert!(actual.take_retired_route().is_some());
}

#[test]
fn long_prefix_legacy_update_matches_direct_convolution_after_blend() {
    // Legacy twin of the ordered test above: prefix 3000 exceeds the legacy
    // support (1023 + 32 = 1055), so priming must cover the NUPC streaming
    // delay, not just the FIR taps. The independent leg is direct f64
    // convolution of the new design (same coefficients, independent kernel).
    let old_bands = vec![
        band("Peak", 1000.0, 1.0, 0.0, None),
        band("Peak", 3000.0, 1.0, 0.0, None),
    ];
    let mut new_bands = old_bands.clone();
    new_bands[0] = band("Peak", 1000.0, 1.0, 9.0, None);
    let total = 5000;
    let prefix = 3000;
    let input = pattern(total);
    let mut actual =
        LinearPhaseEqPlugin::from_params(CHANNELS, RATE, params_for(old_bands, 1.0)).unwrap();
    let mut actual_out = stream_partitioned(&mut actual, &input[..prefix * CHANNELS], RATE, 64);
    let snapshot = actual.snapshot_config();
    actual
        .commit_prepared_update(
            LinearPhaseEqPlugin::prepare_band_update(
                &snapshot,
                0,
                band("Peak", 1000.0, 1.0, 9.0, None),
            )
            .unwrap(),
        )
        .unwrap();
    actual_out.extend(stream_partitioned(
        &mut actual,
        &input[prefix * CHANNELS..],
        RATE,
        64,
    ));
    actual_out.extend(drain_all(&mut actual, RATE, 257));

    let fresh =
        LinearPhaseEqPlugin::from_params(CHANNELS, RATE, params_for(new_bands, 1.0)).unwrap();
    let coefficients = fresh.fir_coeffs.clone();
    let support = coefficients.len() - 1 + 32;
    assert_eq!(actual_out.len(), (total + support) * CHANNELS);
    let mut expected = vec![0.0f64; actual_out.len()];
    for frame in 0..total {
        for ch in 0..CHANNELS {
            let value = f64::from(input[frame * CHANNELS + ch]);
            for (tap, &coefficient) in coefficients.iter().enumerate() {
                expected[(frame + 32 + tap) * CHANNELS + ch] +=
                    value * f64::from(coefficient);
            }
        }
    }
    for frame in (prefix + XFADE_FRAMES + 1)..total + support {
        for ch in 0..CHANNELS {
            let index = frame * CHANNELS + ch;
            assert!(
                (f64::from(actual_out[index]) - expected[index]).abs() < 1e-5,
                "frame{frame} ch{ch}: {} vs {}",
                actual_out[index],
                expected[index]
            );
        }
    }
    assert!(actual.take_retired_route().is_some());
}

#[test]
fn chart_and_controls_report_target_while_audio_blends() {
    // Contract: at commit, live band configuration, FIR data and cached
    // scalars switch to the target immediately, so the chart-facing response
    // APIs report the target design while audio still morphs old-to-new.
    // Identical designs produce bit-identical API output.
    for ordered in [false, true] {
        let (old_bands, new_band, band_index) = if ordered {
            (
                vec![
                    band("Peak", 1000.0, 1.0, 0.0, Some(Placement::Left)),
                    band("Lowshelf", 300.0, 1.0, 0.0, Some(Placement::Mid)),
                ],
                band("Lowshelf", 300.0, 1.0, -6.0, Some(Placement::Mid)),
                1,
            )
        } else {
            (
                vec![
                    band("Peak", 1000.0, 1.0, 0.0, None),
                    band("Peak", 3000.0, 1.0, 0.0, None),
                ],
                band("Peak", 1000.0, 1.0, 9.0, None),
                0,
            )
        };
        let mut new_bands = old_bands.clone();
        new_bands[band_index] = new_band.clone();
        let mut plugin =
            LinearPhaseEqPlugin::from_params(CHANNELS, RATE, params_for(old_bands, 1.0)).unwrap();
        stream_partitioned(&mut plugin, &pattern(800), RATE, 64);
        let snapshot = plugin.snapshot_config();
        plugin
            .commit_prepared_update(
                LinearPhaseEqPlugin::prepare_band_update(&snapshot, band_index, new_band).unwrap(),
            )
            .unwrap();
        assert!(plugin.update_in_progress(), "ordered={ordered}");
        let fresh =
            LinearPhaseEqPlugin::from_params(CHANNELS, RATE, params_for(new_bands, 1.0)).unwrap();
        for channel in 0..CHANNELS {
            for frequency in [100.0, 1000.0, 8000.0] {
                assert_eq!(
                    plugin.channel_complex_response(channel, frequency),
                    fresh.channel_complex_response(channel, frequency),
                    "ordered={ordered} ch{channel} {frequency}Hz"
                );
            }
            assert_eq!(
                plugin.channel_group_delay_samples(channel, 1000.0),
                fresh.channel_group_delay_samples(channel, 1000.0),
                "ordered={ordered} ch{channel} group delay"
            );
        }
        for stage in 0..plugin.stage_count() {
            assert_eq!(
                plugin.stage_fir(stage),
                fresh.stage_fir(stage),
                "ordered={ordered} stage{stage}"
            );
        }
        // Finish the blend so the test also proves the commit was live audio,
        // not just metadata.
        stream_partitioned(&mut plugin, &pattern(XFADE_FRAMES + 64), RATE, 1024);
        assert!(!plugin.update_in_progress(), "ordered={ordered}");
        assert!(plugin.take_retired_route().is_some());
    }
}

#[test]
fn ten_band_legacy_update_blends_without_steps() {
    let old_bands: Vec<BandConfig> = (0..10)
        .map(|index| band("Peak", 500.0 + f64::from(index) * 500.0, 1.0, 0.0, None))
        .collect();
    run_blend_case(
        old_bands,
        7,
        band("Peak", 4000.0, 1.0, 6.0, None),
        "ten-band legacy",
    );
}

#[test]
fn mixed_stereo_legacy_and_placed_update_blends_without_steps() {
    run_blend_case(
        vec![
            band("Peak", 1000.0, 1.0, 0.0, None),
            band("Peak", 2000.0, 1.0, 0.0, Some(Placement::Stereo)),
            band("Lowshelf", 300.0, 1.0, 0.0, Some(Placement::Left)),
            band("Highpass", 800.0, 0.707, 0.0, Some(Placement::Side)),
        ],
        2,
        band("Lowshelf", 300.0, 1.0, -6.0, Some(Placement::Left)),
        "mixed stereo/legacy/placed",
    );
}

#[test]
fn zero_channel_commit_installs_immediately() {
    let mut plugin = LinearPhaseEqPlugin::from_params(
        0,
        RATE,
        params_for(vec![band("Peak", 1000.0, 1.0, 0.0, None)], 1.0),
    )
    .unwrap();
    let snapshot = plugin.snapshot_config();
    plugin
        .commit_prepared_update(
            LinearPhaseEqPlugin::prepare_band_update(
                &snapshot,
                0,
                band("Peak", 1000.0, 1.0, 9.0, None),
            )
            .unwrap(),
        )
        .unwrap();
    assert!(!plugin.update_in_progress());
    assert!(plugin.take_retired_route().is_some());
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("band_0_gain")),
        Some(ParameterValue::Float(9.0))
    );
}

#[test]
fn realtime_commit_success_is_allocation_free_with_populated_history() {
    for ordered in [false, true] {
        let bands = if ordered {
            vec![
                band("Peak", 1000.0, 1.0, 0.0, Some(Placement::Left)),
                band("Lowshelf", 300.0, 1.0, 0.0, Some(Placement::Mid)),
            ]
        } else {
            vec![
                band("Peak", 1000.0, 1.0, 0.0, None),
                band("Peak", 3000.0, 1.0, 0.0, None),
            ]
        };
        let mut plugin =
            LinearPhaseEqPlugin::from_params(CHANNELS, RATE, params_for(bands, 1.0)).unwrap();
        // Populate history beyond the full cascade support (1055 legacy,
        // 2110 ordered for 2-stage 1024-tap) so priming runs the full window.
        stream_partitioned(&mut plugin, &pattern(4000), RATE, 128);
        assert!(
            plugin.history_len >= 2110,
            "ordered={ordered}: history must cover full support"
        );
        let latency_before = plugin.latency_samples();
        let snapshot = plugin.snapshot_config();
        let new_band = if ordered {
            band("Lowshelf", 300.0, 1.0, -6.0, Some(Placement::Mid))
        } else {
            band("Peak", 1000.0, 1.0, 9.0, None)
        };
        let band_index = usize::from(ordered);
        let mut slot = Some(
            LinearPhaseEqPlugin::prepare_band_update(&snapshot, band_index, new_band).unwrap(),
        );
        let mut process_buf = pattern(2048);
        let mut drain_buf = vec![0.0; 257 * CHANNELS];
        let ((allocs, frees), _) = count_allocs(|| {
            plugin.try_commit_prepared_update(&mut slot).unwrap();
            assert!(slot.is_none(), "ordered={ordered}: success takes ownership");
            // 2048 frames cover the full 513-frame blend plus post-blend audio.
            plugin
                .process_in_place(&mut process_buf, &ProcessContext::new(RATE, 2048))
                .unwrap();
            loop {
                let done = plugin
                    .drain(&mut drain_buf, &ProcessContext::new(RATE, 0))
                    .unwrap()
                    .complete;
                if done {
                    break;
                }
            }
            plugin.reset();
        });
        assert_eq!((allocs, frees), (0, 0), "ordered={ordered}");
        assert_eq!(plugin.latency_samples(), latency_before);
        // Reclamation stays outside the counted section: dropping frees.
        assert!(plugin.take_retired_route().is_some());
    }
}

#[test]
fn realtime_transient_refusals_retain_and_retry_without_allocating() {
    // In-flight refusal: the second update is prepared from the committed
    // target config, refused while blending, retained, then retried with the
    // same prepared value after the blend completes and the retired route is
    // reclaimed. No preflight check avoids the error branch: the test calls
    // try_commit directly and asserts the typed refusal under the guard.
    let old_bands = vec![
        band("Peak", 1000.0, 1.0, 0.0, None),
        band("Peak", 3000.0, 1.0, 0.0, None),
    ];
    let mut plugin =
        LinearPhaseEqPlugin::from_params(CHANNELS, RATE, params_for(old_bands.clone(), 1.0))
            .unwrap();
    let mut twin =
        LinearPhaseEqPlugin::from_params(CHANNELS, RATE, params_for(old_bands, 1.0)).unwrap();
    let history = pattern(3000);
    assert_eq!(
        stream_partitioned(&mut plugin, &history, RATE, 128),
        stream_partitioned(&mut twin, &history, RATE, 128)
    );
    let first = LinearPhaseEqPlugin::prepare_band_update(
        &plugin.snapshot_config(),
        0,
        band("Peak", 1000.0, 1.0, 6.0, None),
    )
    .unwrap();
    let mut first_slot = Some(first);
    plugin.try_commit_prepared_update(&mut first_slot).unwrap();
    assert!(first_slot.is_none());
    twin
        .commit_prepared_update(
            LinearPhaseEqPlugin::prepare_band_update(
                &twin.snapshot_config(),
                0,
                band("Peak", 1000.0, 1.0, 6.0, None),
            )
            .unwrap(),
        )
        .unwrap();
    let committed = plugin.snapshot_config();
    let history_len = plugin.history_len;
    let mut retry = Some(
        LinearPhaseEqPlugin::prepare_band_update(
            &committed,
            1,
            band("Peak", 3000.0, 1.0, 6.0, None),
        )
        .unwrap(),
    );
    let ((allocs, frees), refusal) =
        count_allocs(|| plugin.try_commit_prepared_update(&mut retry));
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(refusal, Err(CommitRefusal::UpdateInProgress));
    assert!(retry.is_some(), "refusal must retain the prepared update");
    assert_eq!(plugin.snapshot_config(), committed);
    assert_eq!(plugin.history_len, history_len);
    // Audio continues exactly as if the refusal never happened.
    let more = pattern(100);
    assert_eq!(
        stream_partitioned(&mut plugin, &more, RATE, 32),
        stream_partitioned(&mut twin, &more, RATE, 32)
    );
    // Finish the blend, reclaim, then retry the same retained prepared value.
    let rest = pattern(600);
    stream_partitioned(&mut plugin, &rest, RATE, 512);
    stream_partitioned(&mut twin, &rest, RATE, 512);
    assert!(!plugin.update_in_progress());
    let mut unclaimed = retry.take();
    let ((allocs, frees), refusal) =
        count_allocs(|| plugin.try_commit_prepared_update(&mut unclaimed));
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(refusal, Err(CommitRefusal::RetiredUnclaimed));
    assert!(unclaimed.is_some(), "retired refusal must retain");
    assert!(plugin.take_retired_route().is_some());
    // The same prepared value now commits: no re-preparation needed.
    let ((allocs, frees), ()) =
        count_allocs(|| plugin.try_commit_prepared_update(&mut unclaimed).unwrap());
    assert_eq!((allocs, frees), (0, 0));
    assert!(unclaimed.is_none());
    assert!(plugin.update_in_progress());

    // Drained-stream refusal: populated history and config survive, and the
    // same prepared value commits after a reset.
    let mut plugin = LinearPhaseEqPlugin::from_params(
        CHANNELS,
        RATE,
        params_for(
            vec![
                band("Peak", 1000.0, 1.0, 0.0, None),
                band("Peak", 3000.0, 1.0, 0.0, None),
            ],
            1.0,
        ),
    )
    .unwrap();
    stream_partitioned(&mut plugin, &pattern(1500), RATE, 64);
    let before_drain = plugin.snapshot_config();
    let mut partial = vec![0.0; 7 * CHANNELS];
    assert!(
        !plugin
            .drain(&mut partial, &ProcessContext::new(RATE, 0))
            .unwrap()
            .complete
    );
    let mut drained = Some(
        LinearPhaseEqPlugin::prepare_band_update(
            &before_drain,
            0,
            band("Peak", 1000.0, 1.0, 1.0, None),
        )
        .unwrap(),
    );
    let ((allocs, frees), refusal) =
        count_allocs(|| plugin.try_commit_prepared_update(&mut drained));
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(refusal, Err(CommitRefusal::Drained));
    assert!(drained.is_some());
    assert_eq!(plugin.snapshot_config(), before_drain);
    // The refused commit left drain state untouched: completion still works.
    assert!(!drain_all(&mut plugin, RATE, 257).is_empty());
    plugin.reset();
    let ((allocs, frees), ()) =
        count_allocs(|| plugin.try_commit_prepared_update(&mut drained).unwrap());
    assert_eq!((allocs, frees), (0, 0));
    assert!(drained.is_none());

    // Stale-base refusal: the outdated prepared value is retained (not freed
    // on the commit thread) and live state is untouched; a fresh preparation
    // then commits.
    let mut plugin = LinearPhaseEqPlugin::from_params(
        CHANNELS,
        RATE,
        params_for(
            vec![
                band("Peak", 1000.0, 1.0, 0.0, None),
                band("Peak", 3000.0, 1.0, 0.0, None),
            ],
            1.0,
        ),
    )
    .unwrap();
    stream_partitioned(&mut plugin, &pattern(1500), RATE, 64);
    let stale_snapshot = plugin.snapshot_config();
    let mut first = Some(
        LinearPhaseEqPlugin::prepare_band_update(
            &stale_snapshot,
            0,
            band("Peak", 1000.0, 1.0, 6.0, None),
        )
        .unwrap(),
    );
    plugin.try_commit_prepared_update(&mut first).unwrap();
    stream_partitioned(&mut plugin, &pattern(XFADE_FRAMES + 64), RATE, 1024);
    assert!(plugin.take_retired_route().is_some());
    let current = plugin.snapshot_config();
    assert_ne!(stale_snapshot, current);
    let mut outdated = Some(
        LinearPhaseEqPlugin::prepare_band_update(
            &stale_snapshot,
            1,
            band("Peak", 3000.0, 1.0, 6.0, None),
        )
        .unwrap(),
    );
    let history_len = plugin.history_len;
    let ((allocs, frees), refusal) =
        count_allocs(|| plugin.try_commit_prepared_update(&mut outdated));
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(refusal, Err(CommitRefusal::StaleBase));
    assert!(outdated.is_some(), "stale refusal must retain ownership");
    assert_eq!(plugin.snapshot_config(), current);
    assert_eq!(plugin.history_len, history_len);
    drop(outdated);
    let mut fresh = Some(
        LinearPhaseEqPlugin::prepare_band_update(
            &current,
            1,
            band("Peak", 3000.0, 1.0, 6.0, None),
        )
        .unwrap(),
    );
    plugin.try_commit_prepared_update(&mut fresh).unwrap();
    assert!(fresh.is_none());

    // Empty slot refusal is also allocation-free.
    let mut empty: Option<crate::PreparedBandUpdate> = None;
    let ((allocs, frees), refusal) =
        count_allocs(|| plugin.try_commit_prepared_update(&mut empty));
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(refusal, Err(CommitRefusal::NoPreparedUpdate));
}

#[test]
fn realtime_topology_and_invalid_refusals_retain_without_allocating() {
    // Every invalid/topology refusal below is exercised through the real error
    // branch under an allocation guard with populated history, retains the
    // prepared update, and leaves live config and history untouched. The
    // mutations that build each invalid case run outside the guard; only the
    // refusal itself is counted.
    let setup = || {
        let mut plugin = LinearPhaseEqPlugin::from_params(
            CHANNELS,
            RATE,
            params_for(
                vec![
                    band("Peak", 1000.0, 1.0, 0.0, None),
                    band("Peak", 3000.0, 1.0, 0.0, None),
                ],
                1.0,
            ),
        )
        .unwrap();
        stream_partitioned(&mut plugin, &pattern(2500), RATE, 64);
        plugin
    };
    let prepare_valid = |plugin: &LinearPhaseEqPlugin| {
        LinearPhaseEqPlugin::prepare_band_update(
            &plugin.snapshot_config(),
            0,
            band("Peak", 1000.0, 1.0, 3.0, None),
        )
        .unwrap()
    };

    // Band index outside the live storage.
    let mut plugin = setup();
    let before = plugin.snapshot_config();
    let history_len = plugin.history_len;
    let mut invalid = Some(prepare_valid(&plugin));
    invalid.as_mut().unwrap().band_index = 99;
    let ((allocs, frees), refusal) =
        count_allocs(|| plugin.try_commit_prepared_update(&mut invalid));
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(
        refusal,
        Err(CommitRefusal::BandIndexOutOfRange { index: 99, live: 2 })
    );
    assert!(invalid.is_some());
    assert_eq!(plugin.snapshot_config(), before);
    assert_eq!(plugin.history_len, history_len);
    drop(invalid);

    // Placement drift between the prepared band and the live band.
    let mut plugin = setup();
    let before = plugin.snapshot_config();
    let mut invalid = Some(prepare_valid(&plugin));
    invalid.as_mut().unwrap().new_band.placement = Some(Placement::Left);
    let ((allocs, frees), refusal) =
        count_allocs(|| plugin.try_commit_prepared_update(&mut invalid));
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(refusal, Err(CommitRefusal::PlacementMismatch));
    assert!(invalid.is_some());
    assert_eq!(plugin.snapshot_config(), before);
    drop(invalid);

    // Invalid band parameters: non-finite frequency, out-of-range Q/gain, and
    // an unknown filter-type index each refuse without allocating.
    for mutate in [
        "frequency",
        "q",
        "gain",
        "filter_type",
    ] {
        let mut plugin = setup();
        let before = plugin.snapshot_config();
        let mut invalid = Some(prepare_valid(&plugin));
        let new_band = &mut invalid.as_mut().unwrap().new_band;
        match mutate {
            "frequency" => new_band.frequency = f64::NAN,
            "q" => new_band.q = 0.05,
            "gain" => new_band.gain_db = 30.0,
            _ => new_band.filter_type_index = 99,
        }
        let ((allocs, frees), refusal) =
            count_allocs(|| plugin.try_commit_prepared_update(&mut invalid));
        assert_eq!((allocs, frees), (0, 0), "{mutate}");
        assert_eq!(refusal, Err(CommitRefusal::InvalidBand), "{mutate}");
        assert!(invalid.is_some(), "{mutate}");
        assert_eq!(plugin.snapshot_config(), before, "{mutate}");
        drop(invalid);
    }

    // Empty stage list on the legacy route.
    let mut plugin = setup();
    let before = plugin.snapshot_config();
    let mut invalid = Some(prepare_valid(&plugin));
    invalid.as_mut().unwrap().target.stages.clear();
    let ((allocs, frees), refusal) =
        count_allocs(|| plugin.try_commit_prepared_update(&mut invalid));
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(refusal, Err(CommitRefusal::NoStage));
    assert!(invalid.is_some());
    assert_eq!(plugin.snapshot_config(), before);
    drop(invalid);

    // FIR length drift on the legacy route.
    let mut plugin = setup();
    let before = plugin.snapshot_config();
    let mut invalid = Some(prepare_valid(&plugin));
    invalid.as_mut().unwrap().target.stages[0].fir.pop();
    let ((allocs, frees), refusal) =
        count_allocs(|| plugin.try_commit_prepared_update(&mut invalid));
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(refusal, Err(CommitRefusal::FirLengthMismatch));
    assert!(invalid.is_some());
    assert_eq!(plugin.snapshot_config(), before);
    drop(invalid);

    // Stale target banks that already carry a stashed base snapshot.
    let mut plugin = setup();
    let before = plugin.snapshot_config();
    let mut invalid = Some(prepare_valid(&plugin));
    let base_clone = invalid.as_ref().unwrap().base.clone();
    invalid.as_mut().unwrap().target.stashed_base = Some(base_clone);
    let ((allocs, frees), refusal) =
        count_allocs(|| plugin.try_commit_prepared_update(&mut invalid));
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(refusal, Err(CommitRefusal::TargetNotFresh));
    assert!(invalid.is_some());
    assert_eq!(plugin.snapshot_config(), before);
    drop(invalid);

    // Ordered-route topology drift: one stage removed from a 2-stage target.
    let mut plugin = LinearPhaseEqPlugin::from_params(
        CHANNELS,
        RATE,
        params_for(
            vec![
                band("Peak", 1000.0, 1.0, 0.0, Some(Placement::Left)),
                band("Lowshelf", 300.0, 1.0, 0.0, Some(Placement::Mid)),
            ],
            1.0,
        ),
    )
    .unwrap();
    stream_partitioned(&mut plugin, &pattern(2500), RATE, 64);
    let before = plugin.snapshot_config();
    let history_len = plugin.history_len;
    let mut invalid = Some(
        LinearPhaseEqPlugin::prepare_band_update(
            &before,
            1,
            band("Lowshelf", 300.0, 1.0, -6.0, Some(Placement::Mid)),
        )
        .unwrap(),
    );
    invalid.as_mut().unwrap().target.stages.pop();
    let ((allocs, frees), refusal) =
        count_allocs(|| plugin.try_commit_prepared_update(&mut invalid));
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(refusal, Err(CommitRefusal::TopologyMismatch));
    assert!(invalid.is_some());
    assert_eq!(plugin.snapshot_config(), before);
    assert_eq!(plugin.history_len, history_len);
    drop(invalid);

    // Ordered-route FIR drift: one stage shortened, with the first stage
    // still matching, proves the length check runs before any live copy.
    let mut plugin = LinearPhaseEqPlugin::from_params(
        CHANNELS,
        RATE,
        params_for(
            vec![
                band("Peak", 1000.0, 1.0, 0.0, Some(Placement::Left)),
                band("Lowshelf", 300.0, 1.0, 0.0, Some(Placement::Mid)),
            ],
            1.0,
        ),
    )
    .unwrap();
    stream_partitioned(&mut plugin, &pattern(2500), RATE, 64);
    let live_fir_before: Vec<Vec<f32>> = plugin
        .ordered_stages
        .iter()
        .map(|stage| stage.fir.clone())
        .collect();
    let mut invalid = Some(
        LinearPhaseEqPlugin::prepare_band_update(
            &plugin.snapshot_config(),
            1,
            band("Lowshelf", 300.0, 1.0, -6.0, Some(Placement::Mid)),
        )
        .unwrap(),
    );
    invalid.as_mut().unwrap().target.stages[1].fir.pop();
    let ((allocs, frees), refusal) =
        count_allocs(|| plugin.try_commit_prepared_update(&mut invalid));
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(refusal, Err(CommitRefusal::FirLengthMismatch));
    assert!(invalid.is_some());
    let live_fir_after: Vec<Vec<f32>> = plugin
        .ordered_stages
        .iter()
        .map(|stage| stage.fir.clone())
        .collect();
    assert_eq!(live_fir_before, live_fir_after);
    drop(invalid);

    // A fresh valid update still commits after the invalid attempts.
    let mut plugin = setup();
    let mut valid = Some(prepare_valid(&plugin));
    plugin.try_commit_prepared_update(&mut valid).unwrap();
    assert!(valid.is_none());
}

#[test]
fn retry_after_inflight_refusal_uses_same_retained_update() {
    // Focused retry regression: the test never checks `update_in_progress()`
    // before attempting; it drives the real error branch, keeps the same
    // caller-owned slot, and commits that exact prepared value after the
    // first blend completes. Twin comparison proves the retained update was
    // not re-primed or corrupted by the refused attempt.
    let old_bands = vec![
        band("Peak", 1000.0, 1.0, 0.0, None),
        band("Peak", 3000.0, 1.0, 0.0, None),
    ];
    let mut plugin =
        LinearPhaseEqPlugin::from_params(CHANNELS, RATE, params_for(old_bands.clone(), 1.0))
            .unwrap();
    let mut twin =
        LinearPhaseEqPlugin::from_params(CHANNELS, RATE, params_for(old_bands, 1.0)).unwrap();
    stream_partitioned(&mut plugin, &pattern(2000), RATE, 64);
    stream_partitioned(&mut twin, &pattern(2000), RATE, 64);
    let mut first = Some(
        LinearPhaseEqPlugin::prepare_band_update(
            &plugin.snapshot_config(),
            0,
            band("Peak", 1000.0, 1.0, 6.0, None),
        )
        .unwrap(),
    );
    plugin.try_commit_prepared_update(&mut first).unwrap();
    twin
        .commit_prepared_update(
            LinearPhaseEqPlugin::prepare_band_update(
                &twin.snapshot_config(),
                0,
                band("Peak", 1000.0, 1.0, 6.0, None),
            )
            .unwrap(),
        )
        .unwrap();
    // Directly attempt the second update while blending: no preflight.
    let mut slot = Some(
        LinearPhaseEqPlugin::prepare_band_update(
            &plugin.snapshot_config(),
            1,
            band("Peak", 3000.0, 2.0, -9.0, None),
        )
        .unwrap(),
    );
    let ((allocs, frees), refusal) =
        count_allocs(|| plugin.try_commit_prepared_update(&mut slot));
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(refusal, Err(CommitRefusal::UpdateInProgress));
    assert!(slot.is_some());
    // Both routes render the first blend identically; the refusal changed
    // nothing on the plugin under test.
    let mid = pattern(600);
    assert_eq!(
        stream_partitioned(&mut plugin, &mid, RATE, 64),
        stream_partitioned(&mut twin, &mid, RATE, 64)
    );
    assert!(!plugin.update_in_progress());
    assert!(plugin.take_retired_route().is_some());
    // Retry the identical retained slot: no re-preparation, no reallocation
    // on the commit thread.
    let ((allocs, frees), ()) =
        count_allocs(|| plugin.try_commit_prepared_update(&mut slot).unwrap());
    assert_eq!((allocs, frees), (0, 0));
    assert!(slot.is_none());
    // The twin takes the same logical update through the control wrapper; the
    // two blends agree within priming rounding (1e-5, the established blend
    // bound), proving the retained target primed correctly on retry. The twin
    // rendered the same first blend, so its retired route is also unclaimed
    // and must be reclaimed before its second commit (same lifecycle rule).
    assert!(twin.take_retired_route().is_some());
    twin
        .commit_prepared_update(
            LinearPhaseEqPlugin::prepare_band_update(
                &twin.snapshot_config(),
                1,
                band("Peak", 3000.0, 2.0, -9.0, None),
            )
            .unwrap(),
        )
        .unwrap();
    let tail = pattern(1200);
    let actual = stream_partitioned(&mut plugin, &tail, RATE, 64);
    let expected = stream_partitioned(&mut twin, &tail, RATE, 64);
    assert_eq!(actual.len(), expected.len());
    for (index, (got, want)) in actual.iter().zip(expected.iter()).enumerate() {
        assert!(
            (f64::from(*got) - f64::from(*want)).abs() < 1e-5,
            "sample {index}: {got} vs {want}"
        );
    }
}
