//! Actual FIR-oversampled host rate epochs with independent EQ core references.
//!
//! Only the FIR transport primitive is shared. Coefficient design, histories,
//! routing, epoch resets and base/inner rate selection are modeled explicitly.

// Rust guideline compliant 2026-02-21
use super::*;
use sotf_host::host::{DawHost, HostRenderPath};
use sotf_host::oversampling::Oversampler;

struct RateDoublerN {
    channels: usize,
}
impl Plugin for RateDoublerN {
    fn info(&self) -> sotf_host::PluginInfo {
        sotf_host::PluginInfo::new("rate fixture", "1", "test")
    }
    fn input_channels(&self) -> usize {
        self.channels
    }
    fn output_channels(&self) -> usize {
        self.channels
    }
    fn parameters(&self) -> Vec<sotf_host::Parameter> {
        vec![]
    }
    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
        Err("no controls".into())
    }
    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }
    fn output_frames_for_input(&self, n: usize) -> usize {
        n * 2
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
        for (n, frame) in input
            .chunks_exact(self.channels)
            .take(context.num_frames)
            .enumerate()
        {
            output[2 * n * self.channels..(2 * n + 1) * self.channels].copy_from_slice(frame);
            output[(2 * n + 1) * self.channels..(2 * n + 2) * self.channels].copy_from_slice(frame);
        }
        Ok(context.num_frames * 2)
    }
}
fn duplicate(input: &[f32], channels: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(input.len() * 2);
    for frame in input.chunks_exact(channels) {
        out.extend_from_slice(frame);
        out.extend_from_slice(frame);
    }
    out
}

struct RateReference {
    configs: Vec<Vec<BiquadFilterConfig>>,
    models: Vec<MixedReference>,
    per_channel: bool,
    ordered: bool,
    mode: Mode,
    channels: usize,
    factor: u32,
    transport: Oversampler,
}
impl RateReference {
    fn new(params: &EqPluginParams, mode: Mode, channels: usize, rate: u32, factor: u32) -> Self {
        let per_channel = params.channel_filters.is_some();
        let configs = params
            .channel_filters
            .clone()
            .unwrap_or_else(|| vec![params.filters.clone()]);
        let ordered = !per_channel && configs[0].iter().any(|c| c.placement.is_some());
        let initial_rate = if ordered { rate * factor } else { rate };
        let models = configs
            .iter()
            .map(|c| {
                MixedReference::new(
                    c,
                    mode,
                    if per_channel { 1 } else { channels },
                    initial_rate,
                )
            })
            .collect();
        let mut transport = Oversampler::new(factor, channels).unwrap();
        transport.reserve_for_max_frames(4096).unwrap();
        let mut out = Self {
            configs,
            models,
            per_channel,
            ordered,
            mode,
            channels,
            factor,
            transport,
        };
        if !ordered {
            out.retune_legacy(rate, true);
        }
        out
    }
    fn retune_legacy(&mut self, rate: u32, initial: bool) {
        for (configs, model) in self.configs.iter().zip(&mut self.models) {
            for (cfg, band) in configs.iter().zip(&mut model.bands) {
                match &mut band.core {
                    Core::Sections(s) if band.native => {
                        s.target = native_coefficients(design(cfg), rate * self.factor, false);
                        s.old = s.target.clone();
                        s.length = 0;
                        s.elapsed = 0;
                    }
                    Core::Sections(_) if cfg.topology == EqFilterTopology::WarpedBiquad => {
                        assert!(initial);
                        let mut refs = Vec::new();
                        for _ in 0..if self.per_channel { 1 } else { self.channels } {
                            let mut r = ImplicitWarpedReference {
                                c: [0.0; 5],
                                lambda: 0.0,
                                history: [0.0; 4],
                            };
                            r.retune(design(cfg), rate, cfg.lambda);
                            refs.push(r);
                        }
                        band.core = Core::WarpedHistory(refs);
                    }
                    Core::WarpedHistory(refs) => {
                        for r in refs {
                            r.retune(design(cfg), rate, cfg.lambda);
                        }
                    }
                    Core::Modal(refs) => {
                        *refs = (0..refs.len())
                            .map(|_| ModalReference::new(&cfg.kautz_sections, rate))
                            .collect();
                    }
                    _ => unreachable!(),
                }
            }
        }
    }
    fn retune(&mut self, rate: u32) {
        if self.ordered {
            for (configs, model) in self.configs.iter().zip(&mut self.models) {
                *model = MixedReference::new(configs, self.mode, self.channels, rate * self.factor);
            }
        } else {
            self.retune_legacy(rate, false);
        }
        self.transport = Oversampler::new(self.factor, self.channels).unwrap();
        self.transport.reserve_for_max_frames(4096).unwrap();
    }
    fn inner(model: &mut MixedReference, frame: &mut [f32], ordered: bool) {
        if ordered {
            model.frame(frame);
        } else {
            for (ch, sample) in frame.iter_mut().enumerate() {
                let mut x = f64::from(*sample);
                for band in model.bands.iter_mut().filter(|b| b.native) {
                    x = band.process(ch, x);
                }
                *sample = x as f32;
            }
        }
    }
    fn after(model: &mut MixedReference, frame: &mut [f32]) {
        for (ch, sample) in frame.iter_mut().enumerate() {
            let mut x = f64::from(*sample);
            for band in model.bands.iter_mut().filter(|b| !b.native) {
                x = band.process(ch, x);
            }
            *sample = x as f32;
        }
    }
    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        let mut out = input.to_vec();
        let channels = self.channels;
        let ordered = self.ordered;
        let per_channel = self.per_channel;
        let models = &mut self.models;
        self.transport
            .process(&mut out, input.len() / channels, |planar, frames| {
                let mut frame = [0.0_f32; 5];
                // Frame-major traversal is required for L/R/M/S mixing.
                let mut n = 0;
                while n < frames {
                    for ch in 0..channels {
                        frame[ch] = planar[ch][n];
                    }
                    if per_channel {
                        for (ch, model) in models.iter_mut().enumerate() {
                            Self::inner(model, &mut frame[ch..ch + 1], false);
                        }
                    } else {
                        Self::inner(&mut models[0], &mut frame[..channels], ordered);
                    }
                    for ch in 0..channels {
                        planar[ch][n] = frame[ch];
                    }
                    n += 1;
                }
            })
            .unwrap();
        if !ordered {
            for frame in out.chunks_exact_mut(channels) {
                if per_channel {
                    for (ch, model) in models.iter_mut().enumerate() {
                        Self::after(model, &mut frame[ch..ch + 1]);
                    }
                } else {
                    Self::after(&mut models[0], frame);
                }
            }
        }
        assert!(out.iter().all(|x| x.is_finite()));
        out
    }
}

fn make(
    params: EqPluginParams,
    mode: Mode,
    channels: usize,
    rate: u32,
    factor: u32,
) -> Box<dyn Plugin> {
    let mut p = EqPlugin::from_params(channels, rate, params)
        .unwrap()
        .into_boxed_plugin();
    p.set_parameter(
        ParameterId::from("auto_gain_enabled"),
        ParameterValue::Bool(false),
    )
    .unwrap();
    p.set_parameter(ParameterId::from("tdf2"), ParameterValue::Bool(mode.tdf2))
        .unwrap();
    p.set_parameter(
        ParameterId::from("oversampling"),
        ParameterValue::Int(factor as i32),
    )
    .unwrap();
    p
}
fn render_pair(
    host: &mut DawHost,
    reference: &mut RateReference,
    input: &[f32],
    channels: usize,
    domain_factor: usize,
) -> f64 {
    let mut maximum = 0.0_f64;
    for source in input.chunks(193 * channels) {
        let reference_input = if domain_factor == 2 {
            duplicate(source, channels)
        } else {
            source.to_vec()
        };
        let expected = reference.process(&reference_input);
        let mut actual = vec![f32::NAN; source.len() * domain_factor];
        let heap = sotf_host::test_utils::measure_heap_activity(|| {
            assert_eq!(
                host.process(source, &mut actual).unwrap(),
                source.len() / channels * domain_factor
            )
        });
        assert_eq!(heap, (0, 0));
        assert!(actual.iter().all(|x| x.is_finite()));
        maximum = maximum.max(error(&actual, &expected));
    }
    maximum
}
fn run(
    params: EqPluginParams,
    mode: Mode,
    channels: usize,
    rate: u32,
    factor: u32,
    compiled: bool,
) -> f64 {
    let mut host = DawHost::new(channels, rate);
    host.set_parallel_enabled(false).unwrap();
    host.set_compiled_linear_enabled(compiled).unwrap();
    host.add_plugin(Box::new(RateDoublerN { channels }))
        .unwrap();
    host.add_plugin(make(params.clone(), mode, channels, rate * 2, factor))
        .unwrap();
    host.build().unwrap();
    assert_eq!(host.output_sample_rate(rate).unwrap(), f64::from(rate * 2));
    let mut reference = RateReference::new(&params, mode, channels, rate * 2, factor);
    let latency = reference.transport.latency_samples();
    assert!(latency > 0);
    assert_eq!(host.get_plugin(1).unwrap().latency_samples(), latency);
    let full = input(channels, rate);
    let warm = &full[..1025 * channels];
    let mut maximum = render_pair(&mut host, &mut reference, warm, channels, 2);
    let chunk = reference.transport.input_chunk_frames();
    assert_eq!(2050 % chunk, 2);
    assert_eq!(reference.transport.input_frames_until_process(), chunk - 2);
    drop(host.remove_plugin(0).unwrap());
    host.build().unwrap();
    reference.retune(rate);
    assert_eq!(reference.transport.input_frames_until_process(), chunk);
    assert_eq!(host.output_sample_rate(rate).unwrap(), f64::from(rate));
    // Latency is an explicit host compilation barrier. Exercise both enable
    // settings, asserting the actual supported graph fallback in either case.
    assert_eq!(host.render_stats().path, HostRenderPath::Graph);
    assert_eq!(host.total_latency_samples(), latency);
    let p = host.get_plugin(0).unwrap();
    assert_eq!(p.latency_samples(), latency);
    assert_eq!(
        p.get_parameter(&ParameterId::from("oversampling")),
        Some(ParameterValue::Int(factor as i32))
    );
    assert_eq!(p.tail_length(), sotf_host::TailLength::Unknown);
    maximum = maximum.max(render_pair(&mut host, &mut reference, &full, channels, 1));
    assert_eq!(host.tail_length(), sotf_host::TailLength::Unknown);
    assert_eq!(host.tail_support(), None);
    assert_eq!(reference.transport.input_frames_until_process(), chunk - 3);
    let mut eos = vec![123.0; host.drain_output_frames_max().max(1) * channels];
    let heap = sotf_host::test_utils::measure_heap_activity(|| {
        assert_eq!(
            host.drain(&mut eos).unwrap(),
            sotf_host::plugin::PluginDrainResult::COMPLETE
        )
    });
    assert_eq!(heap, (0, 0));
    assert_eq!(
        host.get_plugin(0).unwrap().tail_length(),
        sotf_host::TailLength::Unknown
    );
    assert_eq!(host.tail_support(), None);
    // Legacy recursive COMPLETE does not certify finite support or discard
    // the retained transport/recursive state. New accepted input continues it.
    maximum = maximum.max(render_pair(
        &mut host,
        &mut reference,
        &vec![0.0; 517 * channels],
        channels,
        1,
    ));

    assert!(
        maximum < 2e-5,
        "actual oversampled host error {maximum}: {rate}/{channels}/{factor}/{compiled}/{}",
        mode.kind
    );
    maximum
}

#[test]
fn oversampled_native_families_retain_host_rate_state_against_independent_core_models() {
    let families = [
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
    ];
    let mut maximum = 0.0_f64;
    let mut cases = 0;
    for (ri, rate) in [44100, 48000, 96000].into_iter().enumerate() {
        for (ki, kind) in families.into_iter().enumerate() {
            for order in [2, 8] {
                for tdf2 in [false, true] {
                    for ordered in [false, true] {
                        for factor in [2, 4] {
                            for compiled in [false, true] {
                                let d = Design {
                                    kind,
                                    freq: if (ri + ki) % 2 == 0 { 20.0 } else { 20000.0 },
                                    q: if tdf2 { 20.0 } else { 0.1 },
                                    gain: if order == 2 { -24.0 } else { 24.0 },
                                    order,
                                };
                                let params = EqPluginParams {
                                    filters: vec![config(d, ordered)],
                                    channel_filters: None,
                                    stereo_pairs: None,
                                    auto_gain: AutoGainParams::default(),
                                };
                                let mode = Mode {
                                    kind,
                                    order,
                                    tdf2,
                                    svf: false,
                                };
                                maximum = maximum.max(run(params, mode, 1, rate, factor, compiled));
                                cases += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    assert_eq!(cases, 1056);
    println!(
        "{}",
        serde_json::json!({"case":"actual_oversampled_native_host_rates","cases":cases,"maximum_reference_error":maximum})
    );
}
#[test]
fn oversampled_mixed_placements_use_inner_versus_base_rate_histories_through_host() {
    let mut maximum = 0.0_f64;
    let mut cases = 0;
    for (ri, rate) in [44100, 48000, 96000].into_iter().enumerate() {
        for channels in [2, 5] {
            for (pi, permutation) in PERMUTATIONS.into_iter().enumerate() {
                for profile in 0..4 {
                    for factor in [2, 4] {
                        for compiled in [false, true] {
                            let mode = MODES[pi % 4];
                            let lambda = [Some(-0.37), Some(0.37), None][(ri + pi + profile) % 3];
                            let defs = definitions(mode, lambda, profile, pi % 2 == 0);
                            let configs =
                                permutation.into_iter().map(|i| defs[i].clone()).collect();
                            let params = EqPluginParams {
                                filters: configs,
                                channel_filters: None,
                                stereo_pairs: (channels == 5).then(|| pairs(channels)),
                                auto_gain: AutoGainParams::default(),
                            };
                            maximum =
                                maximum.max(run(params, mode, channels, rate, factor, compiled));
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
        serde_json::json!({"case":"actual_oversampled_mixed_host_rates","cases":cases,"maximum_reference_error":maximum})
    );
}
#[test]
fn oversampled_per_channel_mixed_banks_retain_their_host_rate_state() {
    let mut maximum = 0.0_f64;
    let mut cases = 0;
    for rate in [44100, 48000, 96000] {
        for channels in [2, 5] {
            for (pi, permutation) in PERMUTATIONS.into_iter().enumerate() {
                for factor in [2, 4] {
                    for compiled in [false, true] {
                        let mode = MODES[pi % 4];
                        let configs = channel_configs(mode, permutation, channels);
                        let params = EqPluginParams {
                            filters: vec![],
                            channel_filters: Some(configs),
                            stereo_pairs: None,
                            auto_gain: AutoGainParams::default(),
                        };
                        maximum = maximum.max(run(params, mode, channels, rate, factor, compiled));
                        cases += 1;
                    }
                }
            }
        }
    }
    assert_eq!(cases, 144);
    println!(
        "{}",
        serde_json::json!({"case":"actual_oversampled_per_channel_host_rates","cases":cases,"maximum_reference_error":maximum})
    );
}

#[test]
fn finite_oversampled_host_epochs_flush_the_final_marker_with_declared_support() {
    let mut cases = 0;
    let mut maximum = 0.0_f64;
    let mut minimum_marker = f64::INFINITY;
    for rate in [44100, 48000, 96000] {
        for channels in [1, 2, 5] {
            for factor in [2, 4] {
                for compiled in [false, true] {
                    for frames in [1, 257, 1025] {
                        for capacity_multiplier in [1, 3] {
                            let mode = MODES[0];
                            let params = EqPluginParams {
                                filters: vec![],
                                channel_filters: None,
                                stereo_pairs: None,
                                auto_gain: AutoGainParams::default(),
                            };
                            let mut host = DawHost::new(channels, rate);
                            host.set_parallel_enabled(false).unwrap();
                            host.set_compiled_linear_enabled(compiled).unwrap();
                            host.add_plugin(Box::new(RateDoublerN { channels }))
                                .unwrap();
                            host.add_plugin(make(params.clone(), mode, channels, rate * 2, factor))
                                .unwrap();
                            host.build().unwrap();
                            let mut reference =
                                RateReference::new(&params, mode, channels, rate * 2, factor);
                            let warm = input(channels, rate);
                            maximum = maximum.max(render_pair(
                                &mut host,
                                &mut reference,
                                &warm[..1025 * channels],
                                channels,
                                2,
                            ));
                            drop(host.remove_plugin(0).unwrap());
                            host.build().unwrap();
                            reference.retune(rate);
                            assert_eq!(host.render_stats().path, HostRenderPath::Graph);
                            let support = reference.transport.passthrough_tail_frames();
                            assert_eq!(support, 4 * reference.transport.input_chunk_frames());
                            assert_eq!(
                                host.get_plugin(0).unwrap().tail_length(),
                                sotf_host::TailLength::Finite(support as u64)
                            );
                            assert_eq!(
                                host.total_latency_samples(),
                                reference.transport.latency_samples()
                            );
                            let mut final_input = vec![0.0; frames * channels];
                            for ch in 0..channels {
                                final_input[(frames - 1) * channels + ch] = 0.5 / (ch + 1) as f32;
                            }
                            maximum = maximum.max(render_pair(
                                &mut host,
                                &mut reference,
                                &final_input,
                                channels,
                                1,
                            ));
                            assert_eq!(
                                host.tail_length(),
                                sotf_host::TailLength::Finite(support as u64)
                            );
                            let all_state_support = host
                                .tail_support()
                                .expect("finite transport support must compose");
                            let chunk = reference.transport.input_chunk_frames();
                            let mut expected = Vec::new();
                            for _ in 0..support / chunk {
                                expected.extend(reference.process(&vec![0.0; chunk * channels]));
                            }
                            let mut drained = Vec::new();
                            let capacity =
                                host.drain_output_frames_max().max(1) * capacity_multiplier;
                            let mut completed = false;
                            for _ in 0..64 {
                                let mut out = vec![f32::NAN; capacity * channels];
                                let mut result = sotf_host::plugin::PluginDrainResult::COMPLETE;
                                let heap = sotf_host::test_utils::measure_heap_activity(|| {
                                    result = host.drain(&mut out).unwrap()
                                });
                                assert_eq!(heap, (0, 0));
                                assert!(result.frames <= host.drain_output_frames_max());
                                assert!(
                                    out[..result.frames * channels]
                                        .iter()
                                        .all(|x| x.is_finite())
                                );
                                drained.extend_from_slice(&out[..result.frames * channels]);
                                if result.complete {
                                    completed = true;
                                    break;
                                }
                            }
                            assert!(completed, "finite host EOS did not complete");
                            assert_eq!(drained.len(), support * channels);
                            assert!(
                                (drained.len() / channels) as u64 <= all_state_support,
                                "composed all-state support must bound actual emitted frames"
                            );
                            maximum = maximum.max(error(&drained, &expected));
                            assert_eq!(
                                drained, expected,
                                "host finite tail must equal the explicit padded FIR continuation"
                            );
                            assert_eq!(host.tail_length(), sotf_host::TailLength::Finite(0));
                            let heap = sotf_host::test_utils::measure_heap_activity(|| {
                                assert_eq!(
                                    host.drain(&mut []).unwrap(),
                                    sotf_host::plugin::PluginDrainResult::COMPLETE
                                )
                            });
                            assert_eq!(heap, (0, 0));
                            let remainder = reference.process(&vec![0.0; 4096 * channels]);
                            assert!(remainder.iter().all(|x| x.abs() < 1e-7));
                            host.reset();
                            reference.retune(rate);
                            maximum = maximum.max(render_pair(
                                &mut host,
                                &mut reference,
                                &final_input,
                                channels,
                                1,
                            ));
                            let marker = expected
                                .iter()
                                .map(|&x| f64::from(x).abs())
                                .fold(0.0_f64, f64::max);
                            minimum_marker = minimum_marker.min(marker);
                            assert!(marker > 0.1);
                            cases += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(cases, 216);
    println!(
        "{}",
        serde_json::json!({"case":"finite_oversampled_host_eof","cases":cases,"maximum_reference_error":maximum,"minimum_reference_final_marker_peak":minimum_marker})
    );
}
