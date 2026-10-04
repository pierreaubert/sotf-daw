//! Ordered-route exactness, refusal and compatibility checks (R1/A3).

use crate::{
    BandConfig, LinearPhaseEqBandPlacement as Placement, LinearPhaseEqPlugin,
    LinearPhaseEqPluginParams,
};
use sotf_host::plugin::{ProcessContext, TailLength};
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin};

/// Independent f64 reference for the ordered cascade: direct convolution per
/// stage with the stored design coefficients, f64 Mid/Side math and analytic
/// dry alignment. Independent of NUPC partition queues, FFTs, block
/// partitioning and the drain implementation. Shared with the dynamic-update
/// tests, which compare post-blend output against it (same FIR designs, fully
/// independent convolution path).
pub(crate) struct CascadeRef {
    firs: Vec<Vec<f64>>,
    placements: Vec<Placement>,
    pairs: Vec<[usize; 2]>,
    channels: usize,
    identity: Vec<f64>,
    mix: f64,
    latency: usize,
    histories: Vec<Vec<Vec<f64>>>,
    inputs: Vec<Vec<f64>>,
}

impl CascadeRef {
    pub(crate) fn new(plugin: &LinearPhaseEqPlugin, mix: f64) -> Self {
        let firs = plugin
            .ordered_stages
            .iter()
            .map(|stage| stage.fir.iter().map(|&tap| f64::from(tap)).collect())
            .collect::<Vec<Vec<f64>>>();
        let placements = plugin
            .ordered_stages
            .iter()
            .map(|stage| stage.placement)
            .collect();
        let stages = firs.len();
        let channels = plugin.channels();
        Self {
            firs,
            placements,
            pairs: plugin.stereo_pairs.clone(),
            channels,
            identity: plugin
                .identity_fir
                .iter()
                .map(|&tap| f64::from(tap))
                .collect(),
            mix,
            latency: plugin.latency_samples(),
            histories: vec![vec![Vec::new(); channels]; stages],
            inputs: Vec::new(),
        }
    }

    pub(crate) fn push_frame(&mut self, input: &[f32]) -> Vec<f32> {
        debug_assert_eq!(input.len(), self.channels);
        let mut frame: Vec<f64> = input.iter().map(|&s| f64::from(s)).collect();
        self.inputs.push(frame.clone());
        for stage in 0..self.firs.len() {
            for (channel, sample) in frame.iter().enumerate().take(self.channels) {
                self.histories[stage][channel].push(*sample);
            }
            frame = self.stage_output(stage);
        }
        let t = self.inputs.len() - 1;
        let mut output = vec![0.0f32; self.channels];
        for channel in 0..self.channels {
            let dry = if t >= self.latency {
                self.inputs[t - self.latency][channel]
            } else {
                0.0
            };
            output[channel] = (dry * (1.0 - self.mix) + frame[channel] * self.mix) as f32;
        }
        output
    }

    fn stage_output(&self, stage: usize) -> Vec<f64> {
        let t = self.histories[stage][0].len() - 1;
        let fir = &self.firs[stage];
        let mut output = vec![0.0; self.channels];
        match self.placements[stage] {
            Placement::Stereo => {
                for (channel, slot) in output.iter_mut().enumerate().take(self.channels) {
                    *slot = conv(&self.histories[stage][channel], fir, t);
                }
            }
            Placement::Left | Placement::Right => {
                let left_selected = self.placements[stage] == Placement::Left;
                for (channel, slot) in output.iter_mut().enumerate().take(self.channels) {
                    let selected = self.pairs.iter().any(|[left, right]| {
                        (left_selected && *left == channel)
                            || (!left_selected && *right == channel)
                    });
                    *slot = conv(
                        &self.histories[stage][channel],
                        if selected { fir } else { &self.identity },
                        t,
                    );
                }
            }
            Placement::Mid | Placement::Side => {
                let mid_selected = self.placements[stage] == Placement::Mid;
                for [left, right] in &self.pairs {
                    let (left, right) = (*left, *right);
                    let mut mid_out = 0.0;
                    let mut side_out = 0.0;
                    for (k, &tap) in fir.iter().enumerate() {
                        let index = t as isize - 32 - k as isize;
                        if index < 0 {
                            continue;
                        }
                        let index = index as usize;
                        let mid = 0.5
                            * (self.histories[stage][left][index]
                                + self.histories[stage][right][index]);
                        let side = 0.5
                            * (self.histories[stage][left][index]
                                - self.histories[stage][right][index]);
                        if mid_selected {
                            mid_out += tap * mid;
                            side_out += self.identity[k] * side;
                        } else {
                            mid_out += self.identity[k] * mid;
                            side_out += tap * side;
                        }
                    }
                    output[left] = mid_out + side_out;
                    output[right] = mid_out - side_out;
                }
                for (channel, slot) in output.iter_mut().enumerate().take(self.channels) {
                    if self
                        .pairs
                        .iter()
                        .any(|[l, r]| *l == channel || *r == channel)
                    {
                        continue;
                    }
                    *slot = conv(&self.histories[stage][channel], &self.identity, t);
                }
            }
        }
        output
    }
}

fn conv(history: &[f64], fir: &[f64], t: usize) -> f64 {
    let mut acc = 0.0;
    for (k, &h) in fir.iter().enumerate() {
        let index = t as isize - 32 - k as isize;
        if index >= 0 {
            acc += h * history[index as usize];
        }
    }
    acc
}

fn stream_all(plugin: &mut LinearPhaseEqPlugin, input: &[f32], rate: u32) -> Vec<f32> {
    let channels = plugin.channels();
    let frames = input.len() / channels;
    let mut result = input.to_vec();
    let mut position = 0;
    for block in [1, 31, 127, 7, 1031].into_iter().cycle() {
        let count = block.min(frames - position);
        if count == 0 {
            break;
        }
        assert_eq!(
            plugin
                .process_in_place(
                    &mut result[position * channels..(position + count) * channels],
                    &ProcessContext::new(rate, count)
                )
                .unwrap(),
            count
        );
        position += count;
    }
    result
}

fn drain_all(plugin: &mut LinearPhaseEqPlugin, rate: u32, capacity: usize) -> Vec<f32> {
    let channels = plugin.channels();
    let mut result = Vec::new();
    for _ in 0..20000 {
        let mut output = vec![0.0; capacity * channels];
        let drained = plugin
            .drain(&mut output, &ProcessContext::new(rate, 0))
            .unwrap();
        assert!(drained.frames <= capacity && drained.frames <= plugin.drain_output_frames_max());
        result.extend_from_slice(&output[..drained.frames * channels]);
        if drained.complete {
            return result;
        }
    }
    panic!("ordered drain did not finish");
}

fn seeded_input(frames: usize, channels: usize) -> Vec<f32> {
    (0..frames * channels)
        .map(|i| {
            if i / channels == frames - 1 {
                (i % channels + 1) as f32 * 0.25
            } else {
                ((i * 13 % 23) as f32 - 11.0) / 64.0
            }
        })
        .collect()
}

fn band(filter_type: &str, frequency: f64, q: f64, gain_db: f64, placement: Option<Placement>) -> BandConfig {
    BandConfig {
        filter_type: filter_type.to_string(),
        frequency,
        q,
        gain_db,
        active: true,
        placement,
    }
}

/// One exactness-matrix cell: per-band placements plus
/// `(filter_type, frequency, q, gain_db)` band specs.
type ExactCaseSet = (Vec<Option<Placement>>, Vec<(&'static str, f64, f64, f64)>);

#[allow(
    clippy::too_many_arguments,
    reason = "test helper: each argument selects one matrix dimension"
)]
fn run_exact_case(
    placements: &[Option<Placement>],
    specs: &[(&str, f64, f64, f64)],
    channels: usize,
    pairs: Option<Vec<[usize; 2]>>,
    rate: u32,
    mix: f32,
    length_index: usize,
    phase: usize,
    drain_capacity: usize,
    bound: f64,
    context: &str,
) {
    let taps = 1024 << length_index;
    let filters = placements
        .iter()
        .zip(specs.iter())
        .map(|(placement, (filter_type, frequency, q, gain_db))| {
            band(filter_type, *frequency, *q, *gain_db, *placement)
        })
        .collect::<Vec<_>>();
    let mut plugin = LinearPhaseEqPlugin::from_params(
        channels,
        rate,
        LinearPhaseEqPluginParams {
            num_filters: filters.len(),
            fir_length_index: length_index,
            phase_mode_index: phase,
            auto_gain: false,
            mix,
            filters,
            stereo_pairs: pairs,
        },
    )
    .unwrap();
    assert!(
        plugin.is_ordered_route(),
        "{context}: expected the ordered route"
    );
    let stages = placements.len();
    let expected_latency = if phase == 0 {
        stages * (taps / 2 + 32)
    } else {
        stages * 32
    };
    assert_eq!(plugin.latency_samples(), expected_latency, "{context}");
    let support = stages * (taps - 1 + 32);
    assert_eq!(
        plugin.tail_length(),
        TailLength::Finite(support.max(expected_latency) as u64),
        "{context}"
    );

    let frames = 40 + 7 * stages;
    let input = seeded_input(frames, channels);
    let mut reference = CascadeRef::new(&plugin, f64::from(mix));
    let mut actual = stream_all(&mut plugin, &input, rate);
    let mut expected = Vec::with_capacity((frames + support) * channels);
    for frame in input.chunks_exact(channels) {
        expected.extend_from_slice(&reference.push_frame(frame));
    }
    actual.extend(drain_all(&mut plugin, rate, drain_capacity));
    let zeros = vec![0.0; support * channels];
    for frame in zeros.chunks_exact(channels) {
        expected.extend_from_slice(&reference.push_frame(frame));
    }
    assert_eq!(actual.len(), (frames + support) * channels, "{context}");
    assert_eq!(actual.len(), expected.len(), "{context}");
    // `bound` is fixed by the caller before measurement: single-stage f32
    // partitioned error is bounded by 2e-6 (see the finite-stream gate) and
    // cascades accumulate linearly in the worst case, plus f64/f32 Mid/Side
    // encode rounding, with small band gains keeping signal peaks near unity.
    for (index, (&actual, &expected)) in actual.iter().zip(&expected).enumerate() {
        assert!(
            (f64::from(actual) - f64::from(expected)).abs() < bound,
            "{context} sample{index}: {actual} vs {expected}"
        );
    }
}

#[test]
fn exact_ordered_output_matches_independent_f64_cascade() {
    let sets: Vec<ExactCaseSet> = vec![
        (vec![Some(Placement::Left)], vec![("Peak", 1000.0, 1.0, 3.0)]),
        (vec![Some(Placement::Right)], vec![("Peak", 2000.0, 1.5, -4.0)]),
        (
            vec![Some(Placement::Mid)],
            vec![("Lowshelf", 300.0, 1.0, 3.0)],
        ),
        (vec![Some(Placement::Side)], vec![("Highpass", 500.0, 0.707, 0.0)]),
        (
            vec![None, Some(Placement::Left)],
            vec![("Peak", 1000.0, 1.0, 3.0), ("Peak", 2000.0, 1.0, -4.0)],
        ),
        (
            vec![Some(Placement::Mid), Some(Placement::Left)],
            vec![
                ("Highpass", 4000.0, 0.707, 0.0),
                ("Lowpass", 500.0, 0.707, 0.0),
            ],
        ),
        (
            vec![Some(Placement::Left), Some(Placement::Mid)],
            vec![("Peak", 1000.0, 1.0, 3.0), ("Lowshelf", 300.0, 1.0, -3.0)],
        ),
        (
            vec![None, Some(Placement::Mid), Some(Placement::Right)],
            vec![
                ("Peak", 1000.0, 1.0, 3.0),
                ("Lowshelf", 300.0, 1.0, -2.0),
                ("Highpass", 800.0, 0.707, 0.0),
            ],
        ),
    ];
    let mut case = 0usize;
    for length_index in [0usize, 1, 2] {
        for phase in [0usize, 1] {
            for (set_index, (placements, specs)) in sets.iter().enumerate() {
                if length_index == 2 && !matches!(set_index, 4 | 5) {
                    continue;
                }
                if length_index == 1 && !matches!(set_index, 0 | 4 | 5 | 7) {
                    continue;
                }
                for channels_cfg in 0..2 {
                    if channels_cfg == 1 && !matches!(set_index, 0 | 5 | 7) {
                        continue;
                    }
                    let channels = if channels_cfg == 0 { 2 } else { 4 };
                    let pairs = if channels_cfg == 0 {
                        None
                    } else {
                        Some(vec![[0, 2]])
                    };
                    let rate = if channels_cfg == 1 { 96_000 } else { 48_000 };
                    let taps = 1024 << length_index;
                    let context =
                        format!("set{set_index}/taps{taps}/phase{phase}/{rate}Hz/{channels}ch");
                    run_exact_case(
                        placements,
                        specs,
                        channels,
                        pairs,
                        rate,
                        1.0,
                        length_index,
                        phase,
                        [3, 257][case % 2],
                        1e-5,
                        &context,
                    );
                    case += 1;
                }
            }
        }
    }
    // Partial-mix and dry-only exactness on the cascade route.
    run_exact_case(
        &sets[5].0,
        &sets[5].1,
        2,
        None,
        48_000,
        0.375,
        0,
        0,
        17,
        1e-5,
        "mix0.375/linear",
    );
    run_exact_case(
        &sets[5].0,
        &sets[5].1,
        4,
        Some(vec![[0, 2]]),
        96_000,
        0.375,
        0,
        1,
        17,
        1e-5,
        "mix0.375/minimum/4ch",
    );
    run_exact_case(
        &sets[2].0,
        &sets[2].1,
        2,
        None,
        48_000,
        0.0,
        0,
        0,
        257,
        1e-5,
        "mix0.0/mid",
    );
    // 44.1 kHz and 8192-tap smoke coverage.
    run_exact_case(
        &sets[5].0,
        &sets[5].1,
        2,
        None,
        44_100,
        1.0,
        0,
        0,
        3,
        1e-5,
        "44.1kHz",
    );
    run_exact_case(
        &sets[0].0,
        &sets[0].1,
        2,
        None,
        96_000,
        1.0,
        3,
        0,
        257,
        1e-5,
        "8192taps",
    );
}

#[test]
fn exactness_covers_ten_stages_two_pairs_and_eight_channels() {
    // Ten stages at the maximum band count: the worst-case linear error
    // accumulation is 10 * 2e-6 = 2e-5 (accepted single-stage gate), plus
    // Mid/Side f64/f32 encode rounding with small band gains keeping signal
    // peaks near unity, so 3e-5 keeps a 50% margin. This bound is fixed here
    // before measurement and deliberately differs from the <=3-stage 1e-5.
    let ten: Vec<Option<Placement>> = vec![
        Some(Placement::Left),
        Some(Placement::Mid),
        Some(Placement::Right),
        Some(Placement::Side),
        None,
        Some(Placement::Left),
        Some(Placement::Mid),
        Some(Placement::Right),
        Some(Placement::Side),
        None,
    ];
    let specs: Vec<(&str, f64, f64, f64)> = vec![
        ("Peak", 1000.0, 1.0, 3.0),
        ("Lowshelf", 300.0, 1.0, -2.0),
        ("Peak", 2000.0, 1.5, 2.0),
        ("Highpass", 800.0, 0.707, 0.0),
        ("Peak", 4000.0, 1.0, -3.0),
        ("Highshelf", 6000.0, 1.0, 2.0),
        ("Peak", 500.0, 2.0, -2.0),
        ("Lowpass", 9000.0, 0.707, 0.0),
        ("Peak", 3000.0, 1.0, 3.0),
        ("Peak", 1500.0, 1.0, -2.0),
    ];
    run_exact_case(
        &ten, &specs, 2, None, 48_000, 1.0, 0, 0, 17, 3e-5, "ten-stage/linear",
    );
    // Two explicit pairs on four channels: both pairs process, no channel is
    // left unpaired.
    run_exact_case(
        &[Some(Placement::Left), Some(Placement::Mid)],
        &[("Peak", 1000.0, 1.0, 3.0), ("Lowshelf", 300.0, 1.0, -3.0)],
        4,
        Some(vec![[0, 1], [2, 3]]),
        48_000,
        1.0,
        0,
        0,
        17,
        1e-5,
        "two-pairs/4ch",
    );
    // Eight channels with two pairs: channels 2-5 take the identity path.
    run_exact_case(
        &[Some(Placement::Side), Some(Placement::Right)],
        &[
            ("Highpass", 500.0, 0.707, 0.0),
            ("Peak", 2000.0, 1.0, -4.0),
        ],
        8,
        Some(vec![[0, 1], [6, 7]]),
        48_000,
        1.0,
        0,
        0,
        17,
        1e-5,
        "two-pairs/8ch",
    );
}

#[test]
fn swapped_placement_order_does_not_commute() {
    // Same two bands in opposite orders: a Left cut (-18 dB) and a Mid boost
    // (+18 dB) at the same 2 kHz center. Left is a diagonal MIMO stage (L: H,
    // R: I) while Mid is a coupled stage (Mid: H2, Side: I with 0.5*(L+-R)
    // mixing); diagonal and coupled stages provably do not commute, with
    // off-diagonal cascade difference 0.5*(H2-1)*(H1-1) = -3.03 at center
    // (vs -1.12 for +/-12 dB). Cut-then-boost keeps final output levels near
    // unity so the 1e-5 per-order exactness holds, while the order difference
    // stays large. The 2 kHz center sits at the period-23 seeded stimulus
    // fundamental (48000/23 = 2087 Hz) for strong excitation; a prior 1 kHz
    // center fell between stimulus harmonics and differed by only 0.030 (and
    // an earlier disjoint lowpass/highpass pair by 0.044). Each order must
    // match its own independent reference, and the references themselves must
    // differ (setup guard, not a guessed bound). Guards unchanged.
    let orders: [Vec<BandConfig>; 2] = [
        vec![
            band("Peak", 2000.0, 1.0, -18.0, Some(Placement::Left)),
            band("Peak", 2000.0, 1.0, 18.0, Some(Placement::Mid)),
        ],
        vec![
            band("Peak", 2000.0, 1.0, 18.0, Some(Placement::Mid)),
            band("Peak", 2000.0, 1.0, -18.0, Some(Placement::Left)),
        ],
    ];
    let mut outputs = Vec::new();
    let mut references = Vec::new();
    for (order_index, filters) in orders.iter().enumerate() {
        let mut plugin = LinearPhaseEqPlugin::from_params(
            2,
            48_000,
            LinearPhaseEqPluginParams {
                num_filters: 2,
                fir_length_index: 0,
                phase_mode_index: 0,
                auto_gain: false,
                mix: 1.0,
                filters: filters.clone(),
                stereo_pairs: None,
            },
        )
        .unwrap();
        let mut input = seeded_input(64, 2);
        // Stereo-asymmetric content: an impulse on the left, a delayed
        // opposite impulse on the right.
        input[0] = 1.0;
        input[11] = -0.75;
        let mut reference = CascadeRef::new(&plugin, 1.0);
        let mut actual = stream_all(&mut plugin, &input, 48_000);
        let mut expected = Vec::new();
        for frame in input.as_chunks::<2>().0 {
            expected.extend_from_slice(&reference.push_frame(frame));
        }
        actual.extend(drain_all(&mut plugin, 48_000, 257));
        let TailLength::Finite(support) = plugin.tail_length() else {
            panic!("order{order_index}: expected a finite tail");
        };
        for frame in vec![0.0; support as usize * 2].chunks_exact(2) {
            expected.extend_from_slice(&reference.push_frame(frame));
        }
        assert_eq!(actual.len(), expected.len());
        for (index, (&actual, &expected)) in actual.iter().zip(&expected).enumerate() {
            assert!(
                (f64::from(actual) - f64::from(expected)).abs() < 1e-5,
                "order{order_index} sample{index}: {actual} vs {expected}"
            );
        }
        outputs.push(actual);
        references.push(expected);
    }
    let predicted = references[0]
        .iter()
        .zip(references[1].iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(
        predicted > 0.1,
        "noncommuting setup degenerated: reference orders differ by only {predicted}"
    );
    let measured = outputs[0]
        .iter()
        .zip(outputs[1].iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(
        measured > 0.1,
        "swapped placement order must not commute: measured {measured}"
    );
}

fn placed_params(
    filters: Vec<BandConfig>,
    pairs: Option<Vec<[usize; 2]>>,
    auto_gain: bool,
) -> LinearPhaseEqPluginParams {
    let num_filters = filters.len();
    LinearPhaseEqPluginParams {
        num_filters,
        fir_length_index: 0,
        phase_mode_index: 0,
        auto_gain,
        mix: 1.0,
        filters,
        stereo_pairs: pairs,
    }
}

fn expect_err_contains(result: Result<LinearPhaseEqPlugin, String>, needle: &str) {
    match result {
        Ok(_) => panic!("expected an error containing {needle:?}"),
        Err(error) => assert!(error.contains(needle), "error {error:?} lacks {needle:?}"),
    }
}

#[test]
fn invalid_pairs_placements_and_mixes_are_rejected() {
    let left = || band("Peak", 1000.0, 1.0, 3.0, Some(Placement::Left));
    // One channel cannot host pair placements.
    expect_err_contains(
        LinearPhaseEqPlugin::from_params(1, 48_000, placed_params(vec![left()], None, false)),
        "at least two",
    );
    // Multichannel pair placements require explicit pairs.
    expect_err_contains(
        LinearPhaseEqPlugin::from_params(4, 48_000, placed_params(vec![left()], None, false)),
        "explicit stereo_pairs",
    );
    // Overlapping pairs are rejected.
    expect_err_contains(
        LinearPhaseEqPlugin::from_params(
            4,
            48_000,
            placed_params(vec![left()], Some(vec![[0, 1], [1, 2]]), false),
        ),
        "disjoint",
    );
    // Out-of-range pairs are rejected.
    expect_err_contains(
        LinearPhaseEqPlugin::from_params(
            4,
            48_000,
            placed_params(vec![left()], Some(vec![[0, 4]]), false),
        ),
        "exceeds 4",
    );
    // Degenerate self-pairs are rejected.
    expect_err_contains(
        LinearPhaseEqPlugin::from_params(
            2,
            48_000,
            placed_params(vec![left()], Some(vec![[1, 1]]), false),
        ),
        "distinct",
    );
    // An explicit empty pair list cannot satisfy a required placement.
    expect_err_contains(
        LinearPhaseEqPlugin::from_params(
            2,
            48_000,
            placed_params(vec![left()], Some(vec![]), false),
        ),
        "at least one stereo pair",
    );
    // Auto gain stays stereo-linked only.
    expect_err_contains(
        LinearPhaseEqPlugin::from_params(2, 48_000, placed_params(vec![left()], None, true)),
        "auto_gain is not supported",
    );
    // Unknown placement labels fail closed at the serde boundary.
    assert!(serde_json::from_value::<LinearPhaseEqPluginParams>(serde_json::json!({
        "num_filters": 1,
        "filters": [{"filter_type": "Peak", "placement": "diagonal"}],
    }))
    .is_err());
    // Malformed pair shapes fail closed as well.
    assert!(serde_json::from_value::<LinearPhaseEqPluginParams>(serde_json::json!({
        "num_filters": 1,
        "filters": [{"filter_type": "Peak", "placement": "left"}],
        "stereo_pairs": [[0]],
    }))
    .is_err());
    // Valid controls: explicit pairs on four channels, default pair on stereo.
    let four = LinearPhaseEqPlugin::from_params(
        4,
        48_000,
        placed_params(vec![left()], Some(vec![[0, 2]]), false),
    )
    .unwrap();
    assert_eq!(four.stereo_pairs(), &[[0, 2]]);
    let stereo =
        LinearPhaseEqPlugin::from_params(2, 48_000, placed_params(vec![left()], None, false))
            .unwrap();
    assert_eq!(stereo.stereo_pairs(), &[[0, 1]]);
}

#[test]
fn reset_and_reprepare_restore_ordered_state() {
    let params = placed_params(
        vec![
            band("Peak", 1000.0, 1.0, 6.0, Some(Placement::Mid)),
            band("Highpass", 800.0, 0.707, 0.0, Some(Placement::Right)),
        ],
        None,
        false,
    );
    let input = seeded_input(200, 2);
    let mut plugin = LinearPhaseEqPlugin::from_params(2, 48_000, params.clone()).unwrap();
    let first = stream_all(&mut plugin, &input, 48_000);
    let first_tail = drain_all(&mut plugin, 48_000, 257);
    plugin.reset();
    let replay = stream_all(&mut plugin, &input, 48_000);
    let replay_tail = drain_all(&mut plugin, 48_000, 17);
    assert_eq!(first, replay);
    assert_eq!(first_tail, replay_tail);

    // Same-rate initialization preserves ordered history.
    let mut candidate = LinearPhaseEqPlugin::from_params(2, 48_000, params.clone()).unwrap();
    let mut reference = LinearPhaseEqPlugin::from_params(2, 48_000, params).unwrap();
    let half = input.len() / 2;
    let mut candidate_out = stream_all(&mut candidate, &input[..half], 48_000);
    let mut reference_out = stream_all(&mut reference, &input[..half], 48_000);
    candidate.initialize(48_000.0).unwrap();
    candidate_out.extend(stream_all(&mut candidate, &input[half..], 48_000));
    reference_out.extend(stream_all(&mut reference, &input[half..], 48_000));
    assert_eq!(candidate_out, reference_out);

    // A rate change rebuilds ordered designs and clears old-rate history.
    candidate.initialize(44_100.0).unwrap();
    let mut fresh = LinearPhaseEqPlugin::from_params(
        2,
        44_100,
        placed_params(
            vec![
                band("Peak", 1000.0, 1.0, 6.0, Some(Placement::Mid)),
                band("Highpass", 800.0, 0.707, 0.0, Some(Placement::Right)),
            ],
            None,
            false,
        ),
    )
    .unwrap();
    assert_eq!(candidate.latency_samples(), fresh.latency_samples());
    assert_eq!(candidate.latency_samples(), 2 * (1024 / 2 + 32));
    let zeros = vec![0.0; 3000 * 2];
    assert_eq!(
        stream_all(&mut candidate, &zeros, 44_100),
        stream_all(&mut fresh, &zeros, 44_100)
    );
    assert_eq!(
        drain_all(&mut candidate, 44_100, 17),
        drain_all(&mut fresh, 44_100, 257)
    );
}

#[test]
fn legacy_defaults_routing_and_audio_are_unchanged() {
    // Unset routing keeps the legacy single-FIR path with established latency.
    let legacy = LinearPhaseEqPlugin::from_params(
        2,
        48_000,
        LinearPhaseEqPluginParams {
            num_filters: 2,
            fir_length_index: 1,
            phase_mode_index: 0,
            auto_gain: false,
            mix: 1.0,
            filters: vec![
                band("Peak", 1000.0, 1.0, 6.0, None),
                band("Lowshelf", 200.0, 0.707, -3.0, None),
            ],
            stereo_pairs: None,
        },
    )
    .unwrap();
    assert!(!legacy.is_ordered_route());
    assert_eq!(legacy.stage_count(), 1);
    assert_eq!(legacy.latency_samples(), 2048 / 2 + 32);
    assert_eq!(legacy.stereo_pairs(), &[[0, 1]]);
    assert_eq!(legacy.band_placement(0), Some(None));
    assert_eq!(legacy.band_placement(99), None);

    // Explicit Stereo behaves exactly like the legacy default.
    let stereo = LinearPhaseEqPlugin::from_params(
        2,
        48_000,
        LinearPhaseEqPluginParams {
            num_filters: 2,
            fir_length_index: 1,
            phase_mode_index: 0,
            auto_gain: false,
            mix: 1.0,
            filters: vec![
                band("Peak", 1000.0, 1.0, 6.0, Some(Placement::Stereo)),
                band("Lowshelf", 200.0, 0.707, -3.0, Some(Placement::Stereo)),
            ],
            stereo_pairs: None,
        },
    )
    .unwrap();
    assert!(!stereo.is_ordered_route());
    assert_eq!(legacy.fir_coeffs, stereo.fir_coeffs);
    let input = seeded_input(300, 2);
    let mut legacy = legacy;
    let mut stereo = stereo;
    assert_eq!(
        stream_all(&mut legacy, &input, 48_000),
        stream_all(&mut stereo, &input, 48_000)
    );

    // Old presets without the new keys load with legacy routing, and
    // serialization omits the new keys (append-only persistence).
    let old: LinearPhaseEqPluginParams = serde_json::from_value(serde_json::json!({
        "num_filters": 2,
        "fir_length": 1,
        "phase_mode": 0,
        "filters": [{"filter_type": "Peak", "frequency": 500.0}],
    }))
    .unwrap();
    assert_eq!(old.stereo_pairs, None);
    assert_eq!(old.filters.len(), 1);
    assert_eq!(old.filters[0].placement, None);
    let serialized = serde_json::to_value(&old).unwrap();
    assert!(!serialized.as_object().unwrap().contains_key("stereo_pairs"));
    assert!(
        !serialized["filters"][0]
            .as_object()
            .unwrap()
            .contains_key("placement")
    );

    // Parameter layout is append-only: placement entries follow each band's
    // active entry, legacy IDs keep their relative order.
    let ids = legacy
        .parameters()
        .iter()
        .map(|p| p.id.as_str().to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec![
            "num_filters",
            "fir_length_index",
            "phase_mode_index",
            "auto_gain",
            "mix",
            "band_0_type",
            "band_0_freq",
            "band_0_q",
            "band_0_gain",
            "band_0_active",
            "band_0_placement",
            "band_1_type",
            "band_1_freq",
            "band_1_q",
            "band_1_gain",
            "band_1_active",
            "band_1_placement",
        ]
    );
    assert_eq!(
        legacy.get_parameter(&ParameterId::from("band_1_placement")),
        Some(ParameterValue::Int(0))
    );
    assert!(
        legacy
            .set_parameter(
                ParameterId::from("band_0_placement"),
                ParameterValue::Int(2)
            )
            .unwrap_err()
            .contains("structural")
    );
    assert!(
        legacy
            .set_parameter(
                ParameterId::from("band_0_placement"),
                ParameterValue::Int(6)
            )
            .is_err()
    );
}

#[test]
fn latency_and_tail_scale_with_stages_taps_and_phase() {
    for length_index in 0..4 {
        let taps = 1024 << length_index;
        for phase in 0..2 {
            let per_stage = if phase == 0 { taps / 2 + 32 } else { 32 };
            for num_filters in [1usize, 2, 5, 10] {
                let legacy = LinearPhaseEqPlugin::from_params(
                    2,
                    48_000,
                    LinearPhaseEqPluginParams {
                        num_filters,
                        fir_length_index: length_index,
                        phase_mode_index: phase,
                        auto_gain: false,
                        mix: 1.0,
                        filters: vec![],
                        stereo_pairs: None,
                    },
                )
                .unwrap();
                assert_eq!(legacy.latency_samples(), per_stage);
                assert_eq!(
                    legacy.tail_length(),
                    TailLength::Finite((32 + taps - 1).max(per_stage) as u64)
                );
                let mut filters = vec![band("Peak", 1000.0, 1.0, 3.0, Some(Placement::Left))];
                while filters.len() < num_filters {
                    filters.push(band("Peak", 2000.0, 1.0, 0.0, None));
                }
                let ordered = LinearPhaseEqPlugin::from_params(
                    2,
                    48_000,
                    LinearPhaseEqPluginParams {
                        num_filters,
                        fir_length_index: length_index,
                        phase_mode_index: phase,
                        auto_gain: false,
                        mix: 1.0,
                        filters,
                        stereo_pairs: None,
                    },
                )
                .unwrap();
                assert!(ordered.is_ordered_route());
                assert_eq!(ordered.latency_samples(), num_filters * per_stage);
                assert_eq!(
                    ordered.tail_length(),
                    TailLength::Finite(
                        (num_filters * (taps - 1 + 32)).max(num_filters * per_stage) as u64
                    )
                );
            }
        }
    }
}

#[test]
fn inactive_slots_keep_ordered_latency_stable() {
    // Identity stages make latency independent of which bands are enabled,
    // which is what keeps dynamic active toggles latency-stable. The Mid cut
    // uses -18 dB (linear 0.126, difference 0.874 vs identity) so the
    // audible-difference guard has robust margin above its 0.01 bound on the
    // period-23 seeded stimulus: a -6 dB cut (difference 0.5) measured only
    // 0.0072 max on the same input because the stimulus energy spreads across
    // 23 harmonics. Bound unchanged.
    let active = placed_params(
        vec![
            band("Peak", 1000.0, 1.0, 6.0, Some(Placement::Left)),
            band("Peak", 2000.0, 1.0, -18.0, Some(Placement::Mid)),
        ],
        None,
        false,
    );
    let mut idle = active.clone();
    idle.filters[1].active = false;
    let active = LinearPhaseEqPlugin::from_params(2, 48_000, active).unwrap();
    let idle = LinearPhaseEqPlugin::from_params(2, 48_000, idle).unwrap();
    assert_eq!(active.latency_samples(), idle.latency_samples());
    assert_eq!(active.latency_samples(), 2 * (1024 / 2 + 32));
    assert_eq!(active.tail_length(), idle.tail_length());
    // The responses still differ: the idle slot passes its domain through.
    // Stream well beyond the 1088-sample cascade latency so the main lobe has
    // arrived: 256 frames sit entirely in the pre-latency sidelobe region
    // where any two linear-phase FIRs agree to ~1e-8, which made the
    // difference guard vacuous.
    let input = seeded_input(2048, 2);
    let mut active = active;
    let mut idle = idle;
    let a = stream_all(&mut active, &input, 48_000);
    let b = stream_all(&mut idle, &input, 48_000);
    let diff = a
        .iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max);
    assert!(diff > 0.01, "disabling a band must change audio, diff={diff}");
}

fn streamed_impulse(
    plugin: &mut LinearPhaseEqPlugin,
    channel: usize,
    channels: usize,
    rate: u32,
) -> Vec<f32> {
    let support = match plugin.tail_length() {
        TailLength::Finite(frames) => frames as usize,
        TailLength::Unknown => panic!("expected a finite tail"),
        TailLength::Infinite => panic!("expected a finite tail"),
    };
    let length = support + 64;
    let mut input = vec![0.0; length * channels];
    input[channel] = 1.0;
    let mut output = stream_all(plugin, &input, rate);
    output.extend(drain_all(plugin, rate, 257));
    output
}

fn dft_at(output: &[f32], channel: usize, channels: usize, rate: u32, frequency: f64) -> (f64, f64) {
    let mut re = 0.0;
    let mut im = 0.0;
    for (n, frame) in output.chunks_exact(channels).enumerate() {
        let angle = std::f64::consts::TAU * frequency * n as f64 / f64::from(rate);
        re += f64::from(frame[channel]) * angle.cos();
        im -= f64::from(frame[channel]) * angle.sin();
    }
    (re, im)
}

#[test]
fn channel_response_matches_streamed_impulse_per_channel() {
    // The response API shares FIR designs with streaming but computes through
    // an independent path (direct DTFT + cascade math vs partitioned FFT
    // convolution), so agreement cross-validates routing, delay accounting
    // and the chart-facing API at once.
    let configs: Vec<(Vec<BandConfig>, usize)> = vec![
        (
            vec![
                band("Peak", 1000.0, 1.0, 6.0, None),
                band("Lowshelf", 200.0, 0.707, 3.0, None),
            ],
            0,
        ),
        (
            vec![
                band("Peak", 1000.0, 1.0, 6.0, Some(Placement::Left)),
                band("Highpass", 800.0, 0.707, 0.0, Some(Placement::Mid)),
            ],
            0,
        ),
        (
            vec![
                band("Peak", 1000.0, 1.0, 6.0, Some(Placement::Right)),
                band("Lowshelf", 200.0, 0.707, -6.0, Some(Placement::Side)),
            ],
            1,
        ),
    ];
    for (config_index, (filters, phase)) in configs.iter().enumerate() {
        let num_filters = filters.len();
        for channel in 0..2 {
            let mut plugin = LinearPhaseEqPlugin::from_params(
                2,
                48_000,
                LinearPhaseEqPluginParams {
                    num_filters,
                    fir_length_index: 0,
                    phase_mode_index: *phase,
                    auto_gain: false,
                    mix: 1.0,
                    filters: filters.clone(),
                    stereo_pairs: None,
                },
            )
            .unwrap();
            let output = streamed_impulse(&mut plugin, channel, 2, 48_000);
            for frequency in [100.0, 500.0, 1000.0, 3000.0, 8000.0] {
                let api = plugin
                    .channel_complex_response(channel, frequency)
                    .unwrap_or_else(|| panic!("config{config_index} ch{channel} {frequency}Hz"));
                let (re, im) = dft_at(&output, channel, 2, 48_000, frequency);
                // 5e-3 absolute: streamed f32 partitioned rounding accumulates
                // incoherently over ~1k DFT terms (~1e-4 RMS); the bound keeps
                // a 50x margin on unity-scale responses.
                let err = (api.re - re).hypot(api.im - im);
                assert!(
                    err < 5e-3,
                    "config{config_index} ch{channel} {frequency}Hz: api {api:?} vs streamed ({re:.6}, {im:.6}), err {err:.6}"
                );
            }
            if *phase == 0 {
                // Linear-phase group delay is exactly the reported latency;
                // the numerical derivative is exact on linear phase, so 0.5
                // samples is an enormous margin that only guards wiring.
                for frequency in [200.0, 1000.0, 5000.0] {
                    let delay = plugin
                        .channel_group_delay_samples(channel, frequency)
                        .unwrap();
                    let latency = plugin.latency_samples() as f64;
                    assert!(
                        (delay - latency).abs() < 0.5,
                        "config{config_index} ch{channel} {frequency}Hz: gd {delay} vs latency {latency}"
                    );
                }
            } else {
                // Minimum-phase delay varies; only finiteness is asserted
                // here. Exact minimum-phase behavior is proven by the
                // cascade-exactness matrix, which covers both phases.
                let delay = plugin.channel_group_delay_samples(channel, 1000.0).unwrap();
                assert!(delay.is_finite(), "minimum-phase gd must be finite");
            }
        }
    }
    // Invalid queries fail closed.
    let plugin = LinearPhaseEqPlugin::new(2, 48_000);
    assert!(plugin.channel_complex_response(2, 1000.0).is_none());
    assert!(plugin.channel_complex_response(0, -1.0).is_none());
    assert!(plugin.channel_complex_response(0, 24_001.0).is_none());
    assert!(plugin.channel_complex_response(0, f64::NAN).is_none());
    assert!(plugin.channel_group_delay_samples(2, 1000.0).is_none());
}

#[test]
fn mid_stopband_response_matches_single_channel_excitation() {
    // Regression for the correlated-input response bug: with all channels
    // driven, a Mid highpass at stopband answers H ~= 0; with the correct
    // single-channel (delta) excitation the diagonal is 0.5 * (H + I) ~= 0.5.
    // A single-left impulse DFT is the independent streaming leg.
    for channel in 0..2 {
        let mut plugin = LinearPhaseEqPlugin::from_params(
            2,
            48_000,
            LinearPhaseEqPluginParams {
                num_filters: 1,
                fir_length_index: 0,
                phase_mode_index: 0,
                auto_gain: false,
                mix: 1.0,
                filters: vec![band("Highpass", 800.0, 0.707, 0.0, Some(Placement::Mid))],
                stereo_pairs: None,
            },
        )
        .unwrap();
        let output = streamed_impulse(&mut plugin, channel, 2, 48_000);
        let (re, im) = dft_at(&output, channel, 2, 48_000, 100.0);
        let streamed = re.hypot(im);
        // Setup guard: the streamed diagonal really sits at the stopband
        // half-identity. Without this the bound below could pass vacuously.
        assert!(
            (streamed - 0.5).abs() < 0.05,
            "ch{channel}: streamed stopband diagonal {streamed} must sit near 0.5"
        );
        let api = plugin.channel_complex_response(channel, 100.0).unwrap();
        let err = (api.re - re).hypot(api.im - im);
        assert!(
            err < 5e-3,
            "ch{channel}: api {api:?} vs streamed ({re:.6}, {im:.6}), err {err:.6}"
        );
    }
}
