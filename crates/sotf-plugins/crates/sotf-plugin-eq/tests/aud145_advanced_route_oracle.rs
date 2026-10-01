//! AUD145 public ordered-route composition checks for the advanced realizations.
//!
//! The reference uses separate public math-audio realization instances and an
//! independently written per-pair L/R/M/S composition loop. It checks route
//! order and state partitioning; it is not an independent coefficient-design
//! oracle for WarpedBiquad, Kautz, or SVF.

use math_audio_iir_fir::{
    Biquad, BiquadFilterType, KautzFilter, SvfFilter, SvfFilterType, WarpedBiquad, bark_lambda,
};
use sotf_host::{AutoGainParams, ParameterId, ParameterValue, ParametricPlugin, ProcessContext};
use sotf_plugin_eq::{
    BiquadFilterConfig, EqBandPlacement, EqFilterTopology, EqPlugin, EqPluginParams,
    KautzSectionConfig,
};

const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: usize = 5;
const PAIRS: [[usize; 2]; 2] = [[0, 1], [3, 2]];
const FRAMES: usize = 2_048;
const PROCESS_BLOCK: usize = 193;
const PEAK_LIMIT: f64 = 2.0e-5;
const RMS_LIMIT: f64 = 2.0e-6;

#[derive(Clone, Copy)]
enum ReferenceKind {
    Biquad,
    Warped,
    Kautz,
    Svf,
}

#[derive(Clone, Copy)]
struct Scenario {
    name: &'static str,
    config_topology: EqFilterTopology,
    reference_kind: ReferenceKind,
    order: usize,
    lambda: Option<f64>,
    use_svf_topology: bool,
}

enum ReferenceRealization {
    Biquad(Vec<Biquad<f64>>),
    Warped(WarpedBiquad<f64>),
    Kautz(KautzFilter<f64>),
    Svf(SvfFilter<f64>),
}

impl ReferenceRealization {
    fn new(config: &BiquadFilterConfig, kind: ReferenceKind, sample_rate: f64) -> Self {
        match kind {
            ReferenceKind::Biquad => {
                let section_count = (config.order / 2).max(1);
                let stages = (0..section_count)
                    .map(|stage| {
                        let q = if config.order == 2 {
                            config.q
                        } else {
                            let angle = std::f64::consts::PI * (2 * stage + 1) as f64
                                / (2 * config.order) as f64;
                            config.q / (2.0 * angle.cos())
                        };
                        Biquad::new(
                            BiquadFilterType::Peak,
                            config.freq,
                            sample_rate,
                            q,
                            config.db_gain / section_count as f64,
                        )
                    })
                    .collect();
                Self::Biquad(stages)
            }
            ReferenceKind::Warped => Self::Warped(WarpedBiquad::new(
                BiquadFilterType::Peak,
                config.freq,
                sample_rate,
                config.q,
                config.db_gain,
                config.lambda.unwrap_or_else(|| bark_lambda(sample_rate)),
            )),
            ReferenceKind::Kautz => {
                let sections = if config.kautz_sections.is_empty() {
                    vec![KautzSectionConfig {
                        pole_freq: config.freq,
                        q: config.q,
                        gain: config.db_gain,
                    }]
                } else {
                    config.kautz_sections.clone()
                };
                let modes = sections
                    .iter()
                    .map(|section| (section.pole_freq, section.q))
                    .collect::<Vec<_>>();
                let mut filter = KautzFilter::from_room_modes(&modes, sample_rate);
                for (realized, configured) in filter.sections.iter_mut().zip(&sections) {
                    realized.gain = configured.gain;
                }
                Self::Kautz(filter)
            }
            ReferenceKind::Svf => Self::Svf(SvfFilter::new(
                SvfFilterType::Peak,
                config.freq,
                sample_rate,
                config.q,
                config.db_gain,
            )),
        }
    }

    fn process(&mut self, input: f64) -> f64 {
        match self {
            Self::Biquad(stages) => stages
                .iter_mut()
                .fold(input, |sample, stage| stage.process(sample)),
            Self::Warped(filter) => filter.process(input),
            Self::Kautz(filter) => input + filter.process(input),
            Self::Svf(filter) => filter.process(input),
        }
    }
}

struct ReferenceStage {
    placement: EqBandPlacement,
    filters_by_channel: Vec<ReferenceRealization>,
}

impl ReferenceStage {
    fn new(
        config: &BiquadFilterConfig,
        kind: ReferenceKind,
        sample_rate: f64,
        channels: usize,
    ) -> Self {
        Self {
            placement: config.placement.expect("ordered reference needs placement"),
            filters_by_channel: (0..channels)
                .map(|_| ReferenceRealization::new(config, kind, sample_rate))
                .collect(),
        }
    }

    fn process_frame(&mut self, frame: &mut [f32]) {
        match self.placement {
            EqBandPlacement::Stereo => {
                for (channel, sample) in frame.iter_mut().enumerate() {
                    *sample = self.filters_by_channel[channel].process(f64::from(*sample)) as f32;
                }
            }
            EqBandPlacement::Left => {
                for [left, _] in PAIRS {
                    frame[left] =
                        self.filters_by_channel[left].process(f64::from(frame[left])) as f32;
                }
            }
            EqBandPlacement::Right => {
                for [_, right] in PAIRS {
                    frame[right] =
                        self.filters_by_channel[right].process(f64::from(frame[right])) as f32;
                }
            }
            EqBandPlacement::Mid => {
                for [left, right] in PAIRS {
                    let left_sample = f64::from(frame[left]);
                    let right_sample = f64::from(frame[right]);
                    let mid = 0.5 * (left_sample + right_sample);
                    let side = 0.5 * (left_sample - right_sample);
                    let filtered_mid = self.filters_by_channel[left].process(mid);
                    frame[left] = (filtered_mid + side) as f32;
                    frame[right] = (filtered_mid - side) as f32;
                }
            }
            EqBandPlacement::Side => {
                for [left, right] in PAIRS {
                    let left_sample = f64::from(frame[left]);
                    let right_sample = f64::from(frame[right]);
                    let mid = 0.5 * (left_sample + right_sample);
                    let side = 0.5 * (left_sample - right_sample);
                    let filtered_side = self.filters_by_channel[left].process(side);
                    frame[left] = (mid + filtered_side) as f32;
                    frame[right] = (mid - filtered_side) as f32;
                }
            }
        }
    }
}

fn placement_sequences() -> [(&'static str, &'static [EqBandPlacement]); 7] {
    use EqBandPlacement::{Left, Mid, Right, Side, Stereo};
    [
        ("stereo", &[Stereo]),
        ("left", &[Left]),
        ("right", &[Right]),
        ("mid", &[Mid]),
        ("side", &[Side]),
        ("left-mid", &[Left, Mid]),
        ("mid-left", &[Mid, Left]),
    ]
}

fn scenarios() -> Vec<Scenario> {
    let mut scenarios = [2usize, 4, 6, 8]
        .into_iter()
        .map(|order| Scenario {
            name: match order {
                2 => "biquad2",
                4 => "biquad4",
                6 => "biquad6",
                _ => "biquad8",
            },
            config_topology: EqFilterTopology::Biquad,
            reference_kind: ReferenceKind::Biquad,
            order,
            lambda: None,
            use_svf_topology: false,
        })
        .collect::<Vec<_>>();
    scenarios.extend([
        Scenario {
            name: "warped-auto",
            config_topology: EqFilterTopology::WarpedBiquad,
            reference_kind: ReferenceKind::Warped,
            order: 2,
            lambda: None,
            use_svf_topology: false,
        },
        Scenario {
            name: "warped-explicit",
            config_topology: EqFilterTopology::WarpedBiquad,
            reference_kind: ReferenceKind::Warped,
            order: 2,
            lambda: Some(0.37),
            use_svf_topology: false,
        },
        Scenario {
            name: "kautz-two-section",
            config_topology: EqFilterTopology::KautzFilter,
            reference_kind: ReferenceKind::Kautz,
            order: 2,
            lambda: None,
            use_svf_topology: false,
        },
        Scenario {
            name: "svf",
            config_topology: EqFilterTopology::Biquad,
            reference_kind: ReferenceKind::Svf,
            order: 2,
            lambda: None,
            use_svf_topology: true,
        },
    ]);
    scenarios
}

fn configs_for_sequence(
    sequence: &[EqBandPlacement],
    scenario: Scenario,
) -> Vec<BiquadFilterConfig> {
    sequence
        .iter()
        .enumerate()
        .map(|(index, placement)| {
            let (freq, q, db_gain) = if sequence.len() == 1 {
                (1_379.0, 0.83, 7.0)
            } else if index == 0 {
                (911.0, 0.71, 6.5)
            } else {
                (3_413.0, 1.17, -4.0)
            };
            BiquadFilterConfig {
                filter_type: "peak".into(),
                freq,
                q,
                db_gain,
                order: scenario.order,
                topology: scenario.config_topology,
                placement: Some(*placement),
                lambda: scenario.lambda,
                kautz_sections: if matches!(scenario.reference_kind, ReferenceKind::Kautz) {
                    vec![
                        KautzSectionConfig {
                            pole_freq: 4_700.0,
                            q: 1.7,
                            gain: -2.0,
                        },
                        KautzSectionConfig {
                            pole_freq: 10_000.0,
                            q: 2.0,
                            gain: 4.0,
                        },
                    ]
                } else {
                    Vec::new()
                },
            }
        })
        .collect()
}

fn input_vector() -> Vec<f32> {
    let mut input = Vec::with_capacity(FRAMES * CHANNELS);
    for frame in 0..FRAMES {
        let time = frame as f64 / f64::from(SAMPLE_RATE);
        let common = 0.31 * (2.0 * std::f64::consts::PI * 733.0 * time).sin();
        let side = 0.19 * (2.0 * std::f64::consts::PI * 1_681.0 * time).cos();
        let second_left = 0.17 * (2.0 * std::f64::consts::PI * 419.0 * time).sin();
        let second_right = 0.12 * (2.0 * std::f64::consts::PI * 2_203.0 * time).cos();
        let unpaired = 0.09 * (2.0 * std::f64::consts::PI * 3_117.0 * time).sin();
        input.extend_from_slice(&[
            (common + side) as f32,
            (common - side) as f32,
            second_right as f32,
            second_left as f32,
            unpaired as f32,
        ]);
    }
    input
}

fn render_public(plugin: &mut EqPlugin, input: &[f32]) -> Vec<f32> {
    let mut output = vec![0.0; input.len()];
    let block_samples = PROCESS_BLOCK * CHANNELS;
    for (input_block, output_block) in input
        .chunks(block_samples)
        .zip(output.chunks_mut(block_samples))
    {
        let frames = input_block.len() / CHANNELS;
        let processed = plugin
            .process(
                input_block,
                output_block,
                &ProcessContext::new(SAMPLE_RATE, frames),
            )
            .expect("process public ordered route");
        assert_eq!(processed, frames, "public EQ returned a short block");
    }
    assert_eq!(output.len(), input.len());
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert!(output.iter().any(|sample| sample.abs() > 1.0e-6));
    output
}

fn render_reference(
    input: &[f32],
    configs: &[BiquadFilterConfig],
    kind: ReferenceKind,
) -> Vec<f32> {
    let mut output = input.to_vec();
    let mut stages = configs
        .iter()
        .map(|config| ReferenceStage::new(config, kind, f64::from(SAMPLE_RATE), CHANNELS))
        .collect::<Vec<_>>();
    let (frames, remainder) = output.as_chunks_mut::<CHANNELS>();
    assert!(remainder.is_empty(), "output must contain complete frames");
    for frame in frames {
        for stage in &mut stages {
            stage.process_frame(frame);
        }
    }
    output
}

fn assert_matches_route_reference(actual: &[f32], expected: &[f32], case: &str) {
    assert_eq!(actual.len(), expected.len());
    let mut peak_error = 0.0f64;
    let mut squared_error = 0.0f64;
    for (&actual, &expected) in actual.iter().zip(expected) {
        assert!(
            actual.is_finite() && expected.is_finite(),
            "{case}: non-finite output"
        );
        let error = (f64::from(actual) - f64::from(expected)).abs();
        peak_error = peak_error.max(error);
        squared_error += error * error;
    }
    let rms_error = (squared_error / actual.len() as f64).sqrt();
    assert!(
        peak_error <= PEAK_LIMIT,
        "{case}: peak error {peak_error:e} exceeds {PEAK_LIMIT:e}"
    );
    assert!(
        rms_error <= RMS_LIMIT,
        "{case}: RMS error {rms_error:e} exceeds {RMS_LIMIT:e}"
    );
}

#[test]
fn advanced_and_svf_placements_match_independent_pair_composition() {
    let input = input_vector();
    for scenario in scenarios() {
        for (sequence_name, sequence) in placement_sequences() {
            let configs = configs_for_sequence(sequence, scenario);
            let mut plugin = EqPlugin::from_params(
                CHANNELS,
                SAMPLE_RATE,
                EqPluginParams {
                    filters: configs.clone(),
                    channel_filters: None,
                    stereo_pairs: Some(PAIRS.to_vec()),
                    auto_gain: AutoGainParams::default(),
                },
            )
            .unwrap_or_else(|error| panic!("{} {sequence_name}: {error}", scenario.name));
            plugin
                .parametric_set_parameter(
                    ParameterId::from("auto_gain_enabled"),
                    ParameterValue::Bool(false),
                )
                .expect("disable whole-plugin AutoGain for route reference");
            plugin
                .plugin_initialize(SAMPLE_RATE)
                .expect("initialize public route");
            if scenario.use_svf_topology {
                plugin
                    .parametric_set_parameter(ParameterId::from("topology"), ParameterValue::Int(1))
                    .expect("select second-order SVF route");
            }
            let actual = render_public(&mut plugin, &input);
            let expected = render_reference(&input, &configs, scenario.reference_kind);
            let case = format!("{}-{sequence_name}", scenario.name);
            assert_matches_route_reference(&actual, &expected, &case);
        }
    }
}
