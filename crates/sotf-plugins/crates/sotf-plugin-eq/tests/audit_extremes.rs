//! Standard EQ extremes against prewarped analog prototypes and explicit state.
//!
//! Fixed bounds: 4096-frame unit-impulse prefixes, absolute output error 2e-5.
//! The reference uses tan-prewarping and polynomial bilinear substitution, not
//! the production trigonometric design or its processing helpers. Shelf slope
//! is the standard fixed-slope design; its Q metadata does not alter the slope.

// Rust guideline compliant 2026-02-21
use sotf_host::{AutoGainParams, ParameterId, ParameterValue, Plugin};
use sotf_plugin_eq::{
    BiquadFilterConfig, EqBandPlacement, EqFilterTopology, EqPlugin, EqPluginParams,
};
use std::f64::consts::PI;

#[global_allocator]
static ALLOCATOR: sotf_host::CountingAlloc = sotf_host::CountingAlloc;

#[derive(Clone, Copy)]
struct Design {
    kind: &'static str,
    freq: f64,
    q: f64,
    gain: f64,
    order: usize,
}

fn config(d: Design, ordered: bool) -> BiquadFilterConfig {
    BiquadFilterConfig {
        filter_type: d.kind.into(),
        freq: d.freq,
        q: d.q,
        db_gain: d.gain,
        order: d.order,
        topology: EqFilterTopology::Biquad,
        placement: ordered.then_some(EqBandPlacement::Stereo),
        lambda: None,
        kautz_sections: vec![],
    }
}
fn params(d: Design, ordered: bool) -> EqPluginParams {
    EqPluginParams {
        filters: vec![config(d, ordered)],
        channel_filters: None,
        stereo_pairs: None,
        auto_gain: AutoGainParams::default(),
    }
}
fn prepared(d: Design, rate: u32, tdf2: bool, ordered: bool) -> Box<dyn Plugin> {
    let mut p = EqPlugin::from_params(1, rate, params(d, ordered))
        .unwrap_or_else(|e| panic!("{}/{}/{}/{}/{rate}: {e}", d.kind, d.freq, d.q, d.gain))
        .into_boxed_plugin();
    p.set_parameter(
        ParameterId::from("auto_gain_enabled"),
        ParameterValue::Bool(false),
    )
    .unwrap();
    p.set_parameter(ParameterId::from("tdf2"), ParameterValue::Bool(tdf2))
        .unwrap();
    p.initialize(f64::from(rate)).unwrap();
    p
}

// Bilinear transform of a quadratic analog polynomial, with s=(1-z^-1)/(k(1+z^-1)).
fn bilinear(p: [f64; 3], k: f64) -> [f64; 3] {
    let [constant, linear, quadratic] = p;
    [
        constant * k * k + linear * k + quadratic,
        2.0 * (constant * k * k - quadratic),
        constant * k * k - linear * k + quadratic,
    ]
}
fn coefficients(d: Design, rate: u32) -> Vec<[f64; 5]> {
    let count = d.order / 2;
    let k = (PI * d.freq / f64::from(rate)).tan();
    let a = 10.0_f64.powf(d.gain / count as f64 / 40.0);
    (0..count)
        .map(|stage| {
            let prototype =
                1.0 / (2.0 * (PI * (2 * stage + 1) as f64 / (2 * d.order) as f64).cos());
            let q = if count == 1 {
                d.q
            } else if matches!(d.kind, "peak" | "notch" | "bandpass" | "allpass") {
                d.q * prototype
            } else {
                prototype
            };
            let (num, den) = match d.kind {
                "lowpass" => ([1.0, 0.0, 0.0], [1.0, 1.0 / q, 1.0]),
                "highpass" => ([0.0, 0.0, 1.0], [1.0, 1.0 / q, 1.0]),
                "bandpass" => ([0.0, 1.0 / q, 0.0], [1.0, 1.0 / q, 1.0]),
                "notch" => ([1.0, 0.0, 1.0], [1.0, 1.0 / q, 1.0]),
                "allpass" => ([1.0, -1.0 / q, 1.0], [1.0, 1.0 / q, 1.0]),
                "peak" => ([1.0, a / q, 1.0], [1.0, 1.0 / (a * q), 1.0]),
                "lowshelf" => ([a * a, a * (2.0 * a).sqrt(), a], [1.0, (2.0 * a).sqrt(), a]),
                "highshelf" => ([a, a * (2.0 * a).sqrt(), a * a], [a, (2.0 * a).sqrt(), 1.0]),
                _ => unreachable!(),
            };
            let b = bilinear(num, k);
            let c = bilinear(den, k);
            let section = [
                b[0] / c[0],
                b[1] / c[0],
                b[2] / c[0],
                c[1] / c[0],
                c[2] / c[0],
            ];
            assert!(section.iter().all(|x| x.is_finite()));
            assert!(
                1.0 + section[3] + section[4] > 0.0
                    && 1.0 - section[3] + section[4] > 0.0
                    && 1.0 - section[4] > 0.0,
                "unstable independent section"
            );
            section
        })
        .collect()
}
#[derive(Clone, Copy, Default)]
struct State {
    x: [f64; 2],
    y: [f64; 2],
    z: [f64; 2],
}
impl State {
    fn process(&mut self, x: f64, c: [f64; 5], tdf2: bool) -> f64 {
        if tdf2 {
            let y = c[0] * x + self.z[0];
            self.z[0] = c[1] * x - c[3] * y + self.z[1];
            self.z[1] = c[2] * x - c[4] * y;
            y
        } else {
            let y = c[0] * x + c[1] * self.x[0] + c[2] * self.x[1]
                - c[3] * self.y[0]
                - c[4] * self.y[1];
            self.x = [x, self.x[0]];
            self.y = [y, self.y[0]];
            y
        }
    }
}
fn render(p: &mut dyn Plugin, input: &[f32], rate: u32) -> Vec<f32> {
    let mut out = vec![f32::NAN; input.len()];
    let mut offset = 0;
    for frames in [17, 257, 1024].into_iter().cycle() {
        let frames = frames.min(input.len() - offset);
        if frames == 0 {
            break;
        }
        let heap = sotf_host::test_utils::measure_heap_activity(|| {
            assert_eq!(
                p.process(
                    &input[offset..offset + frames],
                    &mut out[offset..offset + frames],
                    &sotf_host::ProcessContext::new(rate, frames)
                )
                .unwrap(),
                frames
            )
        });
        assert_eq!(heap, (0, 0));
        offset += frames;
    }
    assert!(out.iter().all(|x| x.is_finite()));
    out
}

#[test]
fn standard_family_extreme_impulses_match_independent_bilinear_references() {
    let mut impulse = vec![0.0; 4096];
    impulse[0] = 1.0;
    let mut cases = 0;
    let mut maximum = 0.0_f64;
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for kind in [
            "peak",
            "lowshelf",
            "highshelf",
            "lowpass",
            "highpass",
            "notch",
            "bandpass",
            "allpass",
        ] {
            for freq in [20.0, 1000.0, 20000.0] {
                for q in [0.1, if kind == "notch" { 40.0 } else { 20.0 }] {
                    for gain in [-24.0, 24.0] {
                        for order in [2, 4, 6, 8] {
                            let d = Design {
                                kind,
                                freq,
                                q,
                                gain,
                                order,
                            };
                            let coefficients = coefficients(d, rate);
                            for tdf2 in [false, true] {
                                let mut states = vec![State::default(); order / 2];
                                let expected: Vec<_> = impulse
                                    .iter()
                                    .map(|&input| {
                                        let mut sample = f64::from(input);
                                        for (state, &c) in states.iter_mut().zip(&coefficients) {
                                            sample = state.process(sample, c, tdf2);
                                        }
                                        sample as f32
                                    })
                                    .collect();
                                for ordered in [false, true] {
                                    let mut p = prepared(d, rate, tdf2, ordered);
                                    let actual = render(p.as_mut(), &impulse, rate);
                                    let error = actual
                                        .iter()
                                        .zip(&expected)
                                        .map(|(&a, &b)| (f64::from(a) - f64::from(b)).abs())
                                        .fold(0.0_f64, f64::max);
                                    maximum = maximum.max(error);
                                    assert!(
                                        error < 2e-5,
                                        "impulse error {error}: {rate}/{kind}/{freq}/{q}/{gain}/{order}/{tdf2}/{ordered}"
                                    );
                                    cases += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    assert_eq!(cases, 6144);
    println!(
        "{}",
        serde_json::json!({"case":"standard_family_extremes","cases":cases,"maximum_reference_error":maximum})
    );
}

#[test]
fn exact_json_q_minimum_is_accepted_and_its_predecessor_is_refused() {
    let json =
        serde_json::json!({"filters":[{"filter_type":"peak","freq":1000.0,"q":0.1,"db_gain":0.0}]});
    let parsed: EqPluginParams = serde_json::from_value(json).unwrap();
    let mut p = EqPlugin::from_params(1, 48_000, parsed).expect("exact JSON Q minimum must load");
    let d = Design {
        kind: "peak",
        freq: 1000.0,
        q: 0.1_f64.next_down(),
        gain: 0.0,
        order: 2,
    };
    assert!(EqPlugin::from_params(1, 48_000, params(d, false)).is_err());
    use math_audio_iir_fir::{Biquad, BiquadFilterType};
    p.set_filters(vec![Biquad::new(
        BiquadFilterType::Peak,
        1000.0,
        48_000.0,
        0.1,
        0.0,
    )])
    .expect("replacement Q minimum must load");
    assert!(
        p.set_filters(vec![Biquad::new(
            BiquadFilterType::Peak,
            1000.0,
            48_000.0,
            d.q,
            0.0
        )])
        .is_err()
    );
}

// A rate-metadata fixture with exact frame duplication, not a resampler.
struct RateDoubler;
impl Plugin for RateDoubler {
    fn info(&self) -> sotf_host::PluginInfo {
        sotf_host::PluginInfo::new("rate fixture", "1", "test")
    }
    fn input_channels(&self) -> usize {
        1
    }
    fn output_channels(&self) -> usize {
        1
    }
    fn parameters(&self) -> Vec<sotf_host::Parameter> {
        vec![]
    }
    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
        Err("no parameters".into())
    }
    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }
    fn output_frames_for_input(&self, frames: usize) -> usize {
        frames * 2
    }
    fn output_sample_rate(&self, rate: f64) -> f64 {
        rate * 2.0
    }
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &sotf_host::ProcessContext,
    ) -> Result<usize, String> {
        for (frame, &sample) in input[..context.num_frames].iter().enumerate() {
            output[2 * frame] = sample;
            output[2 * frame + 1] = sample;
        }
        Ok(context.num_frames * 2)
    }
}
fn host_render(host: &mut sotf_host::host::DawHost, input: &[f32], factor: usize) -> Vec<f32> {
    let mut output = vec![f32::NAN; input.len() * factor];
    for (source, destination) in input.chunks(193).zip(output.chunks_mut(193 * factor)) {
        let heap = sotf_host::test_utils::measure_heap_activity(|| {
            assert_eq!(
                host.process(source, destination).unwrap(),
                source.len() * factor
            )
        });
        assert_eq!(heap, (0, 0));
    }
    assert!(output.iter().all(|x| x.is_finite()));
    output
}
fn reference_with_states(
    input: &[f32],
    design: Design,
    rate: u32,
    tdf2: bool,
    states: &mut [State],
) -> Vec<f32> {
    let coefficients = coefficients(design, rate);
    input
        .iter()
        .map(|&sample| {
            let mut value = f64::from(sample);
            for (state, &c) in states.iter_mut().zip(&coefficients) {
                value = state.process(value, c, tdf2);
            }
            value as f32
        })
        .collect()
}
fn error(actual: &[f32], expected: &[f32]) -> f64 {
    assert_eq!(actual.len(), expected.len());
    actual
        .iter()
        .zip(expected)
        .map(|(&a, &b)| (f64::from(a) - f64::from(b)).abs())
        .fold(0.0_f64, f64::max)
}

#[test]
fn retained_eq_rate_changes_through_host_match_independent_state_and_reset_policies() {
    use sotf_host::host::DawHost;
    let mut warm = vec![0.0; 1024];
    warm[0] = 0.25;
    let duplicated: Vec<_> = warm.iter().flat_map(|&sample| [sample, sample]).collect();
    let mut input = vec![0.0; 1536];
    input[7] = 0.25;
    let mut cases = 0;
    let mut maximum = 0.0_f64;
    for rate in [44_100, 48_000, 96_000] {
        for kind in [
            "peak",
            "lowshelf",
            "highshelf",
            "lowpass",
            "highpass",
            "notch",
            "bandpass",
            "allpass",
        ] {
            for order in [2, 8] {
                for tdf2 in [false, true] {
                    for ordered in [false, true] {
                        for compiled in [false, true] {
                            let d = Design {
                                kind,
                                freq: 20.0,
                                q: if kind == "notch" { 40.0 } else { 20.0 },
                                gain: 12.0,
                                order,
                            };
                            let mut host = DawHost::new(1, rate);
                            host.set_parallel_enabled(false).unwrap();
                            host.set_compiled_linear_enabled(compiled).unwrap();
                            host.add_plugin(Box::new(RateDoubler)).unwrap();
                            host.add_plugin(prepared(d, rate * 2, tdf2, ordered))
                                .unwrap();
                            host.build().unwrap();
                            assert_eq!(host.output_sample_rate(rate).unwrap(), f64::from(rate * 2));
                            let mut states = vec![State::default(); order / 2];
                            let expected_warm =
                                reference_with_states(&duplicated, d, rate * 2, tdf2, &mut states);
                            let actual_warm = host_render(&mut host, &warm, 2);
                            let warm_error = error(&actual_warm, &expected_warm);
                            maximum = maximum.max(warm_error);
                            assert!(warm_error < 2e-5, "old-rate reference error {warm_error}");
                            drop(host.remove_plugin(0).unwrap());
                            host.build().unwrap();
                            assert_eq!(host.output_sample_rate(rate).unwrap(), f64::from(rate));
                            assert_eq!(
                                host.render_stats().path,
                                if compiled {
                                    sotf_host::host::HostRenderPath::CompiledLinear
                                } else {
                                    sotf_host::host::HostRenderPath::Graph
                                }
                            );
                            assert_eq!(
                                host.get_plugin(0).unwrap().tail_length(),
                                sotf_host::TailLength::Unknown
                            );
                            assert_eq!(host.total_latency_samples(), 0);
                            if ordered {
                                states.fill(State::default());
                            }
                            let expected =
                                reference_with_states(&input, d, rate, tdf2, &mut states);
                            let actual = host_render(&mut host, &input, 1);
                            let difference = error(&actual, &expected);
                            maximum = maximum.max(difference);
                            assert!(
                                difference < 2e-5,
                                "new-rate reference error {difference}: {rate}/{kind}/{order}/{tdf2}/{ordered}/{compiled}"
                            );
                            cases += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(cases, 384);
    println!(
        "{}",
        serde_json::json!({"case":"host_retained_rate_change","cases":cases,"maximum_reference_error":maximum})
    );
}
