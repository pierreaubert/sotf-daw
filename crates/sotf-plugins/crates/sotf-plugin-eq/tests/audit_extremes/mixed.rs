//! Independent mixed transfer/state references and callback-partition checks.
//!
//! These models use the parent's analytic polynomial, spectral-factor and modal
//! references, never production DSP instances. The legacy route casts after its
//! native bank and its combined advanced bank. Ordered routes cast per band.

// Rust guideline compliant 2026-02-21
use super::*;
use sotf_plugin_eq::KautzSectionConfig;

const PERMUTATIONS: [[usize; 3]; 6] = [
    [0, 1, 2],
    [0, 2, 1],
    [1, 0, 2],
    [1, 2, 0],
    [2, 0, 1],
    [2, 1, 0],
];

#[derive(Clone, Copy)]
struct Mode {
    kind: &'static str,
    order: usize,
    tdf2: bool,
    svf: bool,
}
const MODES: [Mode; 5] = [
    Mode {
        kind: "peak",
        order: 2,
        tdf2: false,
        svf: false,
    },
    Mode {
        kind: "peak",
        order: 8,
        tdf2: true,
        svf: false,
    },
    Mode {
        kind: "peak_matched",
        order: 8,
        tdf2: false,
        svf: false,
    },
    Mode {
        kind: "peak_matched",
        order: 2,
        tdf2: true,
        svf: false,
    },
    Mode {
        kind: "peak_matched",
        order: 2,
        tdf2: false,
        svf: true,
    },
];

fn pairs(channels: usize) -> Vec<[usize; 2]> {
    if channels == 2 {
        vec![[0, 1]]
    } else {
        vec![[0, 1], [3, 2]]
    }
}
fn placements(profile: usize) -> [Option<EqBandPlacement>; 3] {
    use EqBandPlacement::{Left, Mid, Right, Side, Stereo};
    match profile {
        0 => [None; 3],
        1 => [Some(Stereo); 3],
        2 => [Some(Left), Some(Mid), Some(Side)],
        3 => [Some(Side), Some(Right), Some(Left)],
        _ => unreachable!(),
    }
}
fn definitions(
    mode: Mode,
    lambda: Option<f64>,
    profile: usize,
    matched_warped: bool,
) -> [BiquadFilterConfig; 3] {
    let place = placements(profile);
    let mut native = config(
        Design {
            kind: mode.kind,
            freq: 800.0,
            q: 0.7,
            gain: 6.0,
            order: mode.order,
        },
        false,
    );
    native.placement = place[0];
    let mut warped = config(
        Design {
            kind: if matched_warped {
                "peak_matched"
            } else {
                "highshelf"
            },
            freq: 3100.0,
            q: 1.1,
            gain: -6.0,
            order: 2,
        },
        false,
    );
    warped.placement = place[1];
    warped.topology = EqFilterTopology::WarpedBiquad;
    warped.lambda = lambda;
    let mut modal = config(
        Design {
            kind: "peak",
            freq: 67.0,
            q: 20.0,
            gain: 0.0,
            order: 2,
        },
        false,
    );
    modal.placement = place[2];
    modal.topology = EqFilterTopology::KautzFilter;
    modal.kautz_sections = vec![
        KautzSectionConfig {
            pole_freq: 67.0,
            q: 20.0,
            gain: -0.02,
        },
        KautzSectionConfig {
            pole_freq: 71.0,
            q: 0.1,
            gain: 0.01,
        },
    ];
    [native, warped, modal]
}
fn make_plugin(
    configs: &[BiquadFilterConfig],
    mode: Mode,
    channels: usize,
    rate: u32,
    selected: i32,
) -> Box<dyn Plugin> {
    let parameters = EqPluginParams {
        filters: configs.to_vec(),
        channel_filters: None,
        stereo_pairs: (channels == 5).then(|| pairs(channels)),
        auto_gain: AutoGainParams::default(),
    };
    let mut p = EqPlugin::from_params(channels, rate, parameters)
        .unwrap()
        .into_boxed_plugin();
    p.set_parameter(
        ParameterId::from("auto_gain_enabled"),
        ParameterValue::Bool(false),
    )
    .unwrap();
    p.set_parameter(ParameterId::from("tdf2"), ParameterValue::Bool(mode.tdf2))
        .unwrap();
    if mode.svf {
        p.set_parameter(ParameterId::from("topology"), ParameterValue::Int(1))
            .unwrap();
        p.set_parameter(
            ParameterId::from("oversampling"),
            ParameterValue::Int(selected),
        )
        .unwrap();
        assert_eq!(
            p.get_parameter(&ParameterId::from("oversampling")),
            Some(ParameterValue::Int(selected))
        );
        assert_eq!(p.latency_samples(), 0);
    }
    p.initialize(f64::from(rate)).unwrap();
    assert_eq!(p.latency_samples(), 0);
    assert_eq!(p.tail_length(), sotf_host::TailLength::Unknown);
    assert_eq!(
        p.get_parameter(&ParameterId::from("band_0_freq")),
        Some(ParameterValue::Float(800.0))
    );
    p
}

fn design(c: &BiquadFilterConfig) -> Design {
    Design {
        kind: match c.filter_type.as_str() {
            "peak" => "peak",
            "peak_matched" => "peak_matched",
            "lowshelf" => "lowshelf",
            "highshelf" => "highshelf",
            "lowshelf_orf" => "lowshelf_orf",
            "highshelf_orf" => "highshelf_orf",
            "lowpass" => "lowpass",
            "highpass" => "highpass",
            "notch" => "notch",
            "bandpass" => "bandpass",
            "allpass" => "allpass",
            _ => unreachable!(),
        },
        freq: c.freq,
        q: c.q,
        gain: c.db_gain,
        order: c.order,
    }
}
fn native_coefficients(d: Design, rate: u32, svf: bool) -> Vec<[f64; 5]> {
    if svf {
        vec![svf_coefficients(d, rate)]
    } else if matches!(d.kind, "peak_matched" | "lowshelf_orf" | "highshelf_orf") {
        optimized_coefficients(d, rate)
    } else {
        coefficients(d, rate)
    }
}
fn lerp(a: [f64; 5], b: [f64; 5], t: f64) -> [f64; 5] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}
struct Sections {
    d: Design,
    target: Vec<[f64; 5]>,
    old: Vec<[f64; 5]>,
    states: Vec<Vec<State>>,
    elapsed: usize,
    length: usize,
    tdf2: bool,
}
impl Sections {
    fn coefficient(&self, index: usize) -> [f64; 5] {
        if self.elapsed >= self.length {
            self.target[index]
        } else {
            lerp(
                self.old[index],
                self.target[index],
                self.elapsed as f64 / self.length as f64,
            )
        }
    }
    fn process(&mut self, ch: usize, mut x: f64) -> f64 {
        for i in 0..self.target.len() {
            let c = self.coefficient(i);
            x = self.states[ch][i].process(x, c, self.tdf2);
        }
        x
    }
    fn edit(&mut self, field: &str, value: f32, rate: u32) {
        self.old = (0..self.target.len())
            .map(|i| self.coefficient(i))
            .collect();
        match field {
            "freq" => self.d.freq = f64::from(value),
            "q" => self.d.q = f64::from(value),
            "gain" => self.d.gain = f64::from(value),
            _ => unreachable!(),
        }
        self.target = native_coefficients(self.d, rate, false);
        self.length = (f64::from(rate) * 0.005) as usize;
        self.elapsed = 0;
    }
}
enum Core {
    Sections(Sections),
    Modal(Vec<ModalReference>),
    WarpedHistory(Vec<ImplicitWarpedReference>),
}
struct Band {
    native: bool,
    placement: Option<EqBandPlacement>,
    core: Core,
}
impl Band {
    fn new(c: &BiquadFilterConfig, mode: Mode, channels: usize, rate: u32) -> Self {
        let native = c.topology == EqFilterTopology::Biquad;
        let core = if c.topology == EqFilterTopology::KautzFilter {
            Core::Modal(
                (0..channels)
                    .map(|_| ModalReference::new(&c.kautz_sections, rate))
                    .collect(),
            )
        } else {
            let d = design(c);
            let target = if native {
                native_coefficients(d, rate, mode.svf)
            } else if d.kind == "peak_matched" {
                let l = c.lambda.unwrap_or_else(|| {
                    1.0674 * ((2.0 / PI) * (0.06583 * f64::from(rate) / 1000.0).atan()).sqrt()
                        - 0.1916
                });
                vec![warped_matched_coefficients(d, rate, l)]
            } else {
                coefficients(d, rate)
            };
            let states = vec![vec![State::default(); target.len()]; channels];
            Core::Sections(Sections {
                d,
                old: target.clone(),
                target,
                states,
                elapsed: 0,
                length: 0,
                tdf2: native && mode.tdf2 && !mode.svf,
            })
        };
        Self {
            native,
            placement: c.placement,
            core,
        }
    }
    fn process(&mut self, ch: usize, x: f64) -> f64 {
        match &mut self.core {
            Core::Sections(s) => s.process(ch, x),
            Core::Modal(m) => m[ch].process(x),
            Core::WarpedHistory(m) => m[ch].process(x),
        }
    }
    fn frame(&mut self, frame: &mut [f32], pairs: &[[usize; 2]]) {
        use EqBandPlacement::{Left, Mid, Right, Side, Stereo};
        match self.placement.unwrap_or(Stereo) {
            Stereo => {
                for (ch, x) in frame.iter_mut().enumerate() {
                    *x = self.process(ch, f64::from(*x)) as f32;
                }
            }
            placement => {
                for &[left, right] in pairs {
                    let l = f64::from(frame[left]);
                    let r = f64::from(frame[right]);
                    match placement {
                        Left => frame[left] = self.process(left, l) as f32,
                        Right => frame[right] = self.process(right, r) as f32,
                        Mid => {
                            let m = self.process(left, (l + r) / 2.0);
                            let s = (l - r) / 2.0;
                            frame[left] = (m + s) as f32;
                            frame[right] = (m - s) as f32;
                        }
                        Side => {
                            let s = self.process(left, (l - r) / 2.0);
                            let m = (l + r) / 2.0;
                            frame[left] = (m + s) as f32;
                            frame[right] = (m - s) as f32;
                        }
                        Stereo => unreachable!(),
                    }
                }
            }
        }
    }
}
struct MixedReference {
    bands: Vec<Band>,
    pairs: Vec<[usize; 2]>,
    ordered: bool,
}
impl MixedReference {
    fn new(configs: &[BiquadFilterConfig], mode: Mode, channels: usize, rate: u32) -> Self {
        Self {
            bands: configs
                .iter()
                .map(|c| Band::new(c, mode, channels, rate))
                .collect(),
            pairs: pairs(channels),
            ordered: configs.iter().any(|c| c.placement.is_some()),
        }
    }
    fn frame(&mut self, frame: &mut [f32]) {
        if self.ordered {
            for b in &mut self.bands {
                b.frame(frame, &self.pairs);
            }
        } else {
            for (ch, x) in frame.iter_mut().enumerate() {
                let mut value = f64::from(*x);
                for b in self.bands.iter_mut().filter(|b| b.native) {
                    value = b.process(ch, value);
                }
                value = f64::from(value as f32);
                for b in self.bands.iter_mut().filter(|b| !b.native) {
                    value = b.process(ch, value);
                }
                *x = value as f32;
            }
        }
        for b in self.bands.iter_mut().filter(|b| b.native) {
            if let Core::Sections(s) = &mut b.core
                && s.elapsed < s.length
            {
                s.elapsed += 1;
            }
        }
    }
    fn edit(&mut self, event: Event, rate: u32) {
        let native = self.bands.iter_mut().find(|b| b.native).unwrap();
        let Core::Sections(s) = &mut native.core else {
            unreachable!()
        };
        s.edit(event.field, event.value, rate);
    }
}
#[derive(Clone, Copy)]
struct Event {
    frame: usize,
    field: &'static str,
    value: f32,
}
fn events() -> Vec<Event> {
    [
        (17, "freq", 1200.0),
        (18, "q", 2.3),
        (63, "gain", -9.0),
        (221, "freq", 20000.0),
        (222, "q", 20.0),
        (223, "gain", 24.0),
        (509, "gain", 0.0),
        (733, "freq", 20.0),
        (734, "q", 0.1),
        (1200, "gain", -24.0),
        (1501, "gain", 6.0),
        (1501, "freq", 997.0),
    ]
    .into_iter()
    .map(|(frame, field, value)| Event {
        frame,
        field,
        value,
    })
    .collect()
}
fn input(channels: usize, rate: u32) -> Vec<f32> {
    (0..3075 * channels)
        .map(|i| {
            let n = i / channels;
            let ch = i % channels;
            if n >= 2300 {
                return 0.0;
            }
            let t = 2.0 * PI * n as f64 / f64::from(rate);
            (0.006 * ((73.0 + ch as f64 * 13.0) * t).sin()
                + 0.004 * ((1733.0 - ch as f64 * 71.0) * t + ch as f64 * 0.7).cos()
                + if n == 37 + ch { 0.01 } else { 0.0 }) as f32
        })
        .collect()
}
fn expected(
    configs: &[BiquadFilterConfig],
    mode: Mode,
    channels: usize,
    rate: u32,
    input: &[f32],
    events: &[Event],
) -> Vec<f32> {
    let mut model = MixedReference::new(configs, mode, channels, rate);
    let mut output = input.to_vec();
    let mut e = 0;
    for (n, frame) in output.chunks_exact_mut(channels).enumerate() {
        while e < events.len() && events[e].frame == n {
            model.edit(events[e], rate);
            e += 1;
        }
        model.frame(frame);
    }
    assert!(output.iter().all(|x| x.is_finite()));
    output
}
fn render_plugin(
    p: &mut dyn Plugin,
    channels: usize,
    rate: u32,
    input: &[f32],
    events: &[Event],
    blocks: &[usize],
) -> Vec<f32> {
    let mut output = vec![f32::NAN; input.len()];
    let mut frame = 0;
    let mut e = 0;
    let mut block = 0;
    while frame < input.len() / channels {
        while e < events.len() && events[e].frame == frame {
            let event = events[e];
            let id = ParameterId::from(format!("band_0_{}", event.field));
            // Retain the host-owned Arc outside the counter, as in the existing
            // realtime harness, so its final caller-owned drop is not measured.
            let heap = sotf_host::test_utils::measure_heap_activity(|| {
                p.set_parameter(id.clone(), ParameterValue::Float(event.value))
                    .unwrap()
            });
            assert_eq!(heap, (0, 0));
            e += 1;
        }
        let end = events.get(e).map_or(input.len() / channels, |e| e.frame);
        let frames = blocks[block % blocks.len()]
            .min(end - frame)
            .min(input.len() / channels - frame);
        assert!(frames > 0);
        let span = frame * channels..(frame + frames) * channels;
        let heap = sotf_host::test_utils::measure_heap_activity(|| {
            assert_eq!(
                p.process(
                    &input[span.clone()],
                    &mut output[span],
                    &sotf_host::ProcessContext::new(rate, frames)
                )
                .unwrap(),
                frames
            )
        });
        assert_eq!(heap, (0, 0));
        frame += frames;
        block += 1;
    }
    assert!(output.iter().all(|x| x.is_finite()));
    output
}

fn actual(
    mut p: Box<dyn Plugin>,
    channels: usize,
    rate: u32,
    input: &[f32],
    events: &[Event],
    blocks: &[usize],
) -> Vec<f32> {
    render_plugin(p.as_mut(), channels, rate, input, events, blocks)
}

fn matrix(automated: bool) {
    let script = if automated { events() } else { vec![] };
    let mut cases = 0;
    let mut maximum = 0.0_f64;
    for (ri, rate) in [44100, 48000, 96000, 192000].into_iter().enumerate() {
        for channels in [2, 5] {
            let input = input(channels, rate);
            for mode in MODES.into_iter().filter(|m| !automated || !m.svf) {
                for (pi, permutation) in PERMUTATIONS.into_iter().enumerate() {
                    for profile in 0..4 {
                        let lambda = [Some(-0.9999), Some(0.37), Some(0.9999), None]
                            [(ri + pi + profile) % 4];
                        let definitions = definitions(mode, lambda, profile, pi % 2 == 0);
                        let configs: Vec<_> = permutation
                            .into_iter()
                            .map(|i| definitions[i].clone())
                            .collect();
                        let selected = [1, 2, 4][profile % 3];
                        let expected = expected(&configs, mode, channels, rate, &input, &script);
                        let first = actual(
                            make_plugin(&configs, mode, channels, rate, selected),
                            channels,
                            rate,
                            &input,
                            &script,
                            &[1024, 257, 17],
                        );
                        let second = actual(
                            make_plugin(&configs, mode, channels, rate, selected),
                            channels,
                            rate,
                            &input,
                            &script,
                            &[1, 17, 193, 509],
                        );
                        let difference = error(&first, &expected);
                        maximum = maximum.max(difference);
                        assert!(
                            difference < 2e-5,
                            "mixed {difference}: {automated}/{rate}/{channels}/{}/{}/{}/{}/{pi}/{profile}/{lambda:?}",
                            mode.kind,
                            mode.order,
                            mode.tdf2,
                            mode.svf
                        );
                        assert_eq!(
                            first, second,
                            "mixed callback partition must be bit-identical"
                        );
                        if channels == 5 && profile >= 2 {
                            for (i, o) in input
                                .chunks_exact(channels)
                                .zip(first.chunks_exact(channels))
                            {
                                assert_eq!(i[4], o[4]);
                            }
                        }
                        cases += 1;
                    }
                }
            }
        }
    }
    assert_eq!(cases, if automated { 768 } else { 960 });
    println!(
        "{}",
        serde_json::json!({"case":if automated {"mixed_automated"} else {"mixed_static"},"cases":cases,"maximum_reference_error":maximum})
    );
}
#[test]
fn mixed_static_all_permutations_match_independent_realizations() {
    matrix(false);
}
#[test]
fn interrupted_native_trajectories_among_advanced_neighbors_match_independent_states() {
    matrix(true);
}

#[test]
fn noncommuting_placement_reference_has_a_measurable_order_witness() {
    let mode = MODES[0];
    let channels = 2;
    let rate = 48000;
    let definitions = definitions(mode, Some(0.37), 2, false);
    let input = input(channels, rate);
    let forward = expected(&definitions, mode, channels, rate, &input, &[]);
    let reverse: Vec<_> = definitions.into_iter().rev().collect();
    let reversed = expected(&reverse, mode, channels, rate, &input, &[]);
    let difference = error(&forward, &reversed);
    println!("mixed noncommuting order witness={difference}");
    assert!(
        difference > 1e-4,
        "the oracle must distinguish noncommuting band order"
    );
}

fn channel_configs(
    mode: Mode,
    permutation: [usize; 3],
    channels: usize,
) -> Vec<Vec<BiquadFilterConfig>> {
    (0..channels)
        .map(|ch| {
            let mut defs = definitions(mode, Some(0.37), 0, false);
            defs[0].freq += 101.0 * ch as f64;
            defs[0].db_gain -= 3.0 * ch as f64;
            defs[1].freq += 300.0 * ch as f64;
            defs[1].db_gain += 3.0 * ch as f64;
            for modal in &mut defs[2].kautz_sections {
                modal.pole_freq += 17.0 * ch as f64;
                modal.gain *= 1.0 + ch as f64;
            }
            permutation
                .into_iter()
                .filter(|&i| match ch {
                    0 => true,
                    1 => i != 2,
                    2 => i == 2,
                    3 => false,
                    4 => i != 1,
                    _ => unreachable!(),
                })
                .map(|i| defs[i].clone())
                .collect()
        })
        .collect()
}

fn per_channel_expected(
    configs: &[Vec<BiquadFilterConfig>],
    mode: Mode,
    channels: usize,
    rate: u32,
    input: &[f32],
) -> Vec<f32> {
    let mut models: Vec<_> = configs
        .iter()
        .map(|c| MixedReference::new(c, mode, 1, rate))
        .collect();
    let mut output = input.to_vec();
    for frame in output.chunks_exact_mut(channels) {
        for (sample, model) in frame.iter_mut().zip(&mut models) {
            model.frame(std::slice::from_mut(sample));
        }
    }
    assert!(output.iter().all(|x| x.is_finite()));
    output
}

#[test]
fn per_channel_mixed_descriptions_survive_topology_rebuilds() {
    let mut cases = 0;
    let mut round_trips = 0;
    let mut failures = 0;
    let mut maximum = 0.0_f64;
    for rate in [44100, 48000, 96000, 192000] {
        for channels in [2, 5] {
            let input = input(channels, rate);
            for permutation in PERMUTATIONS {
                for (svf, selected) in [(false, 1), (true, 1), (true, 2), (true, 4)] {
                    let mode = if svf { MODES[4] } else { MODES[0] };
                    let channel_configs = channel_configs(mode, permutation, channels);
                    let expected =
                        per_channel_expected(&channel_configs, mode, channels, rate, &input);
                    let parameters = EqPluginParams {
                        filters: vec![],
                        channel_filters: Some(channel_configs.clone()),
                        stereo_pairs: None,
                        auto_gain: AutoGainParams::default(),
                    };
                    let mut p = EqPlugin::from_params(channels, rate, parameters)
                        .unwrap()
                        .into_boxed_plugin();
                    p.set_parameter(
                        ParameterId::from("auto_gain_enabled"),
                        ParameterValue::Bool(false),
                    )
                    .unwrap();
                    if svf {
                        p.set_parameter(ParameterId::from("topology"), ParameterValue::Int(1))
                            .unwrap();
                        p.set_parameter(
                            ParameterId::from("oversampling"),
                            ParameterValue::Int(selected),
                        )
                        .unwrap();
                    }
                    p.initialize(f64::from(rate)).unwrap();
                    assert_eq!(p.latency_samples(), 0);
                    assert_eq!(p.tail_length(), sotf_host::TailLength::Unknown);
                    assert_eq!(
                        p.get_parameter(&ParameterId::from("oversampling")),
                        Some(ParameterValue::Int(selected))
                    );
                    let rendered =
                        render_plugin(p.as_mut(), channels, rate, &input, &[], &[1, 17, 193, 509]);
                    let difference = error(&rendered, &expected);
                    maximum = maximum.max(difference);
                    if difference >= 2e-5 {
                        failures += 1;
                        if failures < 4 {
                            println!(
                                "per-channel rebuild {rate}/{channels}/{svf}/{selected}: {difference}"
                            );
                        }
                    }
                    if svf {
                        // Return with populated state. A topology switch resets
                        // both realizations, but must retain each channel's bank.
                        p.set_parameter(ParameterId::from("oversampling"), ParameterValue::Int(1))
                            .unwrap();
                        p.set_parameter(ParameterId::from("topology"), ParameterValue::Int(0))
                            .unwrap();
                        let back_mode = Mode { svf: false, ..mode };
                        let expected_back = per_channel_expected(
                            &channel_configs,
                            back_mode,
                            channels,
                            rate,
                            &input,
                        );
                        let back = render_plugin(
                            p.as_mut(),
                            channels,
                            rate,
                            &input,
                            &[],
                            &[1024, 257, 17],
                        );
                        let difference = error(&back, &expected_back);
                        maximum = maximum.max(difference);
                        assert!(
                            difference < 2e-5,
                            "per-channel round trip {difference}: {rate}/{channels}/{selected}"
                        );
                        assert_eq!(p.latency_samples(), 0);
                        assert_eq!(p.tail_length(), sotf_host::TailLength::Unknown);
                        round_trips += 1;
                    }
                    cases += 1;
                }
            }
        }
    }
    println!(
        "{}",
        serde_json::json!({"case":"mixed_per_channel_rebuilds","cases":cases,"populated_round_trips":round_trips,"failures":failures,"maximum_reference_error":maximum})
    );
    assert_eq!(cases, 192);
    assert_eq!(round_trips, 144);
    assert_eq!(failures, 0);
}

#[test]
fn per_channel_advanced_only_banks_stay_recursive_through_topology_and_drain() {
    let mut cases = 0;
    let mut maximum = 0.0_f64;
    let mut smallest_tail = f64::INFINITY;
    for rate in [44100, 48000, 96000, 192000] {
        for channels in [2, 5] {
            let mode = MODES[0];
            let defs = definitions(mode, Some(0.37), 0, false);
            let configs: Vec<Vec<_>> = (0..channels)
                .map(|ch| match ch {
                    0 | 4 => vec![defs[1].clone(), defs[2].clone()],
                    1 => vec![defs[2].clone()],
                    2 => vec![defs[1].clone()],
                    3 => vec![],
                    _ => unreachable!(),
                })
                .collect();
            let parameters = EqPluginParams {
                filters: vec![],
                channel_filters: Some(configs.clone()),
                stereo_pairs: None,
                auto_gain: AutoGainParams::default(),
            };
            let mut p = EqPlugin::from_params(channels, rate, parameters)
                .unwrap()
                .into_boxed_plugin();
            p.set_parameter(
                ParameterId::from("auto_gain_enabled"),
                ParameterValue::Bool(false),
            )
            .unwrap();
            p.initialize(f64::from(rate)).unwrap();
            let input = input(channels, rate);
            let silence = vec![0.0; 257 * channels];
            let extended: Vec<_> = input.iter().chain(&silence).copied().collect();
            let expected = per_channel_expected(&configs, mode, channels, rate, &extended);
            let expected_tail = &expected[input.len()..];
            let tail_peak = expected_tail
                .iter()
                .map(|&x| f64::from(x).abs())
                .fold(0.0_f64, f64::max);
            smallest_tail = smallest_tail.min(tail_peak);
            assert!(
                tail_peak > 1e-12,
                "the drain continuation witness needs a populated recursive tail"
            );
            for phase in 0..3 {
                if phase == 1 {
                    p.set_parameter(ParameterId::from("topology"), ParameterValue::Int(1))
                        .unwrap();
                    p.set_parameter(ParameterId::from("oversampling"), ParameterValue::Int(4))
                        .unwrap();
                } else if phase == 2 {
                    p.set_parameter(ParameterId::from("oversampling"), ParameterValue::Int(1))
                        .unwrap();
                    p.set_parameter(ParameterId::from("topology"), ParameterValue::Int(0))
                        .unwrap();
                }
                assert_eq!(p.latency_samples(), 0);
                assert_eq!(p.tail_length(), sotf_host::TailLength::Unknown);
                let rendered =
                    render_plugin(p.as_mut(), channels, rate, &input, &[], &[17, 257, 1024]);
                let difference = error(&rendered, &expected[..input.len()]);
                maximum = maximum.max(difference);
                assert!(
                    difference < 2e-5,
                    "advanced-only {difference}: {rate}/{channels}/{phase}"
                );
                let mut canary = vec![123.0; 137 * channels + 3];
                let heap = sotf_host::test_utils::measure_heap_activity(|| {
                    assert_eq!(
                        p.drain(&mut canary, &sotf_host::ProcessContext::new(rate, 0))
                            .unwrap(),
                        sotf_host::plugin::PluginDrainResult::COMPLETE
                    )
                });
                assert_eq!(heap, (0, 0));
                assert!(canary.iter().all(|&x| x == 123.0));
                // Unknown recursive support retains legacy COMPLETE without
                // flushing/resetting history; it does not promise finite support.
                assert_eq!(p.tail_length(), sotf_host::TailLength::Unknown);
                let tail = render_plugin(p.as_mut(), channels, rate, &silence, &[], &[1, 17, 193]);
                let difference = error(&tail, expected_tail);
                maximum = maximum.max(difference);
                assert!(
                    difference < 2e-5,
                    "post-drain state {difference}: {rate}/{channels}/{phase}"
                );
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 24);
    println!(
        "{}",
        serde_json::json!({"case":"mixed_advanced_only_tail","cases":cases,"maximum_reference_error":maximum,"smallest_nonzero_reference_tail_peak":smallest_tail})
    );
}

#[path = "oversampled_host.rs"]
mod oversampled_host;
