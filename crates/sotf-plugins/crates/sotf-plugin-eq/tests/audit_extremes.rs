//! EQ extremes against prewarped analog prototypes and explicit state.
//!
//! Fixed bounds: 4096-frame unit-impulse prefixes, absolute output error 2e-5.
//! The reference uses tan-prewarping and polynomial bilinear substitution, not
//! the production trigonometric design or its processing helpers. Native matched
//! peaks use independent endpoint/center complex spectral factoring. Standard
//! biquad shelves have fixed slope; SVF shelves preserve the user Q.

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

fn svf_coefficients(d: Design, rate: u32) -> [f64; 5] {
    let k = (PI * d.freq / f64::from(rate)).tan();
    let a = 10.0_f64.powf(d.gain / 40.0);
    // Canonical analog SVF families. Shelves preserve user Q; raw SVF
    // bandpass has center gain Q, unlike the unity-peak standard biquad.
    let (num, den) = match d.kind {
        "lowpass" => ([1.0, 0.0, 0.0], [1.0, 1.0 / d.q, 1.0]),
        "highpass" => ([0.0, 0.0, 1.0], [1.0, 1.0 / d.q, 1.0]),
        "bandpass" => ([0.0, 1.0, 0.0], [1.0, 1.0 / d.q, 1.0]),
        "notch" => ([1.0, 0.0, 1.0], [1.0, 1.0 / d.q, 1.0]),
        "allpass" => ([1.0, -1.0 / d.q, 1.0], [1.0, 1.0 / d.q, 1.0]),
        "peak" | "peak_matched" => ([1.0, a / d.q, 1.0], [1.0, 1.0 / (a * d.q), 1.0]),
        "lowshelf" | "lowshelf_orf" => ([a * a, a * a.sqrt() / d.q, a], [1.0, a.sqrt() / d.q, a]),
        "highshelf" | "highshelf_orf" => ([a, a * a.sqrt() / d.q, a * a], [a, a.sqrt() / d.q, 1.0]),
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
    assert!(
        1.0 + section[3] + section[4] > 0.0
            && 1.0 - section[3] + section[4] > 0.0
            && 1.0 - section[4] > 0.0
    );
    section
}

#[test]
fn svf_extremes_and_optimized_aliases_match_independent_analog_prototypes() {
    let mut impulse = vec![0.0; 4096];
    impulse[0] = 1.0;
    let mut cases = 0;
    let mut failures = 0;
    let mut maximum = 0.0_f64;
    for kind in [
        "peak",
        "lowshelf",
        "highshelf",
        "lowpass",
        "highpass",
        "notch",
        "bandpass",
        "allpass",
        "peak_matched",
        "lowshelf_orf",
        "highshelf_orf",
    ] {
        let mut family_maximum = 0.0_f64;
        let mut family_failures = 0;
        for rate in [44_100, 48_000, 96_000, 192_000] {
            for freq in [20.0, 1000.0, 20000.0] {
                for q in [0.1, if kind == "notch" { 40.0 } else { 20.0 }] {
                    for gain in [-24.0, 24.0] {
                        let d = Design {
                            kind,
                            freq,
                            q,
                            gain,
                            order: 2,
                        };
                        let coefficients = svf_coefficients(d, rate);
                        let mut state = State::default();
                        let expected: Vec<_> = impulse
                            .iter()
                            .map(|&sample| {
                                state.process(f64::from(sample), coefficients, false) as f32
                            })
                            .collect();
                        for ordered in [false, true] {
                            for factor in [1, 2, 4] {
                                let mut p = prepared(d, rate, false, ordered);
                                p.set_parameter(
                                    ParameterId::from("topology"),
                                    ParameterValue::Int(1),
                                )
                                .unwrap();
                                p.set_parameter(
                                    ParameterId::from("oversampling"),
                                    ParameterValue::Int(factor),
                                )
                                .unwrap();
                                assert_eq!(p.latency_samples(), 0);
                                assert_eq!(
                                    p.get_parameter(&ParameterId::from("oversampling")),
                                    Some(ParameterValue::Int(factor))
                                );
                                let actual = render(p.as_mut(), &impulse, rate);
                                let difference = error(&actual, &expected);
                                family_maximum = family_maximum.max(difference);
                                maximum = maximum.max(difference);
                                family_failures += usize::from(difference >= 2e-5);
                                cases += 1;
                            }
                        }
                    }
                }
            }
        }
        failures += family_failures;
        println!(
            "{}",
            serde_json::json!({"family":kind,"maximum_reference_error":family_maximum,"failures":family_failures})
        );
    }
    println!(
        "{}",
        serde_json::json!({"case":"svf_extremes","cases":cases,"failures":failures,"maximum_reference_error":maximum})
    );
    assert_eq!(cases, 3168);
    assert_eq!(
        failures, 0,
        "SVF impulses must match independent analog prototypes at all stated endpoints"
    );
}

#[test]
fn retained_svf_rate_changes_through_host_reset_integrators_and_preserve_selected_factor() {
    use sotf_host::host::{DawHost, HostRenderPath};
    let mut warm = vec![0.0; 512];
    warm[0] = 0.25;
    let duplicated: Vec<_> = warm.iter().flat_map(|&sample| [sample, sample]).collect();
    let mut input = vec![0.0; 1024];
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
            "peak_matched",
            "lowshelf_orf",
            "highshelf_orf",
        ] {
            for freq in [20.0, 20000.0] {
                for q in [0.1, if kind == "notch" { 40.0 } else { 20.0 }] {
                    for gain in [-24.0, 24.0] {
                        let d = Design {
                            kind,
                            freq,
                            q,
                            gain,
                            order: 2,
                        };
                        for ordered in [false, true] {
                            for factor in [1, 2, 4] {
                                for compiled in [false, true] {
                                    let mut p = prepared(d, rate * 2, false, ordered);
                                    p.set_parameter(
                                        ParameterId::from("topology"),
                                        ParameterValue::Int(1),
                                    )
                                    .unwrap();
                                    p.set_parameter(
                                        ParameterId::from("oversampling"),
                                        ParameterValue::Int(factor),
                                    )
                                    .unwrap();
                                    let mut host = DawHost::new(1, rate);
                                    host.set_parallel_enabled(false).unwrap();
                                    host.set_compiled_linear_enabled(compiled).unwrap();
                                    host.add_plugin(Box::new(RateDoubler)).unwrap();
                                    host.add_plugin(p).unwrap();
                                    host.build().unwrap();
                                    let mut state = State::default();
                                    let old_coefficients = svf_coefficients(d, rate * 2);
                                    let expected_warm: Vec<_> = duplicated
                                        .iter()
                                        .map(|&x| {
                                            state.process(f64::from(x), old_coefficients, false)
                                                as f32
                                        })
                                        .collect();
                                    maximum = maximum.max(error(
                                        &host_render(&mut host, &warm, 2),
                                        &expected_warm,
                                    ));
                                    drop(host.remove_plugin(0).unwrap());
                                    host.build().unwrap();
                                    assert_eq!(
                                        host.output_sample_rate(rate).unwrap(),
                                        f64::from(rate)
                                    );
                                    assert_eq!(
                                        host.render_stats().path,
                                        if compiled {
                                            HostRenderPath::CompiledLinear
                                        } else {
                                            HostRenderPath::Graph
                                        }
                                    );
                                    assert_eq!(host.total_latency_samples(), 0);
                                    let eq = host.get_plugin(0).unwrap();
                                    assert_eq!(
                                        eq.get_parameter(&ParameterId::from("oversampling")),
                                        Some(ParameterValue::Int(factor))
                                    );
                                    assert_eq!(eq.tail_length(), sotf_host::TailLength::Unknown);
                                    // SVF banks rebuild their integrators at a rate change in both routes.
                                    let mut state = State::default();
                                    let new_coefficients = svf_coefficients(d, rate);
                                    let expected: Vec<_> = input
                                        .iter()
                                        .map(|&x| {
                                            state.process(f64::from(x), new_coefficients, false)
                                                as f32
                                        })
                                        .collect();
                                    let difference =
                                        error(&host_render(&mut host, &input, 1), &expected);
                                    maximum = maximum.max(difference);
                                    assert!(
                                        difference < 2e-5,
                                        "SVF host-rate error {difference}: {rate}/{kind}/{freq}/{q}/{gain}/{ordered}/{factor}/{compiled}"
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
    assert_eq!(cases, 3168);
    assert!(maximum < 2e-5, "old/new SVF host-rate bound");
    println!(
        "{}",
        serde_json::json!({"case":"svf_retained_host_rates","cases":cases,"maximum_reference_error":maximum})
    );
}

// Independent spectral factor: unity DC/Nyquist fixes b1 and b0+b2;
// the complex center response fixes |b0-b2|. Choose minimum phase and invert
// the positive-gain transfer for cuts. No production coefficient helper is used.
fn optimized_coefficients(d: Design, rate: u32) -> Vec<[f64; 5]> {
    if d.kind != "peak_matched" {
        return coefficients(
            Design {
                kind: if d.kind == "lowshelf_orf" {
                    "lowshelf"
                } else {
                    "highshelf"
                },
                ..d
            },
            rate,
        );
    }
    let w = 2.0 * PI * d.freq / f64::from(rate);
    let count = d.order / 2;
    let gain = 10.0_f64.powf(d.gain.abs() / count as f64 / 20.0);
    (0..count)
        .map(|stage| {
            let prototype =
                1.0 / (2.0 * (PI * (2 * stage + 1) as f64 / (2 * d.order) as f64).cos());
            let q = if count == 1 { d.q } else { d.q * prototype };
            let radius = (-PI * d.freq / (q * f64::from(rate))).exp();
            let a = [1.0, -2.0 * radius * w.cos(), radius * radius];
            let dc = (1.0 - radius).powi(2) + 4.0 * radius * (w / 2.0).sin().powi(2);
            let nyquist = (1.0 - radius).powi(2) + 4.0 * radius * (w / 2.0).cos().powi(2);
            let p = (dc + nyquist) / 2.0;
            let b1 = (dc - nyquist) / 2.0;
            let re = a[0] + a[1] * w.cos() + a[2] * (2.0 * w).cos();
            let im = -a[1] * w.sin() - a[2] * (2.0 * w).sin();
            let real_b = p * w.cos() + b1;
            let difference =
                (gain * gain * (re * re + im * im) - real_b * real_b).sqrt() / w.sin().abs();
            let b = [(p + difference) / 2.0, b1, (p - difference) / 2.0];
            let (num, den) = if d.gain < 0.0 { (a, b) } else { (b, a) };
            assert!(den.iter().chain(num.iter()).all(|x| x.is_finite()));
            assert!(
                den[0] + den[1] + den[2] > 0.0
                    && den[0] - den[1] + den[2] > 0.0
                    && den[0] - den[2] > 0.0
            );
            [
                num[0] / den[0],
                num[1] / den[0],
                num[2] / den[0],
                den[1] / den[0],
                den[2] / den[0],
            ]
        })
        .collect()
}

#[test]
fn native_optimized_extreme_impulses_match_independent_spectral_factors() {
    let mut impulse = vec![0.0; 4096];
    impulse[0] = 1.0;
    let mut cases = 0;
    let mut maximum = 0.0_f64;
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for kind in ["peak_matched", "lowshelf_orf", "highshelf_orf"] {
            for freq in [20.0, 1000.0, 20000.0] {
                for q in [0.1, 20.0] {
                    for gain in [-24.0, 24.0] {
                        for order in [2, 4, 6, 8] {
                            let d = Design {
                                kind,
                                freq,
                                q,
                                gain,
                                order,
                            };
                            let c = optimized_coefficients(d, rate);
                            for tdf2 in [false, true] {
                                let mut states = vec![State::default(); order / 2];
                                let expected: Vec<_> = impulse
                                    .iter()
                                    .map(|&x| {
                                        let mut y = f64::from(x);
                                        for (state, &section) in states.iter_mut().zip(&c) {
                                            y = state.process(y, section, tdf2);
                                        }
                                        y as f32
                                    })
                                    .collect();
                                assert!(expected.iter().all(|x| x.is_finite()));
                                for ordered in [false, true] {
                                    let mut plugin = prepared(d, rate, tdf2, ordered);
                                    let actual = render(plugin.as_mut(), &impulse, rate);
                                    let error = actual
                                        .iter()
                                        .zip(&expected)
                                        .map(|(&a, &b)| f64::from(a - b).abs())
                                        .fold(0.0_f64, f64::max);
                                    maximum = maximum.max(error);
                                    assert!(
                                        error < 2e-5,
                                        "native optimized error {error}: {rate}/{kind}/{freq}/{q}/{gain}/{order}/{tdf2}/{ordered}"
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
    assert_eq!(cases, 2304);
    println!(
        "{}",
        serde_json::json!({"case":"native_optimized_extremes","cases":cases,"maximum_reference_error":maximum})
    );
}

#[test]
fn warped_standard_extremes_match_independent_physical_analog_reference() {
    // For a bilinear prototype s=(1-z^-1)/(k*(1+z^-1)), allpass substitution
    // A=(z^-1-lambda)/(1-lambda*z^-1) multiplies s by (1+lambda)/(1-lambda).
    // Prewarping the prototype corner multiplies k by the same factor.
    // The physical transfer therefore equals the original analog prototype.
    let mut impulse = vec![0.0; 4096];
    impulse[0] = 1.0;
    let mut cases = 0;
    let mut failures = 0;
    let mut maximum = 0.0_f64;
    let mut worst = String::new();
    let mut by_lambda = std::collections::BTreeMap::<String, (usize, f64)>::new();
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
                for q in [0.1, 20.0] {
                    for gain in [-24.0, 24.0] {
                        let d = Design {
                            kind,
                            freq,
                            q,
                            gain,
                            order: 2,
                        };
                        let c = coefficients(d, rate)[0];
                        let mut state = State::default();
                        let expected: Vec<_> = impulse
                            .iter()
                            .map(|&x| state.process(f64::from(x), c, false) as f32)
                            .collect();
                        assert!(expected.iter().all(|x| x.is_finite()));
                        for lambda in [
                            Some(-0.9999),
                            Some(-0.37),
                            Some(0.0),
                            Some(0.37),
                            Some(0.9999),
                            None,
                        ] {
                            for ordered in [false, true] {
                                let mut parameters = params(d, ordered);
                                parameters.filters[0].topology = EqFilterTopology::WarpedBiquad;
                                parameters.filters[0].lambda = lambda;
                                let mut plugin = EqPlugin::from_params(1, rate, parameters)
                                    .unwrap()
                                    .into_boxed_plugin();
                                plugin
                                    .set_parameter(
                                        ParameterId::from("auto_gain_enabled"),
                                        ParameterValue::Bool(false),
                                    )
                                    .unwrap();
                                plugin.initialize(f64::from(rate)).unwrap();
                                let actual = render(plugin.as_mut(), &impulse, rate);
                                let error = actual
                                    .iter()
                                    .zip(&expected)
                                    .map(|(&a, &b)| (f64::from(a) - f64::from(b)).abs())
                                    .fold(0.0_f64, f64::max);
                                if error >= 2e-5 {
                                    failures += 1;
                                    if failures <= 8 {
                                        println!(
                                            "warped failure {rate}/{kind}/{freq}/{q}/{gain}/{lambda:?}/{ordered}: {error}"
                                        );
                                    }
                                }
                                let group = by_lambda.entry(format!("{lambda:?}")).or_default();
                                group.0 += usize::from(error >= 2e-5);
                                group.1 = group.1.max(error);
                                if error > maximum {
                                    maximum = error;
                                    worst = format!(
                                        "{rate}/{kind}/{freq}/{q}/{gain}/{lambda:?}/{ordered}"
                                    );
                                }
                                cases += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    println!(
        "{}",
        serde_json::json!({"case":"warped_standard_extremes","cases":cases,"failures":failures,"maximum_reference_error":maximum,"worst":worst,"by_lambda":by_lambda})
    );
    assert_eq!(cases, 4608);
    assert_eq!(failures, 0);
}

// Each modal basis is a second-order pole transfer preceded by the product
// of earlier allpasses. Direct-form-I recurrence is independent of the runtime's
// TDF-II state code. Signed modal weights are linear, and the plugin adds dry.
struct ModalStage {
    basis: [f64; 5],
    allpass: [f64; 5],
    gain: f64,
    basis_state: State,
    allpass_state: State,
}
struct ModalReference {
    modes: Vec<ModalStage>,
}
impl ModalReference {
    fn new(sections: &[sotf_plugin_eq::KautzSectionConfig], rate: u32) -> Self {
        Self {
            modes: sections
                .iter()
                .map(|s| {
                    let radius = (-PI * s.pole_freq / (s.q * f64::from(rate)))
                        .exp()
                        .min(0.9999);
                    let a1 = -2.0 * radius * (2.0 * PI * s.pole_freq / f64::from(rate)).cos();
                    let a2 = radius * radius;
                    assert!(1.0 + a1 + a2 > 0.0 && 1.0 - a1 + a2 > 0.0 && 1.0 - a2 > 0.0);
                    let scale = (1.0 - a2).powf(1.5);
                    ModalStage {
                        basis: [scale, 0.0, 0.0, a1, a2],
                        allpass: [a2, a1, 1.0, a1, a2],
                        gain: s.gain,
                        basis_state: State::default(),
                        allpass_state: State::default(),
                    }
                })
                .collect(),
        }
    }
    fn process(&mut self, input: f64) -> f64 {
        let mut chain = input;
        let mut output = input;
        for mode in &mut self.modes {
            output += mode.gain * mode.basis_state.process(chain, mode.basis, false);
            chain = mode.allpass_state.process(chain, mode.allpass, false);
        }
        output
    }
}

#[test]
fn kautz_extremes_match_independent_pole_and_allpass_chain_reference() {
    use sotf_plugin_eq::KautzSectionConfig as Section;
    let mut input = vec![0.0_f32; 4096];
    input[0] = 1.0;
    input[257] = -0.3;
    let mut maximum = 0.0_f64;
    let mut cases = 0;
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for freq in [20.0, 1000.0, 20000.0] {
            for q in [0.1, 20.0] {
                for weight in [-24.0, 0.0, 24.0] {
                    let first = Section {
                        pole_freq: freq,
                        q,
                        gain: weight,
                    };
                    for geometry in 0..4 {
                        let sections = match geometry {
                            0 | 1 => vec![first.clone()],
                            2 => vec![
                                first.clone(),
                                Section {
                                    gain: -weight * 0.5,
                                    ..first.clone()
                                },
                            ],
                            3 => vec![
                                first.clone(),
                                Section {
                                    pole_freq: if freq == 20000.0 {
                                        freq - 0.1
                                    } else {
                                        freq + 0.1
                                    },
                                    q: 0.1,
                                    gain: weight * 0.25,
                                },
                            ],
                            _ => unreachable!(),
                        };
                        let mut reference = ModalReference::new(&sections, rate);
                        let expected: Vec<_> = input
                            .iter()
                            .map(|&x| reference.process(f64::from(x)) as f32)
                            .collect();
                        assert!(expected.iter().all(|x| x.is_finite()));
                        for ordered in [false, true] {
                            let d = Design {
                                kind: "peak",
                                freq,
                                q,
                                gain: weight,
                                order: 2,
                            };
                            let mut parameters = params(d, ordered);
                            parameters.filters[0].topology = EqFilterTopology::KautzFilter;
                            if geometry != 0 {
                                parameters.filters[0].kautz_sections = sections.clone();
                            }
                            let mut plugin = EqPlugin::from_params(1, rate, parameters)
                                .unwrap()
                                .into_boxed_plugin();
                            plugin
                                .set_parameter(
                                    ParameterId::from("auto_gain_enabled"),
                                    ParameterValue::Bool(false),
                                )
                                .unwrap();
                            plugin.initialize(f64::from(rate)).unwrap();
                            let actual = render(plugin.as_mut(), &input, rate);
                            let error = actual
                                .iter()
                                .zip(&expected)
                                .map(|(&a, &b)| (f64::from(a) - f64::from(b)).abs())
                                .fold(0.0_f64, f64::max);
                            maximum = maximum.max(error);
                            assert!(
                                error < 2e-5,
                                "Kautz error {error}: {rate}/{freq}/{q}/{weight}/{geometry}/{ordered}"
                            );
                            cases += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(cases, 576);
    println!(
        "{}",
        serde_json::json!({"case":"kautz_extremes","cases":cases,"maximum_reference_error":maximum})
    );
}

#[test]
fn retained_kautz_host_rate_changes_rebuild_modal_history_against_independent_reference() {
    use sotf_host::host::{DawHost, HostRenderPath};
    use sotf_plugin_eq::KautzSectionConfig as Section;
    let mut warm = vec![0.0; 512];
    warm[0] = 0.25;
    let duplicated: Vec<_> = warm.iter().flat_map(|&x| [x, x]).collect();
    let mut input = vec![0.0; 1024];
    input[7] = -0.125;
    let mut cases = 0;
    let mut maximum = 0.0_f64;
    for rate in [44_100, 48_000, 96_000] {
        for freq in [20.0, 1000.0, 20000.0] {
            for q in [0.1, 20.0] {
                for weight in [-24.0, 0.0, 24.0] {
                    let first = Section {
                        pole_freq: freq,
                        q,
                        gain: weight,
                    };
                    for geometry in 0..4 {
                        let sections = match geometry {
                            0 | 1 => vec![first.clone()],
                            2 => vec![
                                first.clone(),
                                Section {
                                    gain: -weight * 0.5,
                                    ..first.clone()
                                },
                            ],
                            3 => vec![
                                first.clone(),
                                Section {
                                    pole_freq: if freq == 20000.0 {
                                        freq - 0.1
                                    } else {
                                        freq + 0.1
                                    },
                                    q: 0.1,
                                    gain: weight * 0.25,
                                },
                            ],
                            _ => unreachable!(),
                        };
                        for ordered in [false, true] {
                            for compiled in [false, true] {
                                let d = Design {
                                    kind: "peak",
                                    freq,
                                    q,
                                    gain: weight,
                                    order: 2,
                                };
                                let mut parameters = params(d, ordered);
                                parameters.filters[0].topology = EqFilterTopology::KautzFilter;
                                if geometry != 0 {
                                    parameters.filters[0].kautz_sections = sections.clone();
                                }
                                let mut plugin = EqPlugin::from_params(1, rate * 2, parameters)
                                    .unwrap()
                                    .into_boxed_plugin();
                                plugin
                                    .set_parameter(
                                        ParameterId::from("auto_gain_enabled"),
                                        ParameterValue::Bool(false),
                                    )
                                    .unwrap();
                                let mut host = DawHost::new(1, rate);
                                host.set_parallel_enabled(false).unwrap();
                                host.set_compiled_linear_enabled(compiled).unwrap();
                                host.add_plugin(Box::new(RateDoubler)).unwrap();
                                host.add_plugin(plugin).unwrap();
                                host.build().unwrap();
                                let mut old_reference = ModalReference::new(&sections, rate * 2);
                                let expected_warm: Vec<_> = duplicated
                                    .iter()
                                    .map(|&x| old_reference.process(f64::from(x)) as f32)
                                    .collect();
                                maximum = maximum
                                    .max(error(&host_render(&mut host, &warm, 2), &expected_warm));
                                drop(host.remove_plugin(0).unwrap());
                                host.build().unwrap();
                                assert_eq!(host.output_sample_rate(rate).unwrap(), f64::from(rate));
                                assert_eq!(
                                    host.render_stats().path,
                                    if compiled {
                                        HostRenderPath::CompiledLinear
                                    } else {
                                        HostRenderPath::Graph
                                    }
                                );
                                assert_eq!(host.total_latency_samples(), 0);
                                assert_eq!(
                                    host.get_plugin(0).unwrap().tail_length(),
                                    sotf_host::TailLength::Unknown
                                );
                                // Both legacy and ordered Kautz banks rebuild at the new rate.
                                let mut new_reference = ModalReference::new(&sections, rate);
                                let expected: Vec<_> = input
                                    .iter()
                                    .map(|&x| new_reference.process(f64::from(x)) as f32)
                                    .collect();
                                let difference =
                                    error(&host_render(&mut host, &input, 1), &expected);
                                maximum = maximum.max(difference);
                                assert!(
                                    difference < 2e-5,
                                    "retained Kautz error {difference}: {rate}/{freq}/{q}/{weight}/{geometry}/{ordered}/{compiled}"
                                );
                                cases += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    assert_eq!(cases, 864);
    assert!(maximum < 2e-5);
    println!(
        "{}",
        serde_json::json!({"case":"kautz_retained_host_rates","cases":cases,"maximum_reference_error":maximum})
    );
}

fn warped_matched_coefficients(d: Design, rate: u32, lambda: f64) -> [f64; 5] {
    // Independent complex pole mapping and endpoint/center spectral factor.
    // Production uses exp_m1/half-angle pole differences and a residual form.
    let w = 2.0 * PI * d.freq / f64::from(rate);
    let mapped = 2.0 * ((w / 2.0).tan() * (1.0 + lambda) / (1.0 - lambda)).atan();
    let radius = (-mapped / (2.0 * d.q)).exp();
    let re = radius * mapped.cos();
    let im = radius * mapped.sin();
    let nr = re + lambda;
    let ni = im;
    let dr = 1.0 + lambda * re;
    let di = lambda * im;
    let power = dr * dr + di * di;
    let pole_re = (nr * dr + ni * di) / power;
    let pole_im = (ni * dr - nr * di) / power;
    let a = [1.0, -2.0 * pole_re, pole_re * pole_re + pole_im * pole_im];
    let dc = a[0] + a[1] + a[2];
    let nyquist = a[0] - a[1] + a[2];
    let p = (dc + nyquist) / 2.0;
    let b1 = (dc - nyquist) / 2.0;
    let gain = 10.0_f64.powf(d.gain.abs() / 20.0);
    let ar = a[0] + a[1] * w.cos() + a[2] * (2.0 * w).cos();
    let ai = -a[1] * w.sin() - a[2] * (2.0 * w).sin();
    let br = p * w.cos() + b1;
    let split = (gain * gain * (ar * ar + ai * ai) - br * br).sqrt() / w.sin().abs();
    let b = [(p + split) / 2.0, b1, (p - split) / 2.0];
    let (num, den) = if d.gain < 0.0 { (a, b) } else { (b, a) };
    assert!(num.iter().chain(den.iter()).all(|x| x.is_finite()));
    [
        num[0] / den[0],
        num[1] / den[0],
        num[2] / den[0],
        den[1] / den[0],
        den[2] / den[0],
    ]
}

#[test]
fn warped_optimized_extremes_match_independent_physical_references() {
    let mut input = vec![0.0; 4096];
    input[0] = 1.0;
    let mut maximum = 0.0_f64;
    let mut cases = 0;
    for rate in [44100, 48000, 96000, 192000] {
        for kind in ["peak_matched", "lowshelf_orf", "highshelf_orf"] {
            for freq in [20.0, 1000.0, 20000.0] {
                for q in [0.1, 20.0] {
                    for gain in [-24.0, 24.0] {
                        let d = Design {
                            kind,
                            freq,
                            q,
                            gain,
                            order: 2,
                        };
                        for lambda in [
                            Some(-0.9999),
                            Some(-0.37),
                            Some(0.0),
                            Some(0.37),
                            Some(0.9999),
                            None,
                        ] {
                            let l = lambda.unwrap_or_else(|| {
                                1.0674
                                    * ((2.0 / PI) * (0.06583 * f64::from(rate) / 1000.0).atan())
                                        .sqrt()
                                    - 0.1916
                            });
                            let c = if kind == "peak_matched" {
                                warped_matched_coefficients(d, rate, l)
                            } else {
                                optimized_coefficients(d, rate)[0]
                            };
                            let mut state = State::default();
                            let expected: Vec<_> = input
                                .iter()
                                .map(|&x| state.process(f64::from(x), c, false) as f32)
                                .collect();
                            assert!(expected.iter().all(|x| x.is_finite()));
                            for ordered in [false, true] {
                                let mut parameters = params(d, ordered);
                                parameters.filters[0].topology = EqFilterTopology::WarpedBiquad;
                                parameters.filters[0].lambda = lambda;
                                let mut plugin = EqPlugin::from_params(1, rate, parameters)
                                    .unwrap()
                                    .into_boxed_plugin();
                                plugin
                                    .set_parameter(
                                        ParameterId::from("auto_gain_enabled"),
                                        ParameterValue::Bool(false),
                                    )
                                    .unwrap();
                                plugin.initialize(f64::from(rate)).unwrap();
                                let actual = render(plugin.as_mut(), &input, rate);
                                let difference = error(&actual, &expected);
                                maximum = maximum.max(difference);
                                assert!(
                                    difference < 2e-5,
                                    "warped optimized {difference}: {rate}/{kind}/{freq}/{q}/{gain}/{lambda:?}/{ordered}"
                                );
                                cases += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    assert_eq!(cases, 1728);
    println!(
        "{}",
        serde_json::json!({"case":"warped_optimized_extremes","cases":cases,"maximum_reference_error":maximum})
    );
}

struct ImplicitWarpedReference {
    c: [f64; 5],
    lambda: f64,
    history: [f64; 4],
}
impl ImplicitWarpedReference {
    fn retune(&mut self, d: Design, rate: u32, lambda: Option<f64>) {
        self.lambda = lambda.unwrap_or_else(|| {
            1.0674 * ((2.0 / PI) * (0.06583 * f64::from(rate) / 1000.0).atan()).sqrt() - 0.1916
        });
        let k = (PI * d.freq / f64::from(rate)).tan() * (1.0 + self.lambda) / (1.0 - self.lambda);
        let mapped = Design {
            freq: k.atan() * f64::from(rate) / PI,
            ..d
        };
        self.c = if matches!(
            mapped.kind,
            "peak_matched" | "lowshelf_orf" | "highshelf_orf"
        ) {
            optimized_coefficients(mapped, rate)[0]
        } else {
            coefficients(mapped, rate)[0]
        };
    }
    fn process(&mut self, x: f64) -> f64 {
        fn ap(x: f64, s: &mut f64, l: f64) -> f64 {
            let z = x + l * *s;
            let y = *s - l * z;
            *s = z;
            y
        }
        let l = self.lambda;
        let [b0, b1, b2, a1, a2] = self.c;
        let xd1 = ap(x, &mut self.history[0], l);
        let xd2 = ap(xd1, &mut self.history[1], l);
        let y = (b0 * x + b1 * xd1 + b2 * xd2
            - (1.0 - l * l) * ((a1 - a2 * l) * self.history[2] + a2 * self.history[3]))
            / (1.0 - a1 * l + a2 * l * l);
        let yd1 = ap(y, &mut self.history[2], l);
        ap(yd1, &mut self.history[3], l);
        y
    }
}

#[test]
fn retained_warped_host_rates_preserve_legacy_history_and_reset_ordered_history() {
    use sotf_host::host::{DawHost, HostRenderPath};
    let mut warm = vec![0.0; 512];
    warm[0] = 0.25;
    let duplicated: Vec<_> = warm.iter().flat_map(|&x| [x, x]).collect();
    let mut input = vec![0.0; 1024];
    input[7] = -0.125;
    let mut maximum = 0.0_f64;
    let mut cases = 0;
    for rate in [44100, 48000, 96000] {
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
            for freq in [20.0, 20000.0] {
                for q in [0.1, 20.0] {
                    for gain in [-24.0, 24.0] {
                        let d = Design {
                            kind,
                            freq,
                            q,
                            gain,
                            order: 2,
                        };
                        for lambda in [None, Some(-0.37), Some(0.37)] {
                            for ordered in [false, true] {
                                for compiled in [false, true] {
                                    let mut parameters = params(d, ordered);
                                    parameters.filters[0].topology = EqFilterTopology::WarpedBiquad;
                                    parameters.filters[0].lambda = lambda;
                                    let mut plugin = EqPlugin::from_params(1, rate * 2, parameters)
                                        .unwrap()
                                        .into_boxed_plugin();
                                    plugin
                                        .set_parameter(
                                            ParameterId::from("auto_gain_enabled"),
                                            ParameterValue::Bool(false),
                                        )
                                        .unwrap();
                                    let mut host = DawHost::new(1, rate);
                                    host.set_parallel_enabled(false).unwrap();
                                    host.set_compiled_linear_enabled(compiled).unwrap();
                                    host.add_plugin(Box::new(RateDoubler)).unwrap();
                                    host.add_plugin(plugin).unwrap();
                                    host.build().unwrap();
                                    let mut reference = ImplicitWarpedReference {
                                        c: [0.0; 5],
                                        lambda: 0.0,
                                        history: [0.0; 4],
                                    };
                                    reference.retune(d, rate * 2, lambda);
                                    let expected_warm: Vec<_> = duplicated
                                        .iter()
                                        .map(|&x| reference.process(f64::from(x)) as f32)
                                        .collect();
                                    maximum = maximum.max(error(
                                        &host_render(&mut host, &warm, 2),
                                        &expected_warm,
                                    ));
                                    drop(host.remove_plugin(0).unwrap());
                                    host.build().unwrap();
                                    assert_eq!(
                                        host.output_sample_rate(rate).unwrap(),
                                        f64::from(rate)
                                    );
                                    assert_eq!(
                                        host.render_stats().path,
                                        if compiled {
                                            HostRenderPath::CompiledLinear
                                        } else {
                                            HostRenderPath::Graph
                                        }
                                    );
                                    assert_eq!(host.total_latency_samples(), 0);
                                    assert_eq!(
                                        host.get_plugin(0).unwrap().tail_length(),
                                        sotf_host::TailLength::Unknown
                                    );
                                    reference.retune(d, rate, lambda);
                                    if ordered {
                                        reference.history = [0.0; 4];
                                    }
                                    let expected: Vec<_> = input
                                        .iter()
                                        .map(|&x| reference.process(f64::from(x)) as f32)
                                        .collect();
                                    let difference =
                                        error(&host_render(&mut host, &input, 1), &expected);
                                    maximum = maximum.max(difference);
                                    assert!(
                                        difference < 2e-5,
                                        "retained Warped {difference}: {rate}/{kind}/{freq}/{q}/{gain}/{lambda:?}/{ordered}/{compiled}"
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
    assert_eq!(cases, 2304);
    assert!(maximum < 2e-5);
    println!(
        "{}",
        serde_json::json!({"case":"warped_retained_host_rates","cases":cases,"maximum_reference_error":maximum})
    );
}

#[path = "audit_extremes/mixed.rs"]
mod mixed;
