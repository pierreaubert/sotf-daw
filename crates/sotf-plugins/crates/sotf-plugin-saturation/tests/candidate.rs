//! Capture-bound native nonlinear selection and continuously running ADAA fallback.
#![cfg(feature = "qa")]
// Rust guideline compliant 2026-02-21
use sha2::{Digest, Sha256};
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{
    CountingAlloc, ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext,
};
use sotf_plugin_saturation::candidate::capture_controls_sha256;
use sotf_plugin_saturation::{SaturationPlugin, SaturationPluginParams};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

fn params(drive: f32) -> SaturationPluginParams {
    SaturationPluginParams {
        drive,
        tone: 1.0,
        oversampling: "Off".into(),
        mix: 1.0,
        output_gain_db: 0.0,
        dc_blocker_enabled: false,
        use_adaa: true,
        ..Default::default()
    }
}

fn make(drive: f32) -> SaturationPlugin {
    let mut p = SaturationPlugin::try_from_params(1, params(drive)).unwrap();
    p.initialize(48000.0).unwrap();
    p
}

fn fixture(plugin: &SaturationPlugin, drive: f64) -> (Vec<u8>, String, Vec<u8>) {
    fixture_at_rate(plugin, drive, 48000)
}

fn fixture_at_rate(plugin: &SaturationPlugin, drive: f64, rate: u32) -> (Vec<u8>, String, Vec<u8>) {
    // Untrained analytic fixture: coefficients are set explicitly, not fitted to
    // hardware. Remove training records inherited from the generic schema sample.
    let mut raw: serde_json::Value = serde_json::from_str(include_str!(
        "../../../models/analog-hardware/candidate.synthetic.json"
    ))
    .unwrap();
    raw.as_object_mut().unwrap().remove("training_captures");
    raw.as_object_mut()
        .unwrap()
        .remove("capture_manifest_canonical_sha256");
    raw["context"]["sample_rate"] = rate.into();
    raw["context"]["hardware"]["model"] = "untrained-saturation-tanh-fixture".into();
    raw["context"]["hardware"]["controls"] = serde_json::json!({"sotf_stage":"saturation_wet_pre_dc","sotf_controls_sha256":capture_controls_sha256(plugin)});
    raw["architecture"]["hidden"] = 2.into();
    raw["input_scale"] = 0.5.into();
    raw["input_peak_limit"] = 0.9.into();
    raw["linear"] = serde_json::json!([0.0, 0.0, 0.0]);
    raw["hidden_weights"] = serde_json::json!([[drive * 0.5, 0.0], [0.0, 0.0], [0.0, 0.0]]);
    raw["hidden_bias"] = serde_json::json!([0.0, 0.0]);
    raw["output_weights"] = serde_json::json!([1.0 / drive.tanh(), 0.0]);
    raw["output_bias"] = 0.0.into();
    raw["global_output_bound"] = (1.0 / drive.tanh()).into();
    let context = serde_json::to_vec(&raw["context"]).unwrap();
    let bytes = serde_json::to_vec(&raw).unwrap();
    let sha = Sha256::digest(&bytes)
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect();
    (bytes, sha, context)
}

fn install(plugin: &mut SaturationPlugin, drive: f64) {
    let (bytes, sha, context) = fixture(plugin, drive);
    plugin
        .install_qa_candidate(Some(&bytes), &sha, &context)
        .unwrap();
}

fn source() -> Vec<f32> {
    (0..5101)
        .map(|i| ((i * 17 % 103) as f32 - 51.0) / 128.0)
        .collect()
}

fn render(plugin: &mut SaturationPlugin, input: &[f32], block: usize) -> Vec<f32> {
    let mut output = input.to_vec();
    for chunk in output.chunks_mut(block) {
        let context = ProcessContext::new(48000, chunk.len());
        assert_no_allocs_or_deallocs("candidate process", || {
            plugin.process_in_place(chunk, &context).unwrap();
        });
    }
    output
}

#[test]
fn disabled_missing_and_clock_mismatch_keep_native_output() {
    let input = source();
    let expected = render(&mut make(3.0), &input, 193);
    let mut missing = make(3.0);
    missing.install_qa_candidate(None, "", b"").unwrap();
    missing.set_qa_candidate_enabled(true);
    assert_eq!(render(&mut missing, &input, 17), expected);
    assert_eq!(
        missing.qa_candidate_counters().unwrap().unavailable,
        input.len() as u64
    );
    let mut disabled = make(3.0);
    install(&mut disabled, 3.0);
    assert_eq!(render(&mut disabled, &input, 17), expected);
    let mut wrong_rate = make(3.0);
    install(&mut wrong_rate, 3.0);
    wrong_rate.set_qa_candidate_enabled(true);
    wrong_rate.initialize(96000.0).unwrap();
    let mut baseline = make(3.0);
    baseline.initialize(96000.0).unwrap();
    let mut a = input.clone();
    let mut b = input;
    wrong_rate
        .process_in_place(&mut a, &ProcessContext::new(96000, b.len()))
        .unwrap();
    let frames = b.len();
    baseline
        .process_in_place(&mut b, &ProcessContext::new(96000, frames))
        .unwrap();
    assert_eq!(a, b);
    assert_eq!(
        wrong_rate.qa_candidate_counters().unwrap().out_of_domain,
        a.len() as u64
    );
}

#[test]
fn analytic_fixture_sweeps_match_reference_reset_and_callback_boundaries() {
    let input = source();
    for drive in [1.0_f32, 4.0, 12.0, 20.0] {
        let mut a = make(drive);
        install(&mut a, f64::from(drive));
        a.set_qa_candidate_enabled(true);
        let actual = render(&mut a, &input, 193);
        let expected: Vec<_> = input
            .iter()
            .map(|x| (f64::from(*x) * f64::from(drive)).tanh() / f64::from(drive).tanh())
            .collect();
        let error = actual
            .iter()
            .zip(expected)
            .map(|(a, b)| (f64::from(*a) - b).abs())
            .fold(0.0, f64::max);
        assert!(error < 2e-7, "drive {drive} error {error}");
        assert_eq!(
            a.qa_candidate_counters().unwrap().neural,
            input.len() as u64
        );
        assert_eq!(a.latency_samples(), 0);
        assert_no_allocs_or_deallocs("candidate reset", || a.reset());
        assert_eq!(render(&mut a, &input, 17), actual);
        a.initialize(48000.0).unwrap();
        assert_eq!(render(&mut a, &input, 137), actual);
    }
}

#[test]
fn invalid_history_controls_and_disable_preserve_classical_adaa() {
    let mut candidate = make(4.0);
    install(&mut candidate, 4.0);
    candidate.set_qa_candidate_enabled(true);
    let mut classical = make(4.0);
    let mut input = source();
    input[31] = 0.95;
    let actual = render(&mut candidate, &input, 17);
    let baseline = render(&mut classical, &input, 193);
    assert_eq!(actual[31..34], baseline[31..34]);
    assert_eq!(candidate.qa_candidate_counters().unwrap().out_of_domain, 3);
    candidate.set_qa_candidate_enabled(false);
    assert_eq!(
        render(&mut candidate, &input, 17),
        render(&mut classical, &input, 193)
    );
    candidate.set_qa_candidate_enabled(true);
    for p in [&mut candidate, &mut classical] {
        p.set_parameter(ParameterId::from("drive"), ParameterValue::Float(6.0))
            .unwrap();
    }
    assert_eq!(
        render(&mut candidate, &input, 17),
        render(&mut classical, &input, 193)
    );
    for p in [&mut candidate, &mut classical] {
        p.set_parameter(ParameterId::from("drive"), ParameterValue::Float(4.0))
            .unwrap();
    }
    // Context stays invalid until explicit reset, even after the drive returns.
    assert_eq!(
        render(&mut candidate, &input, 17),
        render(&mut classical, &input, 193)
    );
    candidate.reset();
    classical.reset();
    let restored = render(&mut candidate, &input, 17);
    assert_ne!(restored, render(&mut classical, &input, 193));
}

#[test]
fn binding_install_and_input_validation_are_transactional() {
    let mut candidate = make(3.0);
    let (bytes, sha, context) = fixture(&candidate, 3.0);
    assert!(
        candidate
            .install_qa_candidate(Some(&bytes), &"0".repeat(64), &context)
            .is_err()
    );
    let mut other = make(4.0);
    assert!(
        other
            .install_qa_candidate(Some(&bytes), &sha, &context)
            .is_err()
    );
    candidate
        .install_qa_candidate(Some(&bytes), &sha, &context)
        .unwrap();
    candidate.set_qa_candidate_enabled(true);
    let mut control = make(3.0);
    install(&mut control, 3.0);
    control.set_qa_candidate_enabled(true);
    candidate
        .set_parameter(ParameterId::from("drive"), ParameterValue::Float(4.0))
        .unwrap();
    candidate
        .process_in_place(&mut [], &ProcessContext::new(48000, 0))
        .unwrap();
    candidate
        .set_parameter(ParameterId::from("drive"), ParameterValue::Float(3.0))
        .unwrap();
    let mut invalid = [f32::NAN];
    assert!(
        candidate
            .process_in_place(&mut invalid, &ProcessContext::new(48000, 1))
            .is_err()
    );
    let input = source();
    assert_eq!(
        render(&mut candidate, &input, 17),
        render(&mut control, &input, 193)
    );
    assert!(candidate.install_qa_candidate(None, "", b"").is_err());
}

#[test]
fn native_dc_output_and_mix_remain_after_neural_wet_selection() {
    let mut controls = params(4.0);
    controls.use_adaa = false;
    controls.dc_blocker_enabled = true;
    controls.mix = 0.37;
    controls.output_gain_db = -3.0;
    let mut candidate = SaturationPlugin::try_from_params(1, controls.clone()).unwrap();
    candidate.initialize(48000.0).unwrap();
    install(&mut candidate, 4.0);
    candidate.set_qa_candidate_enabled(true);
    let mut native = SaturationPlugin::try_from_params(1, controls).unwrap();
    native.initialize(48000.0).unwrap();
    let input = source();
    let actual = render(&mut candidate, &input, 17);
    let expected = render(&mut native, &input, 193);
    let error = actual
        .iter()
        .zip(expected)
        .map(|(a, b)| (*a - b).abs())
        .fold(0.0, f32::max);
    assert!(error < 2e-6, "post-stage reference error {error}");
}

fn bin_power(samples: &[f32], bin: usize) -> f64 {
    let (mut real, mut imag) = (0.0, 0.0);
    for (n, x) in samples.iter().enumerate() {
        let phase = std::f64::consts::TAU * bin as f64 * n as f64 / samples.len() as f64;
        real += f64::from(*x) * phase.cos();
        imag -= f64::from(*x) * phase.sin();
    }
    (real * real + imag * imag) / (samples.len() * samples.len()) as f64
}

fn spectrum_power(samples: &[f32]) -> Vec<f64> {
    // Independent test-only radix-2 complex transform, not the resampler FFT.
    let n = samples.len();
    assert!(n.is_power_of_two());
    let mut x: Vec<_> = samples.iter().map(|v| (f64::from(*v), 0.0)).collect();
    for i in 0..n {
        let j = i.reverse_bits() >> (usize::BITS - n.trailing_zeros());
        if j > i {
            x.swap(i, j);
        }
    }
    let mut width = 2;
    while width <= n {
        for start in (0..n).step_by(width) {
            for j in 0..width / 2 {
                let angle = -std::f64::consts::TAU * j as f64 / width as f64;
                let (c, s) = (angle.cos(), angle.sin());
                let (br, bi) = x[start + j + width / 2];
                let b = (br * c - bi * s, br * s + bi * c);
                let a = x[start + j];
                x[start + j] = (a.0 + b.0, a.1 + b.1);
                x[start + j + width / 2] = (a.0 - b.0, a.1 - b.1);
            }
        }
        width *= 2;
    }
    x.into_iter()
        .map(|(r, i)| (r * r + i * i) / (n * n) as f64)
        .collect()
}

fn nonharmonic_ratio(power: &[f64], fundamental: usize) -> f64 {
    let n = power.len();
    let energy = power
        .iter()
        .enumerate()
        .filter(|(k, _)| {
            let physical = (*k).min(n - k);
            physical != 0 && !physical.is_multiple_of(fundamental)
        })
        .map(|(_, p)| p)
        .sum::<f64>();
    10.0 * (energy / (power[fundamental] + power[n - fundamental]).max(1e-30))
        .max(1e-30)
        .log10()
}

#[test]
fn independent_spectrum_preserves_parseval_and_matches_scalar_projections() {
    let signal: Vec<_> = (0..4096)
        .map(|n| (0.3 * (std::f64::consts::TAU * 43.0 * n as f64 / 4096.0).sin()) as f32)
        .collect();
    let power = spectrum_power(&signal);
    let energy = signal.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / 4096.0;
    assert!((power.iter().sum::<f64>() - energy).abs() < 1e-12);
    assert!((power[43] - 0.3_f64.powi(2) / 4.0).abs() < 1e-9);
    assert!((power[43] - bin_power(&signal, 43)).abs() < 1e-12);
    let mut fixture = vec![0.0; 4096];
    fixture[999] = 1.0;
    fixture[4096 - 999] = 1.0;
    // The second harmonic is legitimate; the third folds to1099 and must be
    // counted even though its negative-frequency index is divisible by999.
    fixture[1998] = 0.5;
    fixture[4096 - 1998] = 0.5;
    fixture[1099] = 0.01;
    fixture[4096 - 1099] = 0.01;
    assert!((nonharmonic_ratio(&fixture, 999) + 20.0).abs() < 1e-12);
}

#[test]
fn analytic_neural_fixtures_expose_folded_harmonics_in_parameter_sweeps() {
    const N: usize = 4096;
    let mut rows = Vec::new();
    for drive in [1.0_f32, 4.0, 12.0, 20.0] {
        for fundamental in [401, 999] {
            let input: Vec<_> = (0..N * 3)
                .map(|n| {
                    (0.7 * (std::f64::consts::TAU * fundamental as f64 * n as f64 / N as f64).sin())
                        as f32
                })
                .collect();
            let mut candidate = make(drive);
            install(&mut candidate, f64::from(drive));
            candidate.set_qa_candidate_enabled(true);
            let learned = render(&mut candidate, &input, 193);
            let native = render(&mut make(drive), &input, 193);
            let mut folded = Vec::new();
            for harmonic in (3..=31).step_by(2) {
                if harmonic * fundamental <= N / 2 {
                    continue;
                }
                let bin = (harmonic * fundamental) % N;
                let bin = bin.min(N - bin);
                if bin != fundamental && !folded.contains(&bin) {
                    folded.push(bin);
                }
            }
            let ratio = |samples: &[f32]| {
                let tail = &samples[samples.len() - N..];
                let energy = folded.iter().map(|b| bin_power(tail, *b)).sum::<f64>();
                10.0 * (energy / bin_power(tail, fundamental).max(1e-30))
                    .max(1e-30)
                    .log10()
            };
            let learned_db = ratio(&learned);
            let classical_db = ratio(&native);
            let learned_total =
                nonharmonic_ratio(&spectrum_power(&learned[learned.len() - N..]), fundamental);
            let classical_total =
                nonharmonic_ratio(&spectrum_power(&native[native.len() - N..]), fundamental);
            assert!(learned_db.is_finite() && classical_db.is_finite());
            assert!(learned_db > -110.0);
            rows.push(serde_json::json!({"drive":drive,"fundamental_bin":fundamental,"frequency_hz":48000.0*fundamental as f64/N as f64,
                "sample_rate":48000,"alias_bins":folded,"learned_folded_to_fundamental_db":learned_db,"native_adaa_folded_to_fundamental_db":classical_db,
                "learned_total_nonharmonic_to_fundamental_db":learned_total,"native_adaa_total_nonharmonic_to_fundamental_db":classical_total,
                "fixture":"untrained direct tanh; not measured hardware","model_added_lookahead_samples":0}));
        }
    }
    println!(
        "{}",
        serde_json::json!({"alias_parameter_sweep":rows,"deployment_admitted":false})
    );
}

fn wrapped(
    drive: f32,
    rate: u32,
    factor: u32,
    capture_rate: Option<u32>,
) -> Box<dyn sotf_host::Plugin> {
    use sotf_host::oversampling::AutoOversampledPlugin;
    use sotf_host::{ParametricInPlacePluginAdapter, Plugin};
    let mut native = SaturationPlugin::try_from_params(1, params(drive)).unwrap();
    native
        .initialize(f64::from(capture_rate.unwrap_or(rate * factor)))
        .unwrap();
    if let Some(clock) = capture_rate {
        let (bytes, sha, context) = fixture_at_rate(&native, f64::from(drive), clock);
        native
            .install_qa_candidate(Some(&bytes), &sha, &context)
            .unwrap();
        native.set_qa_candidate_enabled(true);
    }
    let mut wrapper = AutoOversampledPlugin::new_with_max_frames(
        Box::new(ParametricInPlacePluginAdapter::new(native)),
        factor,
        193,
    )
    .unwrap();
    wrapper.initialize(f64::from(rate)).unwrap();
    Box::new(wrapper)
}

fn render_wrapped(
    plugin: &mut dyn sotf_host::Plugin,
    input: &[f32],
    rate: u32,
    block: usize,
) -> Vec<f32> {
    let mut output = vec![0.0; input.len()];
    for (source, destination) in input.chunks(block).zip(output.chunks_mut(block)) {
        let context = ProcessContext::new(rate, source.len());
        assert_no_allocs_or_deallocs("oversampled neural process", || {
            plugin.process(source, destination, &context).unwrap();
        });
    }
    output
}

#[test]
fn oversampled_candidate_is_capture_clock_bound_and_callback_independent() {
    let input = source();
    for factor in [2, 4] {
        let mut candidate = wrapped(4.0, 48000, factor, Some(48000 * factor));
        let latency = candidate.latency_samples();
        assert!(latency > 0);
        let expected = render_wrapped(candidate.as_mut(), &input, 48000, 193);
        assert_no_allocs_or_deallocs("oversampled neural reset", || candidate.reset());
        assert_eq!(
            render_wrapped(candidate.as_mut(), &input, 48000, 17),
            expected
        );
        let mut wrong_clock = wrapped(4.0, 48000, factor, Some(48000));
        let mut classical = wrapped(4.0, 48000, factor, None);
        assert_eq!(
            render_wrapped(wrong_clock.as_mut(), &input, 48000, 17),
            render_wrapped(classical.as_mut(), &input, 48000, 193)
        );
        assert_eq!(wrong_clock.latency_samples(), latency);
    }
}

#[test]
fn oversampled_neural_alias_sweeps_report_actual_clock_latency_and_folded_energy() {
    const N: usize = 4096;
    let mut rows = Vec::new();
    for rate in [44100, 48000] {
        for factor in [2, 4] {
            for drive in [1.0_f32, 4.0, 12.0, 20.0] {
                for fundamental in [401, 999] {
                    let input: Vec<_> = (0..N * 4)
                        .map(|n| {
                            (0.7 * (std::f64::consts::TAU * fundamental as f64 * n as f64
                                / N as f64)
                                .sin()) as f32
                        })
                        .collect();
                    let mut candidate = wrapped(drive, rate, factor, Some(rate * factor));
                    let output = render_wrapped(candidate.as_mut(), &input, rate, 193);
                    let tail = &output[output.len() - N..];
                    let mut bins = Vec::new();
                    for h in (3..=31).step_by(2) {
                        if h * fundamental <= N / 2 {
                            continue;
                        }
                        let bin = (h * fundamental) % N;
                        let bin = bin.min(N - bin);
                        if bin != fundamental && !bins.contains(&bin) {
                            bins.push(bin);
                        }
                    }
                    let fundamental_power = bin_power(tail, fundamental);
                    let alias = bins.iter().map(|bin| bin_power(tail, *bin)).sum::<f64>();
                    let power = spectrum_power(tail);
                    for bin in &bins {
                        assert!((power[*bin] - bin_power(tail, *bin)).abs() < 1e-12);
                    }
                    let total_nonharmonic = nonharmonic_ratio(&power, fundamental);
                    let ratio = 10.0 * (alias / fundamental_power.max(1e-30)).max(1e-30).log10();
                    assert!(ratio.is_finite());
                    rows.push(serde_json::json!({"sample_rate":rate,"factor":factor,"capture_sample_rate":rate*factor,
                        "drive":drive,"fundamental_bin":fundamental,"frequency_hz":f64::from(rate)*fundamental as f64/N as f64,
                        "selected_folded_to_fundamental_db":ratio,"fundamental_power":fundamental_power,"latency_samples":candidate.latency_samples(),
                        "total_nonharmonic_to_fundamental_db":total_nonharmonic,
                        "added_model_lookahead_samples":0,"alias_bins":bins,"fixture":"untrained direct tanh at actual processing clock"}));
                }
            }
        }
    }
    println!(
        "{}",
        serde_json::json!({"oversampled_alias_parameter_sweep":rows,"deployment_admitted":false})
    );
}
