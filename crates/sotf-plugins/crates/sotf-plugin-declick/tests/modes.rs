//! Independent accuracy suite for declick modes (DECLICK-A1/A2/A3).
//!
//! Tolerances are fixed from the published eight-sample latency contract,
//! f32 reference precision, and the already accepted repair bounds (0.05
//! absolute for an isolated click, 5% of click amplitude). Detection
//! precision/recall is measured through the aligned residual tap, which is
//! nonzero exactly where repair substituted interpolation for dry input.

// Rust guideline compliant 2026-02-21
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::parametric_plugin::ParameterSet;
use sotf_host::plugin::{ProcessContext, TailLength};
use sotf_plugin_declick::{DeclickPlugin, DeclickPluginParams};
use std::cmp::Ordering;

const SR: u32 = 48_000;
const CLICK_AMP: f32 = 3.0;
/// Residual magnitude above which a frame counts as repaired.
///
/// Click amplitudes are 3.0 against sub-1.0 programme, so repaired click
/// frames exceed this by an order of magnitude while interpolation residue
/// on clean frames stays two orders below.
const HOT_RESIDUAL: f32 = 0.5;
/// Post-context frames every asserted control sample enjoys.
///
/// Matches the eight-sample detection lookahead, so no asserted frame sees
/// flush zeros in its post window.
const FULL_CONTEXT: usize = 8;
/// Blowup-exclusion peak for 2-band boundary outputs (click + click-free).
///
/// Conservative hull (measured worsts sit near 0.05-0.26 across the
/// boundedness matrix); values inside prove no wild extrapolation, but this
/// pin alone proves no repair-error bound. Accuracy at the defined
/// zero-continuation endpoint is proven by
/// `a3_multiband_eof_endpoint_error_oracle` (0.15/0.05 vs differential and
/// analytical references), not by this magnitude leg. Removal (>1.0 from
/// the 3.25 spike) is asserted alongside.
const MULTIBAND_PEAK_2BAND: f32 = 1.0;
/// Blowup-exclusion peak for 3-band boundary outputs: 2.0.
///
/// Same role as the 2-band pin with headroom for the three-band cascade;
/// measured worsts sit near 0.08-0.26. Accuracy is proven by the endpoint
/// error oracle, not here. Removal still holds (3.25-2.0=1.25>1.0).
const MULTIBAND_PEAK_3BAND: f32 = 2.0;

/// Report a measured worst case for the coordinator record.
///
/// Visible with `cargo test -- --nocapture`; assertions below never depend
/// on these lines.
fn report(label: &str, worst: f32) {
    eprintln!("[declick-accuracy] {label} worst={worst:.6}");
}

fn params() -> DeclickPluginParams {
    DeclickPluginParams {
        sensitivity: 2.0,
        ..Default::default()
    }
}

fn run(plugin: &mut DeclickPlugin, signal: &[f32], channels: usize, rate: u32) -> Vec<f32> {
    let mut stream = signal.to_vec();
    let frames = stream.len() / channels;
    plugin
        .process_in_place(&mut stream, &ProcessContext::new(rate, frames))
        .unwrap();
    stream
}

/// Process `signal` plus enough zero padding to flush every input frame
/// through the plugin's reported latency.
fn run_flushed(plugin: &mut DeclickPlugin, signal: &[f32], channels: usize, rate: u32) -> Vec<f32> {
    let latency = plugin.latency_samples();
    let mut stream = signal.to_vec();
    stream.extend(std::iter::repeat_n(0.0, latency * channels));
    let frames = stream.len() / channels;
    plugin
        .process_in_place(&mut stream, &ProcessContext::new(rate, frames))
        .unwrap();
    stream
}

fn run_drained(plugin: &mut DeclickPlugin, signal: &[f32], channels: usize, rate: u32) -> Vec<f32> {
    let mut output = run(plugin, signal, channels, rate);
    loop {
        let mut block = vec![0.0; 64 * channels];
        let status = plugin
            .drain(&mut block, &ProcessContext::new(rate, 64))
            .unwrap();
        output.extend_from_slice(&block[..status.frames * channels]);
        if status.complete {
            return output;
        }
    }
}

fn sine(frames: usize, freq_hz: f32, rate: f32, amp: f32) -> Vec<f32> {
    (0..frames)
        .map(|i| (i as f32 * freq_hz / rate * std::f32::consts::TAU).sin() * amp)
        .collect()
}

/// Fixed click plan: (start frame, width, sign).
fn click_plan() -> Vec<(usize, usize, f32)> {
    vec![
        (64, 1, 1.0),
        (128, 1, -1.0),
        (200, 3, 1.0),
        (300, 1, 1.0),
        (400, 3, -1.0),
        (500, 1, 1.0),
        (640, 1, -1.0),
        (760, 3, 1.0),
    ]
}

fn corrupt(clean: &[f32], plan: &[(usize, usize, f32)]) -> (Vec<f32>, Vec<bool>) {
    let mut signal = clean.to_vec();
    let mut is_click = vec![false; clean.len()];
    for &(start, width, sign) in plan {
        for offset in 0..width {
            signal[start + offset] += sign * CLICK_AMP;
            is_click[start + offset] = true;
        }
    }
    (signal, is_click)
}

/// Run one endpoint signal-type control case through exact drain.
///
/// Builds the named control (dual-mono across `channels`), renders it with
/// the given multiband topology, and asserts against the independent
/// analytical reference: 1e-5 transparency for `dc`, `dc_step`,
/// `silence_transition`, and `interior_zeros` (all must stay dry, including
/// the EOF edge with drain zeros); 0.15 repair at impulse frames plus 0.05
/// damage outside +-2 for `impulse` (click-like, repaired as clicks).
#[allow(
    clippy::too_many_arguments,
    reason = "test helper: one argument per case dimension"
)]
fn run_control_case(
    tag: &str,
    bands_param: usize,
    channels: usize,
    rate: u32,
    crossover_hz: f32,
    width: usize,
    control: &str,
    frames: usize,
) {
    let tone = sine(frames, 440.0, rate as f32, 0.25);
    let (mono, analytical, impulses): (Vec<f32>, Vec<f32>, Vec<bool>) = match control {
        "dc" => (vec![0.3; frames], vec![0.3; frames], vec![false; frames]),
        "dc_step" => {
            let mut signal = vec![0.2; frames];
            for sample in signal.iter_mut().skip(frames / 2) {
                *sample = 0.5;
            }
            (signal.clone(), signal, vec![false; frames])
        }
        "silence_transition" => {
            let mut signal = tone.clone();
            for sample in signal.iter_mut().skip(frames / 2) {
                *sample = 0.0;
            }
            (signal.clone(), signal, vec![false; frames])
        }
        "interior_zeros" => {
            let mut signal = tone.clone();
            // Exact zeros at the three interior frames closest to zero
            // crossings (tones nearest 0.0 in 64..192).
            let mut near: Vec<(usize, f32)> = (64..192).map(|i| (i, tone[i].abs())).collect();
            near.sort_by(|a, b| a.1.total_cmp(&b.1));
            for (i, _) in near.iter().take(3) {
                signal[*i] = 0.0;
            }
            (signal.clone(), signal, vec![false; frames])
        }
        "impulse" => {
            let mut signal = tone.clone();
            let mut hot = vec![false; frames];
            for &at in &[100, 200] {
                signal[at] += 1.0;
                hot[at] = true;
            }
            (signal, tone, hot)
        }
        _ => panic!("{tag}: unknown control {control}"),
    };
    let mut input = vec![0.0; frames * channels];
    let mut expected = vec![0.0; frames * channels];
    for ch in 0..channels {
        for frame in 0..frames {
            input[frame * channels + ch] = mono[frame];
            expected[frame * channels + ch] = analytical[frame];
        }
    }
    let template = DeclickPluginParams {
        mode: 0,
        bands: bands_param,
        repair_width: width,
        link_channels: true,
        sensitivity: 2.0,
        crossover_hz,
        ..Default::default()
    };
    let mut plugin = DeclickPlugin::from_params(channels, rate, template).unwrap();
    let latency = plugin.latency_samples();
    assert_eq!(latency, FULL_CONTEXT + width, "{tag}: latency");
    let output = run_drained(&mut plugin, &input, channels, rate);
    assert_eq!(output.len(), (frames + latency) * channels, "{tag}: length");
    assert!(output.iter().all(|s| s.is_finite()), "{tag}: finite");
    let mut worst = 0.0_f32;
    for frame in 32..frames {
        for ch in 0..channels {
            let actual = output[(frame + latency) * channels + ch];
            let reference = expected[frame * channels + ch];
            if impulses[frame] {
                let error = (actual - reference).abs();
                worst = worst.max(error);
                assert!(
                    error < 0.15,
                    "{tag}: impulse ch{ch} frame={frame} error={error}"
                );
            } else {
                if control == "impulse" {
                    let start = frame.saturating_sub(2);
                    let end = (frame + 3).min(frames);
                    if impulses[start..end].contains(&true) {
                        continue;
                    }
                }
                let error = (actual - reference).abs();
                worst = worst.max(error);
                if control == "impulse" {
                    assert!(
                        error < 0.05,
                        "{tag}: damage ch{ch} frame={frame} error={error}"
                    );
                } else {
                    assert!(
                        error < 1.0e-5,
                        "{tag}: transparency ch{ch} frame={frame} error={error}"
                    );
                }
            }
        }
    }
    report(tag, worst);
}

#[test]
fn a1_precision_recall_repair_error_and_clean_damage() {
    for mode in [0, 1] {
        for bands in [0, 1, 2] {
            let clean = sine(1024, 440.0, SR as f32, 0.2);
            let plan = click_plan();
            let (signal, is_click) = corrupt(&clean, &plan);
            let base = DeclickPluginParams {
                mode,
                bands,
                ..params()
            };

            let mut repaired_plugin = DeclickPlugin::from_params(1, SR, base.clone()).unwrap();
            let latency = repaired_plugin.latency_samples();
            let repaired = run_flushed(&mut repaired_plugin, &signal, 1, SR);

            let mut residual_plugin = DeclickPlugin::from_params(
                1,
                SR,
                DeclickPluginParams {
                    audition_residual: true,
                    ..base
                },
            )
            .unwrap();
            let residual = run_flushed(&mut residual_plugin, &signal, 1, SR);

            // Precision/recall on latency-aligned frames with settled
            // context (periodic mode needs no lock for isolated clicks:
            // the ungated fallback repairs them like random mode).
            let mut true_hot = 0;
            let mut false_hot = 0;
            let mut missed = 0;
            for frame in 32..1024 {
                let hot = residual[frame + latency].abs() > HOT_RESIDUAL;
                match (is_click[frame], hot) {
                    (true, true) => true_hot += 1,
                    (false, true) => false_hot += 1,
                    (true, false) => missed += 1,
                    (false, false) => {}
                }
            }
            let total_click: usize = plan.iter().map(|(_, width, _)| width).sum();
            let recall = true_hot as f32 / total_click as f32;
            let precision = true_hot as f32 / (true_hot + false_hot).max(1) as f32;
            assert!(
                recall >= 0.95,
                "mode={mode} bands={bands} recall={recall} missed={missed}"
            );
            assert!(
                precision >= 0.95,
                "mode={mode} bands={bands} precision={precision} false_hot={false_hot}"
            );
            eprintln!(
                "[declick-accuracy] mode={mode} bands={bands} recall={recall:.4} precision={precision:.4}"
            );

            // Repair error at click frames: accepted 5%-of-amplitude bound.
            // Signal damage away from clicks: accepted 0.05 absolute bound.
            let mut worst_repair = 0.0_f32;
            let mut worst_damage = 0.0_f32;
            for frame in 32..1024 {
                let error = (repaired[frame + latency] - clean[frame]).abs();
                if is_click[frame] {
                    worst_repair = worst_repair.max(error);
                    assert!(
                        error < CLICK_AMP * 0.05,
                        "mode={mode} bands={bands} frame={frame} error={error}"
                    );
                } else if !(frame.saturating_sub(2)..=frame + 2)
                    .any(|near| plan.iter().any(|&(s, w, _)| near >= s && near < s + w))
                {
                    worst_damage = worst_damage.max(error);
                    assert!(
                        error < 0.05,
                        "mode={mode} bands={bands} frame={frame} damage={error}"
                    );
                }
            }
            report(
                &format!("mode={mode} bands={bands} repair-error"),
                worst_repair,
            );
            report(&format!("mode={mode} bands={bands} damage"), worst_damage);
        }
    }
}

#[test]
fn a1_periodic_mode_repairs_grids_and_protects_off_grid_transients() {
    // Grid clicks every 100 frames plus one sharp off-grid drum hit.
    let frames = 2600;
    let clean = sine(frames, 220.0, SR as f32, 0.2);
    let mut signal = clean.clone();
    let mut grid = Vec::new();
    for start in (140..frames - 64).step_by(100) {
        signal[start] += CLICK_AMP;
        grid.push(start);
    }
    // Three-sample attack, exponential decay: sharp enough to tempt the
    // random detector, off the predicted phase so the periodic guard holds.
    let drum_at = 1290;
    for (offset, gain) in [0.9_f32, 0.7, 0.5].iter().enumerate() {
        signal[drum_at + offset] += *gain;
    }
    for tail in 3..120 {
        signal[drum_at + tail] += 0.5 * (-(tail as f32) / 40.0).exp();
    }
    let mut drum_clean = clean.clone();
    for (offset, gain) in [0.9_f32, 0.7, 0.5].iter().enumerate() {
        drum_clean[drum_at + offset] += *gain;
    }
    for tail in 3..120 {
        drum_clean[drum_at + tail] += 0.5 * (-(tail as f32) / 40.0).exp();
    }

    // Sensitivity 5 keeps grid clicks repairable in every gate state
    // (guarded threshold ~1.5 against amplitude 3.0) while the guard still
    // protects the off-grid drum transient.
    let periodic = DeclickPluginParams {
        mode: 1,
        sensitivity: 5.0,
        ..Default::default()
    };
    let mut plugin = DeclickPlugin::from_params(1, SR, periodic).unwrap();
    let latency = plugin.latency_samples();
    let output = run(&mut plugin, &signal, 1, SR);
    // Locked grid clicks repair to the accepted bound; the guarded drum
    // transient survives within the accepted damage bound. Grid positions
    // overlapping the drum tail are skipped (mixed content).
    let mut worst_grid = 0.0_f32;
    for &position in &grid {
        if position < 1300 || (drum_at..drum_at + 120).contains(&position) {
            continue;
        }
        let error = (output[position + latency] - clean[position]).abs();
        worst_grid = worst_grid.max(error);
        assert!(error < 0.15, "grid click at {position}: error={error}");
    }
    report("periodic grid repair-error", worst_grid);
    let mut drum_damage = 0.0_f32;
    for frame in drum_at..drum_at + 120 {
        drum_damage = drum_damage.max((output[frame + latency] - drum_clean[frame]).abs());
    }
    assert!(drum_damage < 0.05, "drum damage={drum_damage}");
    report("periodic drum damage", drum_damage);

    // Without repetition the periodic tracker never locks, so periodic and
    // random outputs are exactly equal on the drum-only signal.
    let drum_only: Vec<f32> = drum_clean.clone();
    let mut random_plugin = DeclickPlugin::from_params(
        1,
        SR,
        DeclickPluginParams {
            sensitivity: 5.0,
            ..Default::default()
        },
    )
    .unwrap();
    let random_out = run(&mut random_plugin, &drum_only, 1, SR);
    let mut periodic_plugin = DeclickPlugin::from_params(
        1,
        SR,
        DeclickPluginParams {
            mode: 1,
            sensitivity: 5.0,
            ..Default::default()
        },
    )
    .unwrap();
    let periodic_out = run(&mut periodic_plugin, &drum_only, 1, SR);
    assert_eq!(random_out, periodic_out);
}

#[test]
fn a1_clean_controls_are_preserved() {
    // Step, onset, high-frequency tone, and square wave: exact, matching
    // the accepted legacy behavior on every mode/band combination. Each
    // control carries an 8-frame natural continuation past the asserted
    // 256-frame span: the accepted legacy contract
    // (`clean_high_frequency_and_square_signals_are_not_repaired` in the
    // shared suppressor suite) pins bit-exact transparency only for frames
    // whose 8-sample post context lies inside the signal, because flush
    // zeros past the last frame form a genuine discontinuity the detector
    // may legitimately repair. The asserted span therefore always enjoys
    // full in-signal context; the flush-boundary behavior itself is pinned
    // structurally by `a1_flush_boundary_follows_legacy_continuation`.
    let rate = SR as f32;
    let span = 256;
    let controls: Vec<(&str, Vec<f32>)> = vec![
        ("step", {
            let mut step = vec![0.0; 32];
            step.extend(vec![0.8; 224 + FULL_CONTEXT]);
            step
        }),
        ("onset", vec![0.8; 256 + FULL_CONTEXT]),
        ("hf_tone", sine(span + FULL_CONTEXT, 12_000.0, rate, 0.5)),
        (
            "square",
            (0..span + FULL_CONTEXT)
                .map(|i| if (i / 12) % 2 == 0 { -0.5 } else { 0.5 })
                .collect(),
        ),
    ];
    for mode in [0, 1] {
        for bands in [0, 1, 2] {
            for (name, control) in &controls {
                let mut plugin = DeclickPlugin::from_params(
                    1,
                    SR,
                    DeclickPluginParams {
                        mode,
                        bands,
                        sensitivity: 1.0,
                        ..Default::default()
                    },
                )
                .unwrap();
                let latency = plugin.latency_samples();
                let output = run_flushed(&mut plugin, control, 1, SR);
                let expected = &control[..span];
                if bands == 0 && mode == 0 {
                    assert_eq!(&output[latency..latency + span], expected, "control={name}");
                } else {
                    // Multiband/owned paths add crossover rounding only.
                    let worst = output[latency..latency + span]
                        .iter()
                        .zip(expected.iter())
                        .map(|(a, b)| (a - b).abs())
                        .fold(0.0_f32, f32::max);
                    assert!(
                        worst < 1.0e-5,
                        "mode={mode} bands={bands} control={name} worst={worst}"
                    );
                    report(&format!("mode={mode} bands={bands} control={name}"), worst);
                }
            }
        }
    }
    // Gradual drum transient (16-sample raised-cosine attack): no exact
    // precedent exists, so the accepted 0.05 damage bound applies.
    let mut drum = vec![0.0; 512];
    for i in 0..16 {
        let t = i as f32 / 16.0;
        drum[100 + i] = 0.8 * (0.5 - 0.5 * (t * std::f32::consts::PI).cos());
    }
    for tail in 0..200 {
        drum[116 + tail] = 0.8 * (-(tail as f32) / 120.0).exp();
    }
    for mode in [0, 1] {
        let mut plugin = DeclickPlugin::from_params(
            1,
            SR,
            DeclickPluginParams {
                mode,
                sensitivity: 2.0,
                ..Default::default()
            },
        )
        .unwrap();
        let latency = plugin.latency_samples();
        let output = run_flushed(&mut plugin, &drum, 1, SR);
        let worst = output[latency..latency + drum.len()]
            .iter()
            .zip(drum.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);
        assert!(worst < 0.05, "mode={mode} drum damage={worst}");
        report(&format!("gradual drum mode={mode}"), worst);
    }
}

#[test]
fn a1_flush_boundary_follows_legacy_continuation() {
    // The accepted legacy contract leaves the last lookahead frames
    // unpinned (flush zeros past the signal end form a genuine
    // discontinuity there); this pins what IS contracted on every
    // mode/band combination: pre-boundary output is bit-identical with and
    // without a natural continuation, and boundary frames stay finite. The
    // legacy boundary additionally stays inside the programme range (median
    // average or dry, plus mix regrouping rounding). The repair direction
    // at the boundary is deliberately NOT pinned: residual sits at
    // threshold there by construction, so it is marginal by design and the
    // shared suite leaves it unpinned for the same reason.
    let full = sine(256 + FULL_CONTEXT, 12_000.0, SR as f32, 0.5);
    let control = &full[..256];
    for mode in [0, 1] {
        for bands in [0, 1, 2] {
            let params = DeclickPluginParams {
                mode,
                bands,
                sensitivity: 1.0,
                ..Default::default()
            };
            let mut flushed = DeclickPlugin::from_params(1, SR, params.clone()).unwrap();
            let latency = flushed.latency_samples();
            let boundary_out = run_flushed(&mut flushed, control, 1, SR);
            let mut continued = DeclickPlugin::from_params(1, SR, params).unwrap();
            let continued_out = run_flushed(&mut continued, &full, 1, SR);
            // The flush discontinuity affects only the last lookahead
            // frames: everything before agrees bit-exactly.
            assert_eq!(
                &boundary_out[latency..latency + 256 - FULL_CONTEXT],
                &continued_out[latency..latency + 256 - FULL_CONTEXT],
                "mode={mode} bands={bands}"
            );
            let boundary = &boundary_out[latency + 256 - FULL_CONTEXT..latency + 256];
            assert!(
                boundary.iter().all(|sample| sample.is_finite()),
                "mode={mode} bands={bands}"
            );
            if mode == 0 && bands == 0 {
                for sample in boundary {
                    assert!(
                        sample.abs() <= 0.5 + 1.0e-6,
                        "control boundary out of range: {sample}"
                    );
                }
            }
        }
    }
}

#[test]
fn a3_multiband_eof_repair_values_are_bounded() {
    // Boundedness regression for multiband EOF (DECLICK-A3 partial final
    // clicks): removal plus conservative magnitude pins (2-band 1.0, 3-band
    // 2.0, see the `MULTIBAND_PEAK_*` constants) exclude blowup/NaN/wild
    // values, but prove no repair-error bound on their own. Accuracy at the
    // contract-defined zero-continuation endpoint (drain zeros through the
    // same crossover/detector; drain is bit-identical to zero-padded
    // processing) is proven by `a3_multiband_eof_endpoint_error_oracle`
    // (0.15/0.05 vs differential and analytical references). Full-context
    // frames here keep the frozen 0.05/1e-5 bounds. Covers both modes (mode
    // 1 never locks in 256-frame streams, so it renders identically to mode
    // 0 here), both multiband topologies, three rates, mono/stereo, linked
    // and independent channels, repair widths 0/3, last-frame 1/2-wide
    // clicks and click-free clean controls at the default crossover, all
    // through exact drain to EOF. Width 8, 3-wide EOF clicks, crossover
    // extremes, and locked periodic behavior are covered in the error-oracle
    // and periodic-lock legs.
    let frames = 256;
    let freqs = [440.0, 660.0];
    for mode in [0, 1] {
        for bands_param in [1, 2] {
            let band_count = bands_param + 1;
            let peak = if bands_param == 1 {
                MULTIBAND_PEAK_2BAND
            } else {
                MULTIBAND_PEAK_3BAND
            };
            for rate in [44_100, 48_000, 96_000] {
                for channels in [1, 2] {
                    for linked in [true, false] {
                        for width in [0, 3] {
                            let tag = format!(
                                "mode={mode} bands={band_count} rate={rate} ch={channels} linked={linked} width={width}"
                            );
                            // Click plan: last-frame partial clicks (mono
                            // single at the last frame; stereo adds a
                            // double-wide straddling the last two on ch1).
                            let mut plans: Vec<Vec<usize>> = vec![Vec::new(); channels];
                            plans[0].push(frames - 1);
                            if channels > 1 {
                                plans[1].push(frames - 2);
                                plans[1].push(frames - 1);
                            }
                            let mut clean = vec![0.0; frames * channels];
                            let mut corrupted = vec![0.0; frames * channels];
                            let mut is_click = vec![vec![false; frames]; channels];
                            for ch in 0..channels {
                                let tone = sine(frames, freqs[ch], rate as f32, 0.25);
                                for frame in 0..frames {
                                    clean[frame * channels + ch] = tone[frame];
                                    corrupted[frame * channels + ch] = tone[frame];
                                }
                                for &at in &plans[ch] {
                                    let sign = if ch == 0 { 1.0 } else { -1.0 };
                                    corrupted[at * channels + ch] += sign * CLICK_AMP;
                                    is_click[ch][at] = true;
                                }
                            }
                            let template = DeclickPluginParams {
                                mode,
                                bands: bands_param,
                                repair_width: width,
                                link_channels: linked,
                                sensitivity: 2.0,
                                ..Default::default()
                            };
                            let mut plugin =
                                DeclickPlugin::from_params(channels, rate, template.clone())
                                    .unwrap();
                            let latency = plugin.latency_samples();
                            assert_eq!(latency, FULL_CONTEXT + width, "{tag}: latency");
                            let output = run_drained(&mut plugin, &corrupted, channels, rate);
                            assert_eq!(
                                output.len(),
                                (frames + latency) * channels,
                                "{tag}: length"
                            );
                            assert!(
                                output.iter().all(|sample| sample.is_finite()),
                                "{tag}: finite"
                            );
                            for (index, sample) in
                                output.iter().take(latency * channels).enumerate()
                            {
                                assert_eq!(*sample, 0.0, "{tag}: leading silence {index}");
                            }
                            let context_end = frames - FULL_CONTEXT;
                            let guard = width + 2;
                            let mut worst_peak = 0.0_f32;
                            let mut worst_damage = 0.0_f32;
                            for input in 32..frames {
                                for ch in 0..channels {
                                    let actual = output[(input + latency) * channels + ch];
                                    if is_click[ch][input] {
                                        let spike = corrupted[input * channels + ch];
                                        assert!(
                                            (actual - spike).abs() > 1.0,
                                            "{tag}: ch{ch} input={input} spike passed through"
                                        );
                                        worst_peak = worst_peak.max(actual.abs());
                                        assert!(
                                            actual.abs() <= peak,
                                            "{tag}: boundary repair ch{ch} input={input} value={actual}"
                                        );
                                    } else if input < context_end {
                                        let start = input.saturating_sub(guard);
                                        let end = (input + guard + 1).min(frames);
                                        if is_click[ch][start..end].contains(&true) {
                                            continue;
                                        }
                                        let expected = clean[input * channels + ch];
                                        let damage = (actual - expected).abs();
                                        worst_damage = worst_damage.max(damage);
                                        assert!(
                                            damage < 0.05,
                                            "{tag}: damage ch{ch} input={input} damage={damage}"
                                        );
                                    } else {
                                        worst_peak = worst_peak.max(actual.abs());
                                        assert!(
                                            actual.abs() <= peak,
                                            "{tag}: boundary programme ch{ch} input={input} value={actual}"
                                        );
                                    }
                                }
                            }
                            report(&format!("{tag} boundary-peak"), worst_peak);
                            report(&format!("{tag} damage"), worst_damage);
                            // Clean control: same tone, no clicks. Full-
                            // context frames stay transparent (1e-5, matching
                            // the accepted multiband clean precedent);
                            // boundary frames stay inside the endpoint peak.
                            let mut clean_plugin =
                                DeclickPlugin::from_params(channels, rate, template).unwrap();
                            let clean_out = run_drained(&mut clean_plugin, &clean, channels, rate);
                            assert_eq!(
                                clean_out.len(),
                                (frames + latency) * channels,
                                "{tag} clean: length"
                            );
                            let mut worst_clean = 0.0_f32;
                            let mut worst_clean_peak = 0.0_f32;
                            for input in 32..frames {
                                for ch in 0..channels {
                                    let actual = clean_out[(input + latency) * channels + ch];
                                    let expected = clean[input * channels + ch];
                                    if input < context_end {
                                        let damage = (actual - expected).abs();
                                        worst_clean = worst_clean.max(damage);
                                        assert!(
                                            damage < 1.0e-5,
                                            "{tag} clean: ch{ch} input={input} damage={damage}"
                                        );
                                    } else {
                                        worst_clean_peak = worst_clean_peak.max(actual.abs());
                                        assert!(
                                            actual.abs() <= peak,
                                            "{tag} clean: ch{ch} input={input} value={actual}"
                                        );
                                    }
                                }
                            }
                            report(&format!("{tag} clean"), worst_clean);
                            report(&format!("{tag} clean-peak"), worst_clean_peak);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn a3_multiband_eof_endpoint_error_oracle() {
    // Zero-continuation endpoint ERROR oracle for multiband EOF (DECLICK-A1
    // repair error / false-positive damage, A3 partial final clicks). Three
    // stated estimands, no boundary exclusion, original bounds only:
    // (D) differential repair error |corrupted_plugin - clean_plugin| at
    // click frames <0.15 (same plugin on clean tones through the identical
    // drain; isolates click removal; useful control, NOT independent).
    // (I) independent clean damage |clean_plugin - analytical| on clean
    // frames outside the (width+2) footprint <0.05, where analytical is the
    // clean tone itself (aligned by input index, no plugin); this is the
    // independent false-positive measure (differential alone would hide
    // common bias).
    // (E) end-to-end error |corrupted_plugin - analytical| <0.15 at clicks,
    // <0.05 on clean outside the footprint (fully independent accuracy).
    // Plus removal >1.0, PR >=0.95 (loud clicks, hot>0.5, counted over
    // 32..frames with no exclusion), regroup <1e-5 vs the delayed
    // corrupted input, and exact geometry. Random mode only here (mode 1
    // never locks in 256-frame streams, proven identical in the
    // boundedness leg); locked periodic behavior is covered in
    // `a3_multiband_periodic_eof_locks_and_repairs`. Matrix: both
    // multiband topologies x 4 rates x mono/stereo-linked/stereo-split x
    // widths 0/3/8 x 1/2/3-wide EOF clicks x crossovers 80/4k/12kHz at
    // neutral skew (skew extremes live in
    // `a3_skew_endpoint_error_oracle`, same estimands and bounds).
    // Stereo uses ch0 W-wide clicks with ch1 clean to exercise link
    // coupling (linked forces ch1 repairs inside the footprint, split does
    // not); both must meet the error bounds. 192kHz is covered here
    // (R15 192k disposition: DECLICK-A3 enumerates 44.1/48/96kHz, all
    // covered; 192kHz is COMMON-typical and admitted by the code, which
    // gates no rate, so the oracle proves it rather than exempting it;
    // periodic/whole-chain 192kHz stays an open residual, not a claim).
    let frames = 256;
    let freqs = [440.0, 660.0];
    let chan_configs = [(1, true), (2, true), (2, false)];
    for bands_param in [1, 2] {
        let band_count = bands_param + 1;
        for rate in [44_100, 48_000, 96_000, 192_000] {
            for (channels, linked) in chan_configs {
                for width in [0, 3, 8] {
                    for click_w in [1, 2, 3] {
                        for crossover_hz in [80.0, 4000.0, 12_000.0] {
                            let tag = format!(
                                "bands={band_count} rate={rate} ch={channels} linked={linked} width={width} clickw={click_w} xover={crossover_hz}"
                            );
                            let mut clean = vec![0.0; frames * channels];
                            let mut corrupted = vec![0.0; frames * channels];
                            let mut is_click = vec![vec![false; frames]; channels];
                            for ch in 0..channels {
                                let tone = sine(frames, freqs[ch], rate as f32, 0.25);
                                for frame in 0..frames {
                                    clean[frame * channels + ch] = tone[frame];
                                    corrupted[frame * channels + ch] = tone[frame];
                                }
                            }
                            // ch0 carries the W-wide EOF click; ch1 (if any)
                            // stays clean to expose link coupling.
                            for offset in 0..click_w {
                                let at = frames - click_w + offset;
                                corrupted[at * channels] += CLICK_AMP;
                                is_click[0][at] = true;
                            }
                            let template = DeclickPluginParams {
                                mode: 0,
                                bands: bands_param,
                                repair_width: width,
                                link_channels: linked,
                                sensitivity: 2.0,
                                crossover_hz,
                                ..Default::default()
                            };
                            let mut plugin =
                                DeclickPlugin::from_params(channels, rate, template.clone())
                                    .unwrap();
                            let latency = plugin.latency_samples();
                            assert_eq!(latency, FULL_CONTEXT + width, "{tag}: latency");
                            let repaired = run_drained(&mut plugin, &corrupted, channels, rate);
                            let mut clean_plugin =
                                DeclickPlugin::from_params(channels, rate, template.clone())
                                    .unwrap();
                            let clean_out = run_drained(&mut clean_plugin, &clean, channels, rate);
                            let mut residual_plugin = DeclickPlugin::from_params(
                                channels,
                                rate,
                                DeclickPluginParams {
                                    audition_residual: true,
                                    ..template
                                },
                            )
                            .unwrap();
                            let residual =
                                run_drained(&mut residual_plugin, &corrupted, channels, rate);
                            for (label, output) in [
                                ("repaired", &repaired),
                                ("clean", &clean_out),
                                ("residual", &residual),
                            ] {
                                assert_eq!(
                                    output.len(),
                                    (frames + latency) * channels,
                                    "{tag} {label}: length"
                                );
                                assert!(
                                    output.iter().all(|sample| sample.is_finite()),
                                    "{tag} {label}: finite"
                                );
                            }
                            for (index, sample) in
                                repaired.iter().take(latency * channels).enumerate()
                            {
                                assert_eq!(*sample, 0.0, "{tag}: leading silence {index}");
                            }
                            let guard = width + 2;
                            let mut worst_diff_repair = 0.0_f32;
                            let mut worst_direct_repair = 0.0_f32;
                            let mut worst_clean_at_click = 0.0_f32;
                            let mut worst_diff_damage = 0.0_f32;
                            let mut worst_indep_damage = 0.0_f32;
                            let mut worst_direct_damage = 0.0_f32;
                            for input in 32..frames {
                                for ch in 0..channels {
                                    let out_idx = (input + latency) * channels + ch;
                                    let actual = repaired[out_idx];
                                    let clean_ref = clean_out[out_idx];
                                    let analytical = clean[input * channels + ch];
                                    if is_click[ch][input] {
                                        let spike = corrupted[input * channels + ch];
                                        assert!(
                                            (actual - spike).abs() > 1.0,
                                            "{tag}: ch{ch} input={input} spike passed through"
                                        );
                                        let diff = (actual - clean_ref).abs();
                                        worst_diff_repair = worst_diff_repair.max(diff);
                                        assert!(
                                            diff < 0.15,
                                            "{tag}: differential ch{ch} input={input} error={diff}"
                                        );
                                        let direct = (actual - analytical).abs();
                                        worst_direct_repair = worst_direct_repair.max(direct);
                                        assert!(
                                            direct < 0.15,
                                            "{tag}: direct ch{ch} input={input} error={direct}"
                                        );
                                        let clean_err = (clean_ref - analytical).abs();
                                        worst_clean_at_click = worst_clean_at_click.max(clean_err);
                                        assert!(
                                            clean_err < 0.05,
                                            "{tag}: clean-at-click ch{ch} input={input} error={clean_err}"
                                        );
                                    } else {
                                        let start = input.saturating_sub(guard);
                                        let end = (input + guard + 1).min(frames);
                                        if is_click[ch][start..end].contains(&true) {
                                            continue;
                                        }
                                        // Link-coupled clean frames (partner
                                        // clicks at the same frame while
                                        // linked) are intentional pair
                                        // repairs, so the repair bound
                                        // applies to differential/direct;
                                        // the clean render has no coupling,
                                        // so independent keeps 0.05.
                                        let coupled =
                                            linked && channels > 1 && is_click[1 - ch][input];
                                        let pair_bound = if coupled { 0.15 } else { 0.05 };
                                        let diff = (actual - clean_ref).abs();
                                        worst_diff_damage = worst_diff_damage.max(diff);
                                        assert!(
                                            diff < pair_bound,
                                            "{tag}: differential damage ch{ch} input={input} coupled={coupled} error={diff}"
                                        );
                                        let indep = (clean_ref - analytical).abs();
                                        worst_indep_damage = worst_indep_damage.max(indep);
                                        assert!(
                                            indep < 0.05,
                                            "{tag}: independent damage ch{ch} input={input} error={indep}"
                                        );
                                        let direct = (actual - analytical).abs();
                                        worst_direct_damage = worst_direct_damage.max(direct);
                                        assert!(
                                            direct < pair_bound,
                                            "{tag}: direct damage ch{ch} input={input} coupled={coupled} error={direct}"
                                        );
                                    }
                                }
                            }
                            report(&format!("{tag} diff-repair"), worst_diff_repair);
                            report(&format!("{tag} direct-repair"), worst_direct_repair);
                            report(&format!("{tag} clean-at-click"), worst_clean_at_click);
                            report(&format!("{tag} diff-damage"), worst_diff_damage);
                            report(&format!("{tag} indep-damage"), worst_indep_damage);
                            report(&format!("{tag} direct-damage"), worst_direct_damage);
                            // Precision/recall over loud EOF clicks (hot>0.5,
                            // no exclusion) plus regroup vs delayed corrupted.
                            let mut true_hot = 0;
                            let mut false_hot = 0;
                            let mut missed = 0;
                            for input in 32..frames {
                                for ch in 0..channels {
                                    let hot = residual[(input + latency) * channels + ch].abs()
                                        > HOT_RESIDUAL;
                                    match (is_click[ch][input], hot) {
                                        (true, true) => true_hot += 1,
                                        (false, true) => false_hot += 1,
                                        (true, false) => missed += 1,
                                        (false, false) => {}
                                    }
                                }
                            }
                            let total = true_hot + missed;
                            let recall = true_hot as f32 / total as f32;
                            let precision = true_hot as f32 / (true_hot + false_hot).max(1) as f32;
                            assert!(
                                recall >= 0.95,
                                "{tag}: recall={recall} ({true_hot}/{total})"
                            );
                            assert!(
                                precision >= 0.95,
                                "{tag}: precision={precision} false_hot={false_hot}"
                            );
                            let mut dry = vec![0.0; (frames + latency) * channels];
                            for input in 0..frames {
                                for ch in 0..channels {
                                    dry[(input + latency) * channels + ch] =
                                        corrupted[input * channels + ch];
                                }
                            }
                            let mut worst_regroup = 0.0_f32;
                            for i in 0..dry.len() {
                                worst_regroup =
                                    worst_regroup.max((repaired[i] + residual[i] - dry[i]).abs());
                            }
                            assert!(
                                worst_regroup < 1.0e-5,
                                "{tag}: regroup drift {worst_regroup}"
                            );
                            report(&format!("{tag} regroup"), worst_regroup);
                        }
                    }
                }
            }
        }
    }
}

/// Shared zero-continuation endpoint cell oracle (R15).
///
/// Same estimands, frozen bounds, and geometry as
/// `a3_multiband_eof_endpoint_error_oracle` and the frozen FFI
/// `check_eof_error`: (D) differential repair <0.15 at clicks, (I)
/// independent clean <0.05 outside the footprint, (E) end-to-end
/// <0.15/<0.05, removal >1.0, PR >=0.95 over 32..frames with no
/// exclusion, regroup <1e-5, exact length/finite/leading silence.
/// Serves the R15 skew/phase/replica legs so the original oracle body
/// stays byte-identical.
#[allow(
    clippy::too_many_arguments,
    reason = "test oracle: one argument per fixture stream and geometry bound"
)]
fn check_eof_cell(
    tag: &str,
    repaired: &[f32],
    clean_out: &[f32],
    residual: &[f32],
    clean: &[f32],
    corrupted: &[f32],
    is_click: &[Vec<bool>],
    channels: usize,
    frames: usize,
    latency: usize,
    width: usize,
    linked: bool,
) {
    for (label, output) in [
        ("repaired", repaired),
        ("clean", clean_out),
        ("residual", residual),
    ] {
        assert_eq!(
            output.len(),
            (frames + latency) * channels,
            "{tag} {label}: length"
        );
        assert!(
            output.iter().all(|sample| sample.is_finite()),
            "{tag} {label}: finite"
        );
    }
    for (index, sample) in repaired.iter().take(latency * channels).enumerate() {
        assert_eq!(*sample, 0.0, "{tag}: leading silence {index}");
    }
    let guard = width + 2;
    let mut worst_diff_repair = 0.0_f32;
    let mut worst_direct_repair = 0.0_f32;
    let mut worst_clean_at_click = 0.0_f32;
    let mut worst_diff_damage = 0.0_f32;
    let mut worst_indep_damage = 0.0_f32;
    let mut worst_direct_damage = 0.0_f32;
    for input in 32..frames {
        for ch in 0..channels {
            let out_idx = (input + latency) * channels + ch;
            let actual = repaired[out_idx];
            let clean_ref = clean_out[out_idx];
            let analytical = clean[input * channels + ch];
            if is_click[ch][input] {
                let spike = corrupted[input * channels + ch];
                assert!(
                    (actual - spike).abs() > 1.0,
                    "{tag}: ch{ch} input={input} spike passed through"
                );
                let diff = (actual - clean_ref).abs();
                worst_diff_repair = worst_diff_repair.max(diff);
                assert!(
                    diff < 0.15,
                    "{tag}: differential ch{ch} input={input} error={diff}"
                );
                let direct = (actual - analytical).abs();
                worst_direct_repair = worst_direct_repair.max(direct);
                assert!(
                    direct < 0.15,
                    "{tag}: direct ch{ch} input={input} error={direct}"
                );
                let clean_err = (clean_ref - analytical).abs();
                worst_clean_at_click = worst_clean_at_click.max(clean_err);
                assert!(
                    clean_err < 0.05,
                    "{tag}: clean-at-click ch{ch} input={input} error={clean_err}"
                );
            } else {
                let start = input.saturating_sub(guard);
                let end = (input + guard + 1).min(frames);
                if is_click[ch][start..end].contains(&true) {
                    continue;
                }
                // Link-coupled clean frames (partner clicks at the same
                // frame while linked) are intentional pair repairs: repair
                // bound for differential/direct, 0.05 for independent
                // (clean render has no coupling).
                let coupled = linked && channels > 1 && is_click[1 - ch][input];
                let pair_bound = if coupled { 0.15 } else { 0.05 };
                let diff = (actual - clean_ref).abs();
                worst_diff_damage = worst_diff_damage.max(diff);
                assert!(
                    diff < pair_bound,
                    "{tag}: differential damage ch{ch} input={input} coupled={coupled} error={diff}"
                );
                let indep = (clean_ref - analytical).abs();
                worst_indep_damage = worst_indep_damage.max(indep);
                assert!(
                    indep < 0.05,
                    "{tag}: independent damage ch{ch} input={input} error={indep}"
                );
                let direct = (actual - analytical).abs();
                worst_direct_damage = worst_direct_damage.max(direct);
                assert!(
                    direct < pair_bound,
                    "{tag}: direct damage ch{ch} input={input} coupled={coupled} error={direct}"
                );
            }
        }
    }
    report(&format!("{tag} diff-repair"), worst_diff_repair);
    report(&format!("{tag} direct-repair"), worst_direct_repair);
    report(&format!("{tag} clean-at-click"), worst_clean_at_click);
    report(&format!("{tag} diff-damage"), worst_diff_damage);
    report(&format!("{tag} indep-damage"), worst_indep_damage);
    report(&format!("{tag} direct-damage"), worst_direct_damage);
    let mut true_hot = 0;
    let mut false_hot = 0;
    let mut missed = 0;
    for input in 32..frames {
        for ch in 0..channels {
            let hot = residual[(input + latency) * channels + ch].abs() > HOT_RESIDUAL;
            match (is_click[ch][input], hot) {
                (true, true) => true_hot += 1,
                (false, true) => false_hot += 1,
                (true, false) => missed += 1,
                (false, false) => {}
            }
        }
    }
    let total = true_hot + missed;
    let recall = true_hot as f32 / total as f32;
    let precision = true_hot as f32 / (true_hot + false_hot).max(1) as f32;
    assert!(
        recall >= 0.95,
        "{tag}: recall={recall} ({true_hot}/{total})"
    );
    assert!(
        precision >= 0.95,
        "{tag}: precision={precision} false_hot={false_hot}"
    );
    let mut dry = vec![0.0; (frames + latency) * channels];
    for input in 0..frames {
        for ch in 0..channels {
            dry[(input + latency) * channels + ch] = corrupted[input * channels + ch];
        }
    }
    let mut worst_regroup = 0.0_f32;
    for i in 0..dry.len() {
        worst_regroup = worst_regroup.max((repaired[i] + residual[i] - dry[i]).abs());
    }
    assert!(
        worst_regroup < 1.0e-5,
        "{tag}: regroup drift {worst_regroup}"
    );
    report(&format!("{tag} regroup"), worst_regroup);
}

#[test]
fn a3_skew_endpoint_error_oracle() {
    // Skew-extreme endpoint ERROR oracle (R15 B1): the same D/I/E
    // estimands, removal, PR, regroup, and geometry as
    // `a3_multiband_eof_endpoint_error_oracle` via `check_eof_cell`,
    // crossed with both skew extremes. Skew -1 halves low-band
    // thresholds (skew +1 halves high-band thresholds); under EOF
    // drain geometry a linked supervisor confirmation can then join a
    // band on level alone where the skew-0 matrix stays dry (frozen
    // FFI red: skew-neg clean-at-click 0.054 > 0.05 at the last
    // frame). Matrix: both multiband topologies x 4 rates x
    // mono/stereo-linked/stereo-split x widths 0/3/8 x 1/2/3-wide EOF
    // clicks x crossovers 80/4k/12kHz x skew -1/+1. Original bounds
    // only, no exclusion.
    let frames = 256;
    let freqs = [440.0, 660.0];
    let chan_configs = [(1, true), (2, true), (2, false)];
    for bands_param in [1, 2] {
        let band_count = bands_param + 1;
        for rate in [44_100, 48_000, 96_000, 192_000] {
            for (channels, linked) in chan_configs {
                for width in [0, 3, 8] {
                    for click_w in [1, 2, 3] {
                        for crossover_hz in [80.0, 4000.0, 12_000.0] {
                            for skew in [-1.0, 1.0] {
                                let tag = format!(
                                    "skew bands={band_count} rate={rate} ch={channels} linked={linked} width={width} clickw={click_w} xover={crossover_hz} skew={skew}"
                                );
                                let mut clean = vec![0.0; frames * channels];
                                let mut corrupted = vec![0.0; frames * channels];
                                let mut is_click = vec![vec![false; frames]; channels];
                                for ch in 0..channels {
                                    let tone = sine(frames, freqs[ch], rate as f32, 0.25);
                                    for frame in 0..frames {
                                        clean[frame * channels + ch] = tone[frame];
                                        corrupted[frame * channels + ch] = tone[frame];
                                    }
                                }
                                for offset in 0..click_w {
                                    let at = frames - click_w + offset;
                                    corrupted[at * channels] += CLICK_AMP;
                                    is_click[0][at] = true;
                                }
                                let template = DeclickPluginParams {
                                    mode: 0,
                                    bands: bands_param,
                                    repair_width: width,
                                    link_channels: linked,
                                    sensitivity: 2.0,
                                    crossover_hz,
                                    frequency_skew: skew,
                                    ..Default::default()
                                };
                                let mut plugin =
                                    DeclickPlugin::from_params(channels, rate, template.clone())
                                        .unwrap();
                                let latency = plugin.latency_samples();
                                assert_eq!(latency, FULL_CONTEXT + width, "{tag}: latency");
                                let repaired = run_drained(&mut plugin, &corrupted, channels, rate);
                                let mut clean_plugin =
                                    DeclickPlugin::from_params(channels, rate, template.clone())
                                        .unwrap();
                                let clean_out =
                                    run_drained(&mut clean_plugin, &clean, channels, rate);
                                let mut residual_plugin = DeclickPlugin::from_params(
                                    channels,
                                    rate,
                                    DeclickPluginParams {
                                        audition_residual: true,
                                        ..template
                                    },
                                )
                                .unwrap();
                                let residual =
                                    run_drained(&mut residual_plugin, &corrupted, channels, rate);
                                check_eof_cell(
                                    &tag, &repaired, &clean_out, &residual, &clean, &corrupted,
                                    &is_click, channels, frames, latency, width, linked,
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn a3_skew_eof_frozen_cell_replica_1024() {
    // Exact DSP replicas of the frozen FFI skew spots (R15 B1
    // regression): 1024-frame 440/660Hz x 0.25 tones, 3-wide EOF click
    // at 1021-1023 on ch0, 3-band width 0 4kHz sensitivity 2.0 random
    // mode stereo: skew -1 linked (the red cell), skew +1 linked (never
    // executed -- short-circuited past the red), and skew 0 split
    // (green control proving the join needs the link). Full D/I/E +
    // removal + PR + regroup + geometry via `check_eof_cell`; the
    // skew-neg clean-at-click frame 1023 is the 0.054 regression pin.
    // Skew-pos is asserted, not assumed: predicted green (the
    // sensitized high band carries little tone), but a red exposes a
    // second sign-side mechanism for R16 rather than weakening
    // anything here.
    let frames = 1024;
    let rate = 48_000;
    let channels = 2;
    let freqs = [440.0, 660.0];
    for (name, linked, skew) in [
        ("split", false, 0.0),
        ("skew-neg", true, -1.0),
        ("skew-pos", true, 1.0),
    ] {
        let tag = format!("replica1024 spot={name}");
        let mut clean = vec![0.0; frames * channels];
        let mut corrupted = vec![0.0; frames * channels];
        let mut is_click = vec![vec![false; frames]; channels];
        for ch in 0..channels {
            let tone = sine(frames, freqs[ch], rate as f32, 0.25);
            for frame in 0..frames {
                clean[frame * channels + ch] = tone[frame];
                corrupted[frame * channels + ch] = tone[frame];
            }
        }
        for at in frames - 3..frames {
            corrupted[at * channels] += CLICK_AMP;
            is_click[0][at] = true;
        }
        let template = DeclickPluginParams {
            mode: 0,
            bands: 2,
            repair_width: 0,
            link_channels: linked,
            sensitivity: 2.0,
            crossover_hz: 4000.0,
            frequency_skew: skew,
            ..Default::default()
        };
        let mut plugin = DeclickPlugin::from_params(channels, rate, template.clone()).unwrap();
        let latency = plugin.latency_samples();
        assert_eq!(latency, FULL_CONTEXT, "{tag}: latency");
        let repaired = run_drained(&mut plugin, &corrupted, channels, rate);
        let mut clean_plugin =
            DeclickPlugin::from_params(channels, rate, template.clone()).unwrap();
        let clean_out = run_drained(&mut clean_plugin, &clean, channels, rate);
        let mut residual_plugin = DeclickPlugin::from_params(
            channels,
            rate,
            DeclickPluginParams {
                audition_residual: true,
                ..template
            },
        )
        .unwrap();
        let residual = run_drained(&mut residual_plugin, &corrupted, channels, rate);
        check_eof_cell(
            &tag, &repaired, &clean_out, &residual, &clean, &corrupted, &is_click, channels,
            frames, latency, 0, linked,
        );
    }
}

#[test]
fn a3_eof_phase_sweep_endpoints() {
    // EOF tone-phase sweep (R15 review section 4): the fixed-256 oracle
    // pins one EOF phase per rate/tone while drain geometry interacts
    // with phase (the FFI red sits at 1024 frames, a different phase).
    // Nine stream lengths spanning 112 samples cover a full 440Hz
    // cycle at 48kHz (period 109.09) and 1.5 660Hz cycles (period
    // 72.73), on the 3-wide-EOF slice (both topologies, skew
    // -1/0/+1, stereo linked, width 0, 4kHz, sens 2.0, random). Full
    // cell oracle each via `check_eof_cell`; original bounds only.
    let freqs = [440.0, 660.0];
    let rate = 48_000;
    let channels = 2;
    for frames in [206, 220, 234, 248, 262, 276, 290, 304, 318] {
        for bands_param in [1, 2] {
            let band_count = bands_param + 1;
            for skew in [-1.0, 0.0, 1.0] {
                let tag = format!("phase frames={frames} bands={band_count} skew={skew}");
                let mut clean = vec![0.0; frames * channels];
                let mut corrupted = vec![0.0; frames * channels];
                let mut is_click = vec![vec![false; frames]; channels];
                for ch in 0..channels {
                    let tone = sine(frames, freqs[ch], rate as f32, 0.25);
                    for frame in 0..frames {
                        clean[frame * channels + ch] = tone[frame];
                        corrupted[frame * channels + ch] = tone[frame];
                    }
                }
                for at in frames - 3..frames {
                    corrupted[at * channels] += CLICK_AMP;
                    is_click[0][at] = true;
                }
                let template = DeclickPluginParams {
                    mode: 0,
                    bands: bands_param,
                    repair_width: 0,
                    link_channels: true,
                    sensitivity: 2.0,
                    crossover_hz: 4000.0,
                    frequency_skew: skew,
                    ..Default::default()
                };
                let mut plugin =
                    DeclickPlugin::from_params(channels, rate, template.clone()).unwrap();
                let latency = plugin.latency_samples();
                assert_eq!(latency, FULL_CONTEXT, "{tag}: latency");
                let repaired = run_drained(&mut plugin, &corrupted, channels, rate);
                let mut clean_plugin =
                    DeclickPlugin::from_params(channels, rate, template.clone()).unwrap();
                let clean_out = run_drained(&mut clean_plugin, &clean, channels, rate);
                let mut residual_plugin = DeclickPlugin::from_params(
                    channels,
                    rate,
                    DeclickPluginParams {
                        audition_residual: true,
                        ..template
                    },
                )
                .unwrap();
                let residual = run_drained(&mut residual_plugin, &corrupted, channels, rate);
                check_eof_cell(
                    &tag, &repaired, &clean_out, &residual, &clean, &corrupted, &is_click,
                    channels, frames, latency, 0, true,
                );
            }
        }
    }
}

#[test]
fn a3_multiband_periodic_eof_locks_and_repairs() {
    // Locked periodic tracker at multiband EOF (DECLICK-R1/A1/A3): long
    // streams (1441 frames, last frame 1440 on-phase for the 100-sample
    // grid from 140) so the 1024-sample tracker window recomputes and locks.
    // The lock is proven behaviorally by the quiet-probe split below, never
    // assumed from length alone: the on-phase probe at 1240 (sensitive
    // x0.25) must repair (<0.15 error) while the off-phase probe at 1290
    // (guard x4) must miss (>0.3 error, i.e. stays dry with the click).
    // That split is impossible unless locked, because the unlocked gate is
    // identically 1.0 (both probes would behave identically). The grid omits
    // 1240 (filled by the quiet probe alone) so the split cannot come from
    // loudness. Direct
    // tracker-state assertion (supervisor + every band reporting period
    // 100) lives in the repair.rs unit test
    // `periodic_tracker_locks_grid_on_owned_engine_bands_and_supervisor`,
    // since cfg(test) hooks are not linkable from integration tests. Loud
    // 3-wide EOF clicks at 1438-1440 (mixed off/off/on phases) must repair
    // (<0.15 differential + direct) under the lock with uniform mixed
    // windows and the active period gate (R3 reverted the R2 pre-only
    // experiment; no EOF trigger exists). Loud clicks exceed both sensitive
    // and guard thresholds, so all three repair regardless of phase; every
    // clean on-phase frame lies inside a loud/quiet footprint, so sensitive
    // predictions have no clean on-phase frame outside the footprint to
    // false-trigger, while off-phase clean stays dry under the guard.
    // Clean vs analytical (<0.05) is the independent false-positive measure
    // on all clean frames outside the footprint, including boundary clean;
    // PR (>=0.95, hot>0.5) counts loud clicks only (quiet probes are gate
    // probes with sub-hot residuals, a separate estimand, not PR inputs).
    // Matrix: both multiband topologies x 3 rates x mono/stereo-linked/
    // stereo-split x crossovers 80/4k/12kHz at width 0, plus width 3/8 spot
    // checks (48k, 3-band, stereo linked, 4kHz) and R15 width-8 spots at
    // (96k, 4kHz) and (48k, 80Hz) on the same topology; widths are fully
    // covered in the random error oracle and lock formation is
    // width-independent (raw triggers feed the tracker before widening).
    let frames = 1441;
    let freqs = [440.0, 660.0];
    let chan_configs = [(1, true), (2, true), (2, false)];
    // (rate, crossover, widths): width-0 across the full matrix, plus
    // 3/8 spot checks on the documented narrow config and R15 width-8
    // periodic boundary spots at the thin rate (96kHz) and the
    // tail-heavy extreme crossover (80Hz), all guarded in the loop.
    let mut width_sets: Vec<(u32, f32, Vec<usize>)> = Vec::new();
    for rate in [44_100, 48_000, 96_000] {
        for crossover_hz in [80.0, 4000.0, 12_000.0] {
            width_sets.push((rate, crossover_hz, vec![0]));
        }
    }
    width_sets.push((48_000, 4000.0, vec![3, 8]));
    width_sets.push((96_000, 4000.0, vec![8]));
    width_sets.push((48_000, 80.0, vec![8]));
    for bands_param in [1, 2] {
        let band_count = bands_param + 1;
        for (channels, linked) in chan_configs {
            for (rate, crossover_hz, widths) in &width_sets {
                let (rate, crossover_hz) = (*rate, *crossover_hz);
                for width in widths {
                    let width = *width;
                    // Spot widths (3/8) exist only on the (48k, 4kHz)
                    // entry; run them solely on 3-band stereo linked.
                    if width != 0 && !(bands_param == 2 && channels == 2 && linked) {
                        continue;
                    }
                    let tag = format!(
                        "bands={band_count} rate={rate} ch={channels} linked={linked} width={width} xover={crossover_hz}"
                    );
                    // Grids + probes + EOF clicks (ch0 carries clicks; ch1,
                    // if any, stays clean to expose link coupling).
                    let quiet_on = 1240;
                    let quiet_off = 1290;
                    let mut grid = Vec::new();
                    let mut start = 140;
                    while start < 1341 {
                        if start != quiet_on {
                            grid.push(start);
                        }
                        start += 100;
                    }
                    let eof = [frames - 3, frames - 2, frames - 1];
                    let mut clean = vec![0.0; frames * channels];
                    let mut corrupted = vec![0.0; frames * channels];
                    let mut is_loud = vec![vec![false; frames]; channels];
                    for ch in 0..channels {
                        let tone = sine(frames, freqs[ch], rate as f32, 0.25);
                        for frame in 0..frames {
                            clean[frame * channels + ch] = tone[frame];
                            corrupted[frame * channels + ch] = tone[frame];
                        }
                    }
                    for &at in &grid {
                        corrupted[at * channels] += CLICK_AMP;
                        is_loud[0][at] = true;
                    }
                    for &at in &eof {
                        corrupted[at * channels] += CLICK_AMP;
                        is_loud[0][at] = true;
                    }
                    corrupted[quiet_on * channels] += 0.5;
                    corrupted[quiet_off * channels] += 0.5;
                    let template = DeclickPluginParams {
                        mode: 1,
                        bands: bands_param,
                        repair_width: width,
                        link_channels: linked,
                        sensitivity: 5.0,
                        crossover_hz,
                        ..Default::default()
                    };
                    let mut plugin =
                        DeclickPlugin::from_params(channels, rate, template.clone()).unwrap();
                    let latency = plugin.latency_samples();
                    assert_eq!(latency, FULL_CONTEXT + width, "{tag}: latency");
                    let repaired = run_drained(&mut plugin, &corrupted, channels, rate);
                    let mut clean_plugin =
                        DeclickPlugin::from_params(channels, rate, template.clone()).unwrap();
                    let clean_out = run_drained(&mut clean_plugin, &clean, channels, rate);
                    let mut residual_plugin = DeclickPlugin::from_params(
                        channels,
                        rate,
                        DeclickPluginParams {
                            audition_residual: true,
                            ..template
                        },
                    )
                    .unwrap();
                    let residual = run_drained(&mut residual_plugin, &corrupted, channels, rate);
                    assert_eq!(
                        repaired.len(),
                        (frames + latency) * channels,
                        "{tag}: length"
                    );
                    assert!(repaired.iter().all(|s| s.is_finite()), "{tag}: finite");
                    // Gate proof on interior quiet probes (full context).
                    let on_err = (repaired[(quiet_on + latency) * channels]
                        - clean[quiet_on * channels])
                        .abs();
                    assert!(
                        on_err < 0.15,
                        "{tag}: quiet on-phase must repair, error={on_err}"
                    );
                    let off_err = (repaired[(quiet_off + latency) * channels]
                        - clean[quiet_off * channels])
                        .abs();
                    assert!(
                        off_err > 0.3,
                        "{tag}: quiet off-phase must miss (guard), error={off_err}"
                    );
                    report(&format!("{tag} quiet-on"), on_err);
                    report(&format!("{tag} quiet-off"), off_err);
                    // EOF loud-click error (differential + direct + clean)
                    // with lock.
                    let mut worst_diff = 0.0_f32;
                    let mut worst_direct = 0.0_f32;
                    let mut worst_eof_clean = 0.0_f32;
                    for &at in &eof {
                        let out_idx = (at + latency) * channels;
                        let diff = (repaired[out_idx] - clean_out[out_idx]).abs();
                        worst_diff = worst_diff.max(diff);
                        assert!(diff < 0.15, "{tag}: EOF differential at={at} error={diff}");
                        let direct = (repaired[out_idx] - clean[at * channels]).abs();
                        worst_direct = worst_direct.max(direct);
                        assert!(direct < 0.15, "{tag}: EOF direct at={at} error={direct}");
                        let clean_err = (clean_out[out_idx] - clean[at * channels]).abs();
                        worst_eof_clean = worst_eof_clean.max(clean_err);
                        assert!(
                            clean_err < 0.05,
                            "{tag}: EOF clean-at-click at={at} error={clean_err}"
                        );
                    }
                    report(&format!("{tag} eof-diff"), worst_diff);
                    report(&format!("{tag} eof-direct"), worst_direct);
                    report(&format!("{tag} eof-clean"), worst_eof_clean);
                    // Independent clean damage on all clean frames outside
                    // the footprint (grid + EOF loud + quiet probes all open
                    // footprints; quiet probes use the same guard width).
                    let guard = width + 2;
                    let mut loud_or_probe = is_loud.clone();
                    loud_or_probe[0][quiet_on] = true;
                    loud_or_probe[0][quiet_off] = true;
                    let mut worst_diff_clean = 0.0_f32;
                    let mut worst_indep = 0.0_f32;
                    let mut worst_direct_clean = 0.0_f32;
                    for input in 32..frames {
                        for ch in 0..channels {
                            if is_loud[ch][input] {
                                continue;
                            }
                            // Quiet probe frames are gate probes, not damage
                            // frames (the off-phase probe intentionally keeps
                            // its click); skip only those two frames.
                            if ch == 0 && (input == quiet_on || input == quiet_off) {
                                continue;
                            }
                            let start = input.saturating_sub(guard);
                            let end = (input + guard + 1).min(frames);
                            if loud_or_probe[ch][start..end].contains(&true) {
                                continue;
                            }
                            // Link-coupled clean frames (partner loud/probe
                            // at the same frame while linked) are intentional
                            // pair repairs: repair bound for
                            // differential/direct, 0.05 for independent
                            // (clean render has no coupling).
                            let coupled = linked && channels > 1 && loud_or_probe[1 - ch][input];
                            let pair_bound = if coupled { 0.15 } else { 0.05 };
                            let out_idx = (input + latency) * channels + ch;
                            let diff = (repaired[out_idx] - clean_out[out_idx]).abs();
                            worst_diff_clean = worst_diff_clean.max(diff);
                            assert!(
                                diff < pair_bound,
                                "{tag}: diff damage ch{ch} input={input} coupled={coupled} error={diff}"
                            );
                            let indep = (clean_out[out_idx] - clean[input * channels + ch]).abs();
                            worst_indep = worst_indep.max(indep);
                            assert!(
                                indep < 0.05,
                                "{tag}: indep damage ch{ch} input={input} error={indep}"
                            );
                            let direct = (repaired[out_idx] - clean[input * channels + ch]).abs();
                            worst_direct_clean = worst_direct_clean.max(direct);
                            assert!(
                                direct < pair_bound,
                                "{tag}: direct damage ch{ch} input={input} coupled={coupled} error={direct}"
                            );
                        }
                    }
                    report(&format!("{tag} diff-clean"), worst_diff_clean);
                    report(&format!("{tag} indep-damage"), worst_indep);
                    report(&format!("{tag} direct-clean"), worst_direct_clean);
                    // PR over loud clicks only (quiet probes excluded as gate
                    // probes with sub-hot residuals by design) + regroup.
                    let mut true_hot = 0;
                    let mut false_hot = 0;
                    let mut missed = 0;
                    for input in 32..frames {
                        for ch in 0..channels {
                            if ch == 0 && (input == quiet_on || input == quiet_off) {
                                continue;
                            }
                            let hot =
                                residual[(input + latency) * channels + ch].abs() > HOT_RESIDUAL;
                            match (is_loud[ch][input], hot) {
                                (true, true) => true_hot += 1,
                                (false, true) => false_hot += 1,
                                (true, false) => missed += 1,
                                (false, false) => {}
                            }
                        }
                    }
                    let total = true_hot + missed;
                    let recall = true_hot as f32 / total as f32;
                    let precision = true_hot as f32 / (true_hot + false_hot).max(1) as f32;
                    assert!(
                        recall >= 0.95,
                        "{tag}: recall={recall} ({true_hot}/{total})"
                    );
                    assert!(
                        precision >= 0.95,
                        "{tag}: precision={precision} false_hot={false_hot}"
                    );
                    let mut dry = vec![0.0; (frames + latency) * channels];
                    for input in 0..frames {
                        for ch in 0..channels {
                            dry[(input + latency) * channels + ch] =
                                corrupted[input * channels + ch];
                        }
                    }
                    let mut worst_regroup = 0.0_f32;
                    for i in 0..dry.len() {
                        worst_regroup =
                            worst_regroup.max((repaired[i] + residual[i] - dry[i]).abs());
                    }
                    assert!(
                        worst_regroup < 1.0e-5,
                        "{tag}: regroup drift {worst_regroup}"
                    );
                    report(&format!("{tag} regroup"), worst_regroup);
                }
            }
        }
    }
}

#[test]
fn a1_periodic_clean_tail_locked_no_damage() {
    // Independent clean reference on arbitrary-phase locked-periodic clean
    // frames outside click footprints (R4 closes the coverage gap the R3
    // report admitted: fixture grids placed loud clicks on every on-phase
    // frame, so clean on-phase frames under a live lock were unexercised).
    // A full 13-click periodic grid (100-sample period) locks the tracker
    // and a clean tail (1341..1700, no clicks) then covers every nominal
    // phase against the last grid click several times over. The tail ends
    // 359 frames after the last grid click, inside the 400-frame staleness
    // limit, so the lock stays active over every asserted frame; the
    // companion unit test `periodic_clean_tail_locks_stay_active_over_tail`
    // proves the supervisor/top-band lock state white-box at both tail
    // ends with these exact params (integration cannot query trackers).
    // Every tail frame from 1343 on sits outside all footprints (grid ± 2
    // ends at 1342) and must match the analytical tone within the original
    // 0.05 clean damage bound. Matrix: both multiband topologies × 3
    // rates, mono, width 0, 4kHz crossover, periodic mode, sensitivity 5.0.
    let frames = 1700;
    for bands_param in [1, 2] {
        let band_count = bands_param + 1;
        for rate in [44_100, 48_000, 96_000] {
            let tag = format!("bands={band_count} rate={rate} ch=1 linked=true width=0 xover=4000");
            let mut grid = Vec::new();
            let mut start = 140;
            while start < 1341 {
                grid.push(start);
                start += 100;
            }
            let tone = sine(frames, 440.0, rate as f32, 0.25);
            let mut corrupted = tone.clone();
            for &at in &grid {
                corrupted[at] += CLICK_AMP;
            }
            let template = DeclickPluginParams {
                mode: 1,
                bands: bands_param,
                repair_width: 0,
                link_channels: true,
                sensitivity: 5.0,
                crossover_hz: 4000.0,
                ..Default::default()
            };
            let mut plugin = DeclickPlugin::from_params(1, rate, template).unwrap();
            let latency = plugin.latency_samples();
            assert_eq!(latency, FULL_CONTEXT, "{tag}: latency");
            let repaired = run_drained(&mut plugin, &corrupted, 1, rate);
            assert_eq!(repaired.len(), frames + latency, "{tag}: length");
            assert!(repaired.iter().all(|s| s.is_finite()), "{tag}: finite");
            let mut worst = 0.0_f32;
            let mut worst_on = 0.0_f32;
            let mut worst_off = 0.0_f32;
            for input in 1343..frames {
                let error = (repaired[input + latency] - tone[input]).abs();
                worst = worst.max(error);
                if (input - 1340) % 100 == 0 {
                    worst_on = worst_on.max(error);
                } else {
                    worst_off = worst_off.max(error);
                }
                assert!(error < 0.05, "{tag}: tail ch0 input={input} error={error}");
            }
            report(&format!("{tag} tail"), worst);
            report(&format!("{tag} tail-on"), worst_on);
            report(&format!("{tag} tail-off"), worst_off);
        }
    }
}

#[test]
fn a1_periodic_quiet_probes_repair_on_phase_only() {
    // Quiet gate probes with the original R4 fixture/matrix/bounds (R6
    // restores assertion-bearing coverage: record-only diagnostics are not
    // equivalent). A periodic grid (12 loud clicks, 100-sample period,
    // gap at 1240) plus two 0.5 quiet probes locks the tracker; the
    // on-phase probe at 1240 (sensitive x0.25) must repair (<0.15 error)
    // while the off-phase probe at 1290 (guard x4) must miss (>0.3 error).
    // That split is impossible unless locked (unlocked gate is identically
    // 1.0). The 560-frame clean tail (1341..1900) keeps the original
    // independent clean reference (<0.05 vs analytical outside every
    // footprint, reported split by nominal phase). Matrix: both multiband
    // topologies x 3 rates x mono/stereo-linked, width 0, 4kHz crossover,
    // periodic mode, sensitivity 5.0. Stereo-linked pairs carry clicks on
    // ch0 only; ch1 (660Hz, fully clean) additionally asserts <0.05
    // independent damage outside the ch0 footprints, so link coupling can
    // never damage clean channels away from clicks.
    let frames = 1900;
    let freqs = [440.0, 660.0];
    for bands_param in [1, 2] {
        let band_count = bands_param + 1;
        for rate in [44_100, 48_000, 96_000] {
            for channels in [1, 2] {
                let tag = format!(
                    "bands={band_count} rate={rate} ch={channels} linked=true width=0 xover=4000"
                );
                let quiet_on = 1240;
                let quiet_off = 1290;
                let mut grid = Vec::new();
                let mut start = 140;
                while start < 1341 {
                    if start != quiet_on {
                        grid.push(start);
                    }
                    start += 100;
                }
                let mut clean = vec![0.0; frames * channels];
                let mut corrupted = vec![0.0; frames * channels];
                for ch in 0..channels {
                    let tone = sine(frames, freqs[ch], rate as f32, 0.25);
                    for frame in 0..frames {
                        clean[frame * channels + ch] = tone[frame];
                        corrupted[frame * channels + ch] = tone[frame];
                    }
                }
                for &at in &grid {
                    corrupted[at * channels] += CLICK_AMP;
                }
                corrupted[quiet_on * channels] += 0.5;
                corrupted[quiet_off * channels] += 0.5;
                let template = DeclickPluginParams {
                    mode: 1,
                    bands: bands_param,
                    repair_width: 0,
                    link_channels: true,
                    sensitivity: 5.0,
                    crossover_hz: 4000.0,
                    ..Default::default()
                };
                let mut plugin = DeclickPlugin::from_params(channels, rate, template).unwrap();
                let latency = plugin.latency_samples();
                assert_eq!(latency, FULL_CONTEXT, "{tag}: latency");
                let repaired = run_drained(&mut plugin, &corrupted, channels, rate);
                assert_eq!(
                    repaired.len(),
                    (frames + latency) * channels,
                    "{tag}: length"
                );
                assert!(repaired.iter().all(|s| s.is_finite()), "{tag}: finite");
                let on_err =
                    (repaired[(quiet_on + latency) * channels] - clean[quiet_on * channels]).abs();
                assert!(
                    on_err < 0.15,
                    "{tag}: quiet on-phase must repair, error={on_err}"
                );
                let off_err = (repaired[(quiet_off + latency) * channels]
                    - clean[quiet_off * channels])
                    .abs();
                assert!(
                    off_err > 0.3,
                    "{tag}: quiet off-phase must miss (guard), error={off_err}"
                );
                report(&format!("{tag} quiet-on"), on_err);
                report(&format!("{tag} quiet-off"), off_err);
                let mut worst = 0.0_f32;
                let mut worst_on = 0.0_f32;
                let mut worst_off = 0.0_f32;
                for input in 1343..frames {
                    let error =
                        (repaired[(input + latency) * channels] - clean[input * channels]).abs();
                    worst = worst.max(error);
                    if (input - 1340) % 100 == 0 {
                        worst_on = worst_on.max(error);
                    } else {
                        worst_off = worst_off.max(error);
                    }
                    assert!(error < 0.05, "{tag}: tail ch0 input={input} error={error}");
                }
                report(&format!("{tag} tail"), worst);
                report(&format!("{tag} tail-on"), worst_on);
                report(&format!("{tag} tail-off"), worst_off);
                if channels == 2 {
                    // ch0 footprints (grid + probes, guard width 2 at
                    // width 0); ch1 is fully clean and must stay within
                    // 0.05 everywhere outside them.
                    let mut hot = vec![false; frames];
                    for &at in &grid {
                        let end = (at + 2).min(frames - 1);
                        hot[at.saturating_sub(2)..=end].fill(true);
                    }
                    for &at in &[quiet_on, quiet_off] {
                        let end = (at + 2).min(frames - 1);
                        hot[at.saturating_sub(2)..=end].fill(true);
                    }
                    let mut worst_ch1 = 0.0_f32;
                    for input in 32..frames {
                        if hot[input] {
                            continue;
                        }
                        let error = (repaired[(input + latency) * channels + 1]
                            - clean[input * channels + 1])
                            .abs();
                        worst_ch1 = worst_ch1.max(error);
                        assert!(
                            error < 0.05,
                            "{tag}: linked clean ch1 input={input} error={error}"
                        );
                    }
                    report(&format!("{tag} ch1-clean"), worst_ch1);
                }
            }
        }
    }
}

#[test]
fn a1_endpoint_controls_zero_dc_step_impulse_silence() {
    // Signal-type controls for the uniform mixed-window endpoint (no EOF
    // trigger exists after the R3 revert, so interior zeros cannot
    // false-trigger endpoint logic — vacuously safe — and this leg proves
    // the behavior directly against independent analytical references with
    // unchanged criteria). Five controls, all through exact drain to EOF:
    // DC constant (dry, 1e-5), DC step 0.2->0.5 (bridge vetoes, dry, 1e-5),
    // tone-to-silence transition with 128 trailing exact zeros ending at
    // EOF (dry, 1e-5; steps vetoed, runs exact), interior exact zeros at
    // zero crossings (dry, 1e-5; baselines near zero so nothing triggers),
    // and 1.0 interior impulses (click-like short excursions per the R1
    // detector definition, repaired <0.15 with <0.05 damage outside +-2).
    // Isolated peak zeros would likewise repair as dropouts (short large
    // excursions are clicks regardless of cause); preserving sparse peak
    // zeros would be a new requirement outside A1/R1 and is not asserted.
    // Matrix: both multiband topologies x mono/stereo-linked at 48k width 0
    // 4kHz for all controls, plus 44.1/96k DC and silence-transition spots
    // (3-band stereo). Stereo is dual-mono (same signal both channels).
    let frames = 256;
    let controls = [
        "dc",
        "dc_step",
        "silence_transition",
        "interior_zeros",
        "impulse",
    ];
    for bands_param in [1, 2] {
        let band_count = bands_param + 1;
        for channels in [1, 2] {
            for control in controls {
                let tag = format!("bands={band_count} ch={channels} control={control} rate=48000");
                run_control_case(
                    &tag,
                    bands_param,
                    channels,
                    48_000,
                    4000.0,
                    0,
                    control,
                    frames,
                );
            }
        }
    }
    for rate in [44_100, 96_000] {
        for control in ["dc", "silence_transition"] {
            let tag = format!("bands=3 ch=2 control={control} rate={rate} spot");
            run_control_case(&tag, 2, 2, rate, 4000.0, 0, control, frames);
        }
    }
}

#[test]
fn a2_band_reconstruction_is_exact_when_bypassed() {
    for rate in [44_100, 48_000, 96_000] {
        for channels in [1, 2, 4] {
            for bands in [0, 1, 2] {
                for crossover_hz in [80.0, 4000.0, 12_000.0] {
                    let mut plugin = DeclickPlugin::from_params(
                        channels,
                        rate,
                        DeclickPluginParams {
                            enabled: false,
                            bands,
                            crossover_hz,
                            frequency_skew: 1.0,
                            ..Default::default()
                        },
                    )
                    .unwrap();
                    let latency = plugin.latency_samples();
                    let frames = 257;
                    let input: Vec<f32> = (0..frames * channels)
                        .map(|i| (i as f32 * 0.23).sin() * 0.6 + (i as f32 * 0.031).cos() * 0.2)
                        .collect();
                    let output = run_flushed(&mut plugin, &input, channels, rate);
                    let start = latency * channels;
                    if bands == 0 {
                        assert_eq!(
                            &output[start..start + input.len()],
                            input.as_slice(),
                            "rate={rate} channels={channels}"
                        );
                    } else {
                        let worst = output[start..start + input.len()]
                            .iter()
                            .zip(input.iter())
                            .map(|(a, b)| (a - b).abs())
                            .fold(0.0_f32, f32::max);
                        assert!(
                            worst < 1.0e-6,
                            "rate={rate} channels={channels} bands={bands} worst={worst}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn a2_residual_and_repaired_sum_to_delayed_dry() {
    for mode in [0, 1] {
        for bands in [0, 1, 2] {
            let clean = sine(512, 330.0, SR as f32, 0.3);
            let (signal, _) = corrupt(&clean, &[(200, 1, 1.0), (300, 3, -1.0)]);
            let base = DeclickPluginParams {
                mode,
                bands,
                sensitivity: 3.0,
                ..Default::default()
            };
            let run_with = |mut plugin: DeclickPlugin| run(&mut plugin, &signal, 1, SR);
            let dry = run_with(
                DeclickPlugin::from_params(
                    1,
                    SR,
                    DeclickPluginParams {
                        enabled: false,
                        ..base.clone()
                    },
                )
                .unwrap(),
            );
            let repaired = run_with(DeclickPlugin::from_params(1, SR, base.clone()).unwrap());
            let residual = run_with(
                DeclickPlugin::from_params(
                    1,
                    SR,
                    DeclickPluginParams {
                        audition_residual: true,
                        ..base
                    },
                )
                .unwrap(),
            );
            assert_eq!(dry.len(), repaired.len());
            assert_eq!(dry.len(), residual.len());
            let mut worst = 0.0_f32;
            for i in 0..dry.len() {
                worst = worst.max((repaired[i] + residual[i] - dry[i]).abs());
            }
            // residual = dry - repaired in f32, so the regrouped sum
            // matches dry within a few ulps on sub-4.0 signals.
            assert!(worst < 1.0e-5, "mode={mode} bands={bands} worst={worst}");
            report(&format!("mode={mode} bands={bands} regroup"), worst);
        }
    }
}

#[test]
fn a2_skew_and_crossover_keep_detection_alive() {
    let clean = sine(512, 440.0, SR as f32, 0.25);
    let (signal, _) = corrupt(&clean, &[(200, 1, 1.0)]);
    let mut worst = 0.0_f32;
    for bands in [1, 2] {
        for skew in [-1.0, 1.0] {
            for crossover_hz in [80.0, 4000.0, 12_000.0] {
                let mut plugin = DeclickPlugin::from_params(
                    1,
                    SR,
                    DeclickPluginParams {
                        bands,
                        crossover_hz,
                        frequency_skew: skew,
                        sensitivity: 3.0,
                        ..Default::default()
                    },
                )
                .unwrap();
                let latency = plugin.latency_samples();
                let output = run(&mut plugin, &signal, 1, SR);
                let error = (output[200 + latency] - clean[200]).abs();
                worst = worst.max(error);
                assert!(
                    error < 0.15,
                    "bands={bands} skew={skew} crossover={crossover_hz} error={error}"
                );
            }
        }
    }
    report("skew/crossover repair-error", worst);
}

#[test]
fn a3_rates_links_latency_and_drain_matrix() {
    let configs: Vec<(&str, DeclickPluginParams, usize)> = vec![
        ("legacy", DeclickPluginParams::default(), 8),
        (
            "periodic",
            DeclickPluginParams {
                mode: 1,
                ..Default::default()
            },
            8,
        ),
        (
            "multiband",
            DeclickPluginParams {
                bands: 2,
                ..Default::default()
            },
            8,
        ),
        (
            "widened",
            DeclickPluginParams {
                bands: 1,
                repair_width: 3,
                ..Default::default()
            },
            11,
        ),
    ];
    for rate in [44_100, 48_000, 96_000] {
        for linked in [true, false] {
            for (name, template, latency) in &configs {
                let mut plugin = DeclickPlugin::from_params(
                    2,
                    rate,
                    DeclickPluginParams {
                        link_channels: linked,
                        ..template.clone()
                    },
                )
                .unwrap();
                assert_eq!(plugin.latency_samples(), *latency, "{name} rate={rate}");
                assert_eq!(
                    plugin.tail_length(),
                    TailLength::Finite(*latency as u64),
                    "{name} rate={rate}"
                );
                // Partial final click: corruption in the last two frames.
                let frames = 129;
                let mut input = vec![0.1; frames * 2];
                input[(frames - 2) * 2] += CLICK_AMP;
                input[(frames - 1) * 2 + 1] -= CLICK_AMP;
                let output = run_drained(&mut plugin, input.as_slice(), 2, rate);
                assert_eq!(
                    output.len(),
                    (frames + latency) * 2,
                    "{name} rate={rate} linked={linked}"
                );
                assert!(
                    output.iter().all(|sample| sample.is_finite()),
                    "{name} rate={rate}"
                );
                // Stable completion, then reset equivalence with a fresh
                // instance over identical blocks.
                let mut tail = vec![0.0; 8 * 2];
                assert!(
                    plugin
                        .drain(&mut tail, &ProcessContext::new(rate, 8))
                        .unwrap()
                        .complete
                );
                plugin.reset();
                let mut fresh = DeclickPlugin::from_params(
                    2,
                    rate,
                    DeclickPluginParams {
                        link_channels: linked,
                        ..template.clone()
                    },
                )
                .unwrap();
                let replay = run_drained(&mut plugin, input.as_slice(), 2, rate);
                let expected = run_drained(&mut fresh, input.as_slice(), 2, rate);
                assert_eq!(replay, expected, "{name} rate={rate} linked={linked}");
            }
        }
    }
}

#[test]
fn a3_block_partitioning_automation_and_structural_switches() {
    let template = DeclickPluginParams {
        mode: 1,
        bands: 1,
        sensitivity: 2.0,
        ..Default::default()
    };
    let input: Vec<f32> = (0..1023).map(|i| (i as f32 * 0.11).sin() * 0.2).collect();
    let mut whole_input = input.clone();
    let mut whole = DeclickPlugin::from_params(1, SR, template.clone()).unwrap();
    whole
        .process_in_place(&mut whole_input, &ProcessContext::new(SR, 1023))
        .unwrap();
    let mut chunked_input = input.clone();
    let mut chunked = DeclickPlugin::from_params(1, SR, template).unwrap();
    let mut frame = 0;
    for block in [1, 7, 13].iter().cycle() {
        if frame >= 1023 {
            break;
        }
        let frames = (*block).min(1023 - frame);
        chunked
            .process_in_place(
                &mut chunked_input[frame..frame + frames],
                &ProcessContext::new(SR, frames),
            )
            .unwrap();
        frame += frames;
    }
    assert_eq!(whole_input, chunked_input);

    // Live automation stays finite; structural switches update latency,
    // keep the stream open, and drain exactly.
    let mut plugin = DeclickPlugin::new(2, SR).unwrap();
    let mut block = vec![0.25; 128 * 2];
    for sensitivity in [1.0, 50.0, 100.0, 2.0] {
        plugin
            .set_parameter(
                ParameterId::from("sensitivity"),
                ParameterValue::Float(sensitivity),
            )
            .unwrap();
        plugin
            .set_parameter(
                ParameterId::from("frequency_skew"),
                ParameterValue::Float(sensitivity / 100.0 - 0.5),
            )
            .unwrap();
        plugin
            .process_in_place(&mut block, &ProcessContext::new(SR, 128))
            .unwrap();
        assert!(block.iter().all(|sample| sample.is_finite()));
        block.fill(0.25);
    }
    plugin
        .set_parameter(ParameterId::from("mode"), ParameterValue::Int(1))
        .unwrap();
    assert_eq!(plugin.latency_samples(), 8);
    plugin
        .set_parameter(ParameterId::from("bands"), ParameterValue::Int(2))
        .unwrap();
    plugin
        .set_parameter(ParameterId::from("repair_width"), ParameterValue::Int(4))
        .unwrap();
    assert_eq!(plugin.latency_samples(), 12);
    plugin
        .process_in_place(&mut block, &ProcessContext::new(SR, 128))
        .unwrap();
    assert!(block.iter().all(|sample| sample.is_finite()));
    let mut drained = 0;
    loop {
        let mut out = vec![0.0; 5 * 2];
        let status = plugin.drain(&mut out, &ProcessContext::new(SR, 5)).unwrap();
        drained += status.frames;
        assert!(out[..status.frames * 2].iter().all(|s| s.is_finite()));
        if status.complete {
            break;
        }
    }
    assert_eq!(drained, 12);
}

#[test]
fn a3_param_plumbing_and_state_compat() {
    let mut plugin = DeclickPlugin::new(1, SR).unwrap();
    let schema = plugin.parameter_schema();
    assert_eq!(schema.len(), 9);
    let keys: Vec<&str> = schema.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(
        keys,
        [
            "enabled",
            "sensitivity",
            "link_channels",
            "mode",
            "bands",
            "crossover_hz",
            "frequency_skew",
            "repair_width",
            "audition_residual"
        ]
    );
    // Getters expose the documented value types.
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("mode")),
        Some(ParameterValue::Int(0))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("repair_width")),
        Some(ParameterValue::Int(0))
    );
    // Setters roundtrip every new parameter, including label addressing.
    plugin
        .set_parameter(ParameterId::from("mode"), ParameterValue::Int(1))
        .unwrap();
    plugin
        .set_parameter(
            ParameterId::from("bands"),
            ParameterValue::String("3-band".into()),
        )
        .unwrap();
    plugin
        .set_parameter(
            ParameterId::from("crossover_hz"),
            ParameterValue::Float(8000.0),
        )
        .unwrap();
    plugin
        .set_parameter(
            ParameterId::from("frequency_skew"),
            ParameterValue::Float(-0.5),
        )
        .unwrap();
    plugin
        .set_parameter(ParameterId::from("repair_width"), ParameterValue::Int(5))
        .unwrap();
    plugin
        .set_parameter(
            ParameterId::from("audition_residual"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("mode")),
        Some(ParameterValue::Int(1))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("bands")),
        Some(ParameterValue::Int(2))
    );
    assert_eq!(plugin.latency_samples(), 13);
    let schema = plugin.parameter_schema();
    assert_eq!(schema[3].default_value, ParameterValue::Int(1));
    assert_eq!(schema[7].default_value, ParameterValue::Int(5));
    // Unknown labels and parameters are rejected transactionally.
    assert!(
        plugin
            .set_parameter(
                ParameterId::from("mode"),
                ParameterValue::String("Bogus".into())
            )
            .is_err()
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("mode")),
        Some(ParameterValue::Int(1))
    );
    let mut batch = ParameterSet::new();
    batch.insert(ParameterId::from("bands"), ParameterValue::Int(0));
    batch.insert(ParameterId::from("unknown"), ParameterValue::Float(1.0));
    assert!(plugin.apply_values(batch).is_err());
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("bands")),
        Some(ParameterValue::Int(2))
    );

    // Old three-key state migrates to neutral new-mode defaults.
    let legacy: DeclickPluginParams = serde_json::from_value(serde_json::json!({
        "enabled": true,
        "sensitivity": 7.0,
        "link_channels": false,
    }))
    .unwrap();
    assert_eq!((legacy.mode, legacy.bands, legacy.repair_width), (0, 0, 0));
    assert_eq!(legacy.crossover_hz, 4000.0);
    assert_eq!(legacy.frequency_skew, 0.0);
    assert!(!legacy.audition_residual);
    let legacy_plugin = DeclickPlugin::from_params(1, SR, legacy).unwrap();
    assert_eq!(legacy_plugin.latency_samples(), 8);

    // Choice labels deserialize; out-of-range indices are hard errors.
    let labeled: DeclickPluginParams = serde_json::from_value(serde_json::json!({
        "mode": "Periodic",
        "bands": "2-band",
        "repair_width": 3,
    }))
    .unwrap();
    assert_eq!(
        (labeled.mode, labeled.bands, labeled.repair_width),
        (1, 1, 3)
    );
    assert!(
        serde_json::from_value::<DeclickPluginParams>(serde_json::json!({ "mode": 9 })).is_err()
    );
    // Malformed numerics canonicalize instead of poisoning the plugin.
    let repaired = DeclickPlugin::from_params(
        1,
        SR,
        DeclickPluginParams {
            crossover_hz: f32::NAN,
            frequency_skew: -99.0,
            repair_width: 99,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        repaired.get_parameter(&ParameterId::from("crossover_hz")),
        Some(ParameterValue::Float(4000.0))
    );
    assert_eq!(
        repaired.get_parameter(&ParameterId::from("frequency_skew")),
        Some(ParameterValue::Float(-1.0))
    );
    assert_eq!(
        repaired.get_parameter(&ParameterId::from("repair_width")),
        Some(ParameterValue::Int(8))
    );
}

#[test]
fn a3_multiband_non_finite_inputs_recover_exactly() {
    // NaN/Inf must recover locally on every owned path: the one-pole
    // crossover state must not latch NaN, and dry taps must hold the last
    // finite frame. A corrupted run is therefore bit-identical to a clean
    // run with the hold-substituted values, with exact recovery on the
    // following clean frames.
    let configs: Vec<(&str, usize, usize, usize, bool)> = vec![
        ("random-2band", 0, 1, 0, false),
        ("random-3band", 0, 2, 0, false),
        ("periodic-2band", 1, 1, 0, false),
        ("periodic-3band", 1, 2, 0, false),
        ("periodic-3band-wide", 1, 2, 4, false),
        ("random-3band-residual", 0, 2, 0, true),
    ];
    for (name, mode, bands, repair_width, audition_residual) in configs {
        let frames = 256;
        let channels = 2;
        let clean: Vec<f32> = (0..frames * channels)
            .map(|i| (i as f32 * 0.11).sin() * 0.2 + 0.05)
            .collect();
        let mut damaged = clean.clone();
        damaged[40 * channels] = f32::NAN;
        damaged[80 * channels + 1] = f32::NEG_INFINITY;
        // Hold-last reference: what the sanitizer substitutes.
        let mut held = clean.clone();
        held[40 * channels] = clean[39 * channels];
        held[80 * channels + 1] = clean[79 * channels + 1];
        let base = DeclickPluginParams {
            mode,
            bands,
            repair_width,
            audition_residual,
            sensitivity: 2.0,
            ..Default::default()
        };
        let mut dirty = DeclickPlugin::from_params(channels, SR, base.clone()).unwrap();
        let dirty_out = run_flushed(&mut dirty, &damaged, channels, SR);
        let mut reference = DeclickPlugin::from_params(channels, SR, base).unwrap();
        let held_out = run_flushed(&mut reference, &held, channels, SR);
        assert!(
            dirty_out.iter().all(|sample| sample.is_finite()),
            "{name}: non-finite output"
        );
        assert_eq!(dirty_out, held_out, "{name}: hold equivalence");
    }
}

#[test]
fn a1_two_phase_grid_repairs_both_phases() {
    // Two interleaved 200-sample grids offset by 50: the tracker keeps one
    // lock, so one phase is sensitive and the other guarded x4. Loud clicks
    // repair on both phases; per-phase recall is recorded in the messages.
    // Marginal clicks on an additional in-range phase may be missed
    // (documented single-phase limitation, structurally pinned in
    // repair.rs tracker tests).
    let frames = 2600;
    let clean = sine(frames, 220.0, SR as f32, 0.2);
    let mut signal = clean.clone();
    let mut phase_a = Vec::new();
    let mut phase_b = Vec::new();
    for start in (140..frames - 64).step_by(200) {
        signal[start] += CLICK_AMP;
        phase_a.push(start);
        signal[start + 50] += CLICK_AMP;
        phase_b.push(start + 50);
    }
    let mut plugin = DeclickPlugin::from_params(
        1,
        SR,
        DeclickPluginParams {
            mode: 1,
            sensitivity: 2.0,
            ..Default::default()
        },
    )
    .unwrap();
    let latency = plugin.latency_samples();
    let output = run(&mut plugin, &signal, 1, SR);
    for (name, phase) in [("A", &phase_a), ("B", &phase_b)] {
        let late: Vec<usize> = phase.iter().copied().filter(|&p| p >= 1300).collect();
        let repaired = late
            .iter()
            .filter(|&&p| (output[p + latency] - clean[p]).abs() < 0.15)
            .count();
        let recall = repaired as f32 / late.len() as f32;
        assert!(
            recall >= 0.95,
            "phase {name}: recall={recall} ({repaired}/{})",
            late.len()
        );
        eprintln!("[declick-accuracy] two-phase {name} recall={recall:.4}");
    }
}

#[test]
fn a1_slow_repetition_falls_back_to_random_style_repair() {
    // 600-sample repetition exceeds the 32-512 tracker range: periodic mode
    // never locks (unit-pinned in repair.rs) and repairs like random mode.
    let frames = 2600;
    let clean = sine(frames, 220.0, SR as f32, 0.2);
    let mut signal = clean.clone();
    let mut positions = Vec::new();
    let mut start = 140;
    while start < frames - 64 {
        signal[start] += CLICK_AMP;
        positions.push(start);
        start += 600;
    }
    let mut random = DeclickPlugin::from_params(
        1,
        SR,
        DeclickPluginParams {
            sensitivity: 2.0,
            ..Default::default()
        },
    )
    .unwrap();
    let random_out = run(&mut random, &signal, 1, SR);
    let mut periodic = DeclickPlugin::from_params(
        1,
        SR,
        DeclickPluginParams {
            mode: 1,
            sensitivity: 2.0,
            ..Default::default()
        },
    )
    .unwrap();
    let latency = periodic.latency_samples();
    assert_eq!(latency, 8);
    let periodic_out = run(&mut periodic, &signal, 1, SR);
    assert_eq!(random_out.len(), periodic_out.len());
    // Fallback equivalence within the accepted cross-implementation bound
    // (legacy vs owned cores, cf. neutral_owned_core_matches_legacy_suppressor).
    let worst = random_out
        .iter()
        .zip(periodic_out.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0_f32, f32::max);
    assert!(worst < 1.0e-6, "fallback drift {worst}");
    report("vinyl fallback drift", worst);
    let mut worst_repair = 0.0_f32;
    for &position in &positions {
        let error = (periodic_out[position + latency] - clean[position]).abs();
        worst_repair = worst_repair.max(error);
        assert!(error < 0.15, "position={position} error={error}");
    }
    report("vinyl fallback repair-error", worst_repair);
}

#[test]
fn a1_independent_channels_with_different_periods_both_repair() {
    // The period tracker follows the OR of channel triggers (documented):
    // per-channel periods are unsupported, but loud clicks on both
    // channels still repair at sensitivity 2.0 under any lock state.
    let frames = 2600;
    let rate = SR as f32;
    let mut signal = vec![0.0; frames * 2];
    for frame in 0..frames {
        let sample = (frame as f32 * 220.0 / rate * std::f32::consts::TAU).sin() * 0.2;
        signal[frame * 2] = sample;
        signal[frame * 2 + 1] = sample;
    }
    let clean = signal.clone();
    let mut grids: [Vec<usize>; 2] = [Vec::new(), Vec::new()];
    for start in (140..frames - 64).step_by(100) {
        signal[start * 2] += CLICK_AMP;
        grids[0].push(start);
    }
    for start in (140..frames - 64).step_by(150) {
        signal[start * 2 + 1] += CLICK_AMP;
        grids[1].push(start);
    }
    let mut plugin = DeclickPlugin::from_params(
        2,
        SR,
        DeclickPluginParams {
            mode: 1,
            link_channels: false,
            sensitivity: 2.0,
            ..Default::default()
        },
    )
    .unwrap();
    let latency = plugin.latency_samples();
    let output = run(&mut plugin, &signal, 2, SR);
    for (ch, grid) in grids.iter().enumerate() {
        let late: Vec<usize> = grid.iter().copied().filter(|&p| p >= 1300).collect();
        let repaired = late
            .iter()
            .filter(|&&p| (output[(p + latency) * 2 + ch] - clean[p * 2 + ch]).abs() < 0.15)
            .count();
        let recall = repaired as f32 / late.len() as f32;
        assert!(
            recall >= 0.95,
            "channel {ch}: recall={recall} ({repaired}/{})",
            late.len()
        );
        eprintln!("[declick-accuracy] per-channel {ch} recall={recall:.4}");
    }
}

#[test]
fn a3_initial_skew_applies_immediately_at_construction() {
    // Construction with nonzero skew must match set-then-settle exactly:
    // the immediate sensitivity snapshot must already use skewed targets
    // instead of converging from neutral over the first milliseconds.
    let mut signal = sine(512, 440.0, SR as f32, 0.25);
    signal[200] += CLICK_AMP;
    let mut constructed = DeclickPlugin::from_params(
        1,
        SR,
        DeclickPluginParams {
            bands: 1,
            frequency_skew: 1.0,
            sensitivity: 3.0,
            ..Default::default()
        },
    )
    .unwrap();
    let out_a = run(&mut constructed, &signal, 1, SR);
    let mut settled = DeclickPlugin::from_params(
        1,
        SR,
        DeclickPluginParams {
            bands: 1,
            sensitivity: 3.0,
            ..Default::default()
        },
    )
    .unwrap();
    settled
        .set_parameter(
            ParameterId::from("frequency_skew"),
            ParameterValue::Float(1.0),
        )
        .unwrap();
    settled.reset();
    let out_b = run(&mut settled, &signal, 1, SR);
    assert_eq!(out_a, out_b);
}

#[test]
fn a2_skew_changes_multiband_output_somewhere_in_sweep() {
    // Skew shifts per-band thresholds 4x between -1 and +1 while the
    // unskewed supervisor gate stays put, so across a 50x sensitivity
    // sweep some band decision must flip on the smeared click residuals
    // unless skew is a true no-op (identical splits, decisions, and
    // baselines would render bit-identically at every setting). Each
    // setting's delta is recorded; no magnitude is pinned.
    let mut signal = sine(512, 440.0, SR as f32, 0.25);
    signal[200] += 6.0;
    for slot in &mut signal[300..303] {
        *slot += 6.0;
    }
    let render = |sensitivity: f32, skew: f32| {
        let mut plugin = DeclickPlugin::from_params(
            1,
            SR,
            DeclickPluginParams {
                bands: 1,
                crossover_hz: 4000.0,
                frequency_skew: skew,
                sensitivity,
                ..Default::default()
            },
        )
        .unwrap();
        run(&mut plugin, &signal, 1, SR)
    };
    let mut hits = 0;
    for sensitivity in [1.0, 2.0, 3.0, 5.0, 8.0, 12.0, 20.0, 30.0, 50.0] {
        let negative = render(sensitivity, -1.0);
        let positive = render(sensitivity, 1.0);
        assert!(negative.iter().all(|sample| sample.is_finite()));
        assert!(positive.iter().all(|sample| sample.is_finite()));
        let delta = negative
            .iter()
            .zip(positive.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);
        eprintln!("[declick-accuracy] skew sweep sensitivity={sensitivity} delta={delta:.6}");
        if delta > 1.0e-6 {
            hits += 1;
        }
    }
    assert!(
        hits > 0,
        "skew rendered bit-identically at every sweep setting"
    );
}

#[test]
fn a2_crossover_frequency_changes_multiband_output() {
    // Different splits produce different band signals, so repaired values
    // differ wherever any band repairs. A repaired click at sensitivity 3
    // (residual far above threshold) therefore separates crossover extremes
    // by interpolation-scale deltas, not rounding.
    let clean = sine(512, 440.0, SR as f32, 0.25);
    let (signal, _) = corrupt(&clean, &[(200, 1, 1.0), (300, 3, -1.0)]);
    for bands in [1, 2] {
        let render = |crossover_hz: f32| {
            let mut plugin = DeclickPlugin::from_params(
                1,
                SR,
                DeclickPluginParams {
                    bands,
                    crossover_hz,
                    sensitivity: 3.0,
                    ..Default::default()
                },
            )
            .unwrap();
            run(&mut plugin, &signal, 1, SR)
        };
        let low = render(80.0);
        let high = render(12_000.0);
        assert!(low.iter().all(|sample| sample.is_finite()));
        assert!(high.iter().all(|sample| sample.is_finite()));
        let delta = low
            .iter()
            .zip(high.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);
        eprintln!("[declick-accuracy] crossover bands={bands} delta={delta:.6}");
        assert!(
            delta > 1.0e-6,
            "bands={bands}: crossover extremes rendered bit-identically"
        );
    }
}

#[test]
fn a1_multiband_periodic_grid_repairs_within_bound() {
    // The hardest topology: per-band period trackers plus the supervisor
    // (all recomputing past frame 1024) gating smeared band repairs. Loud
    // grid clicks must repair within the uniform bound after the lock
    // window; a mistracking band would guard grid frames and fail recall.
    let frames = 1500;
    let clean = sine(frames, 220.0, SR as f32, 0.2);
    let mut signal = clean.clone();
    let mut grid = Vec::new();
    for start in (140..frames - 64).step_by(100) {
        signal[start] += CLICK_AMP;
        grid.push(start);
    }
    for bands in [1, 2] {
        let mut plugin = DeclickPlugin::from_params(
            1,
            SR,
            DeclickPluginParams {
                mode: 1,
                bands,
                sensitivity: 2.0,
                ..Default::default()
            },
        )
        .unwrap();
        let latency = plugin.latency_samples();
        let output = run(&mut plugin, &signal, 1, SR);
        assert!(
            output.iter().all(|sample| sample.is_finite()),
            "bands={bands}"
        );
        let late: Vec<usize> = grid.iter().copied().filter(|&p| p >= 1024).collect();
        assert!(!late.is_empty(), "bands={bands}: no post-lock grid clicks");
        let mut worst = 0.0_f32;
        let mut repaired = 0;
        for &position in &late {
            let error = (output[position + latency] - clean[position]).abs();
            worst = worst.max(error);
            if error < 0.15 {
                repaired += 1;
            }
        }
        let recall = repaired as f32 / late.len() as f32;
        assert!(
            recall >= 0.95,
            "bands={bands}: recall={recall} ({repaired}/{})",
            late.len()
        );
        eprintln!("[declick-accuracy] multiband-periodic bands={bands} recall={recall:.4}");
        report(
            &format!("multiband-periodic bands={bands} repair-error"),
            worst,
        );
    }
}

#[test]
fn a3_rejected_batch_preserves_populated_history_exactly() {
    // Rejection validates into a local staging struct before touching DSP
    // state, so processing after a rejected batch or label is bit-identical
    // to an uninterrupted run over correlated, detector-populated history.
    let clean = sine(1024, 440.0, SR as f32, 0.25);
    let (mono, _) = corrupt(&clean, &click_plan());
    let mut signal = vec![0.0; 1024 * 2];
    for frame in 0..1024 {
        signal[frame * 2] = mono[frame];
        signal[frame * 2 + 1] = mono[frame] * 0.5;
    }
    let template = DeclickPluginParams {
        mode: 1,
        bands: 1,
        repair_width: 2,
        sensitivity: 2.0,
        ..Default::default()
    };
    let drive = |plugin: &mut DeclickPlugin, reject_midway: bool| {
        let mut out = Vec::new();
        for (block, frames) in [(0, 256), (256, 256), (512, 512)] {
            let mut chunk = signal[block * 2..(block + frames) * 2].to_vec();
            plugin
                .process_in_place(&mut chunk, &ProcessContext::new(SR, frames))
                .unwrap();
            out.extend_from_slice(&chunk);
            if reject_midway && block == 0 {
                let mut batch = ParameterSet::new();
                batch.insert(ParameterId::from("sensitivity"), ParameterValue::Float(9.0));
                batch.insert(ParameterId::from("bands"), ParameterValue::Int(0));
                batch.insert(ParameterId::from("unknown"), ParameterValue::Float(1.0));
                assert!(plugin.apply_values(batch).is_err());
                assert!(
                    plugin
                        .set_parameter(
                            ParameterId::from("mode"),
                            ParameterValue::String("Bogus".into())
                        )
                        .is_err()
                );
                // Neither the valid entries nor the structural entry applied.
                assert_eq!(
                    plugin.get_parameter(&ParameterId::from("sensitivity")),
                    Some(ParameterValue::Float(2.0))
                );
                assert_eq!(
                    plugin.get_parameter(&ParameterId::from("bands")),
                    Some(ParameterValue::Int(1))
                );
                assert_eq!(
                    plugin.get_parameter(&ParameterId::from("mode")),
                    Some(ParameterValue::Int(1))
                );
            }
        }
        let latency = plugin.latency_samples();
        loop {
            let mut tail = vec![0.0; latency * 2];
            let status = plugin
                .drain(&mut tail, &ProcessContext::new(SR, latency))
                .unwrap();
            out.extend_from_slice(&tail[..status.frames * 2]);
            if status.complete {
                return out;
            }
        }
    };
    let mut reference = DeclickPlugin::from_params(2, SR, template.clone()).unwrap();
    let expected = drive(&mut reference, false);
    let mut interrupted = DeclickPlugin::from_params(2, SR, template).unwrap();
    assert_eq!(drive(&mut interrupted, true), expected);
}

/// Aggregated guard sweep at one repair width (R10 matrix, R21
/// aggregation; R23 width parameter). Every bound, condition, cell,
/// message, and report is width-independent; only the template width
/// and the expected latency (8 + width) vary, so the width-0 call
/// reproduces the original test exactly.
fn collect_guard_sweep_failures(repair_width: usize, failures: &mut Vec<String>) {
    let frames = 1441;
    let quiet_on = 1240;
    let mut off_probes: Vec<usize> = (1150..1230).step_by(7).collect();
    off_probes.extend((1250..1340).step_by(7));
    off_probes.push(1290);
    for bands_param in [1, 2] {
        let band_count = bands_param + 1;
        for rate in [44_100, 48_000, 96_000] {
            for amplitude in [0.3, 0.5, 0.7, 1.0, 1.5, 2.0] {
                // Miss-robust iff the guarded bar clears the probe:
                // 44.1k bar 1.73 and 48k bar 1.88 exceed 1.5 plus
                // curvature; the 96k bar 0.85 exceeds only 0.7.
                let expect_miss =
                    (rate != 96_000 && amplitude <= 1.5) || (rate == 96_000 && amplitude <= 0.7);
                let tag = format!(
                    "bands={band_count} rate={rate} ch=1 linked=true width={repair_width} xover=4000 amp={amplitude}"
                );
                let tone = sine(frames, 440.0, rate as f32, 0.25);
                let mut corrupted = tone.clone();
                let mut start = 140;
                while start < 1341 {
                    if start != quiet_on {
                        corrupted[start] += CLICK_AMP;
                    }
                    start += 100;
                }
                corrupted[quiet_on] += amplitude;
                for &at in &off_probes {
                    corrupted[at] += amplitude;
                }
                let template = DeclickPluginParams {
                    mode: 1,
                    bands: bands_param,
                    repair_width,
                    link_channels: true,
                    sensitivity: 5.0,
                    crossover_hz: 4000.0,
                    ..Default::default()
                };
                let mut plugin = DeclickPlugin::from_params(1, rate, template).unwrap();
                let latency = plugin.latency_samples();
                // Structural failures collect and skip the cell: downstream
                // indices would be misaligned, so its errors are meaningless.
                let expected_latency = FULL_CONTEXT + repair_width;
                if latency != expected_latency {
                    failures.push(format!(
                        "{tag}: latency: got {latency}, want {expected_latency}"
                    ));
                    continue;
                }
                let repaired = run_drained(&mut plugin, &corrupted, 1, rate);
                if repaired.len() != frames + latency {
                    failures.push(format!(
                        "{tag}: length: got {}, want {}",
                        repaired.len(),
                        frames + latency
                    ));
                    continue;
                }
                let bad = repaired.iter().filter(|s| !s.is_finite()).count();
                if bad > 0 {
                    failures.push(format!("{tag}: finite: {bad} non-finite samples"));
                }
                let on_err = (repaired[quiet_on + latency] - tone[quiet_on]).abs();
                // Explicit partial_cmp (R22 lint): NaN compares as None,
                // which is not Less, so non-finite errors still fail.
                if on_err.partial_cmp(&0.15) != Some(Ordering::Less) {
                    failures.push(format!("{tag}: quiet on-phase must repair, error={on_err}"));
                }
                report(&format!("{tag} sweep-on"), on_err);
                let mut guard_margin = f32::INFINITY;
                let mut worst_fire = 0.0_f32;
                for &at in &off_probes {
                    let error = (repaired[at + latency] - tone[at]).abs();
                    guard_margin = guard_margin.min(error);
                    worst_fire = worst_fire.max(error);
                    if expect_miss {
                        // Explicit partial_cmp (R22 lint): NaN compares
                        // as None, which is not Greater, so non-finite
                        // errors still fail.
                        let miss_floor = amplitude / 2.0;
                        if error.partial_cmp(&miss_floor) != Some(Ordering::Greater) {
                            failures.push(format!(
                                "{tag}: off-phase probe at={at} must miss (guard), error={error}"
                            ));
                        }
                    } else {
                        // Fire expected (probe clears the guarded bar)
                        // but at thin margins: forbid the partial-repair
                        // middle instead of pinning the side; the worst
                        // below shows which side the gate took.
                        if !(error < 0.15 || error > amplitude / 2.0) {
                            failures.push(format!(
                                "{tag}: off-phase probe at={at} must fully repair or miss, error={error}"
                            ));
                        }
                    }
                }
                report(&format!("{tag} guard-margin"), guard_margin);
                if !expect_miss {
                    report(&format!("{tag} fire-worst"), worst_fire);
                }
            }
        }
    }
}

#[test]
fn a1_periodic_guard_holds_across_phase_rate_and_amplitude() {
    // Off-phase guard discrimination across tone phase, rate, and quiet
    // amplitude (R10): the x4 guard must reject programme-scale transients
    // at EVERY signal phase, not just steep tone windows. Local slope hits
    // zero at tone peaks, so a guard referenced to local scale collapses on
    // flat phases (measured: 48kHz/1290 fired 4.7% over while 44.1kHz/1290
    // held 2.4x margin on a steep phase). The guarded fullband authority
    // therefore floors scale on the tracked programme scale, which is
    // phase-independent by construction (a max over history). This leg
    // proves the regime change is not fixture-fitted: 26 off-phase probes
    // spanning ~0.8 tone cycles at every rate (steep AND flat phases),
    // each of which must miss (error > amplitude/2, i.e. the probe stands),
    // while the on-phase probe repairs (error < 0.15) at the same
    // amplitude. Probe wings sit clear of grid clicks and of the on-phase
    // probe's context; missed probes never feed the tracker, so the lock
    // phase is undisturbed. Matrix: both multiband topologies x 3 rates x
    // amplitudes {0.3, 0.5, 0.7, 1.0, 1.5, 2.0} at every rate (R15 B2
    // closes the 96kHz x 0.7 carve-out and the 0.7-3.0 guard-transition
    // hole). Off-phase expectations follow the R10 measured guarded
    // bars (44.1k: 1.73, 48k: 1.88, 96k: 0.85; floor-bound maxima, so
    // local scale can only raise them and miss margins only grow):
    // one-sided MISS (error > amplitude/2) where the bar exceeds the
    // probe plus curvature (44.1/48k through 1.5, 96k through 0.7);
    // two-sided NO-PARTIAL-MIDDLE (error < 0.15 or > amplitude/2) where
    // fire is expected but margins are thin (44.1/48k x 2.0, 96k x
    // 1.0/1.5/2.0), with the fire side predicted and the worst
    // reported. Each amplitude renders fresh, so firing probes never
    // disturb another amplitude's lock phase. On-phase always repairs.
    // R21: failures aggregate across ALL matrix cells and assert at the
    // end, so one red cell no longer hides the rest of the pattern.
    // Every bound, condition, cell, message, and report below is
    // unchanged; only short-circuit panics become collected entries.
    // R23: the sweep body lives in `collect_guard_sweep_failures`;
    // width 0 reproduces this leg exactly.
    let mut failures: Vec<String> = Vec::new();
    collect_guard_sweep_failures(0, &mut failures);
    assert!(
        failures.is_empty(),
        "guard sweep collected {} failure(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn a1_periodic_guard_holds_across_phase_rate_and_amplitude_width3() {
    // R23/F5: the identical aggregated guard sweep at repair width 3
    // (latency 11): same cells, bounds, conditions, messages. Width>0
    // skips R22 sum-forcing, so guarded loud frames emit unforced
    // band-sums; green here measures the skip safe, red names the
    // hole. Red probes adjacent to grid clicks implicate widen
    // hysteresis overlap, red isolated probes implicate unforced
    // partials -- the failure-list positions decide.
    let mut failures: Vec<String> = Vec::new();
    collect_guard_sweep_failures(3, &mut failures);
    assert!(
        failures.is_empty(),
        "guard sweep width 3 collected {} failure(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn a1_periodic_guard_preserves_loud_override() {
    // Guarded loud-click override plus recall/precision under lock (R10):
    // the programme-referenced guard must reject quiet transients yet
    // still repair loud clicks at guarded (off-phase) positions. Three
    // full-amplitude probes sit 10/50/90 samples off the live lock phase;
    // each is guarded at fire time (off-grid repairs no longer re-anchor
    // an established lock (R17), so every loud proves override at a
    // guarded position past the unmoved grid phase, and the 1340 grid
    // click itself stays on-phase). Each loud must repair (removal > 1.0
    // plus error < 0.15), recall/precision over grid plus louds must hold
    // >= 0.95 through the residual tap, and the clean on-phase frame must
    // stay within 0.05 damage (sensitive-phase cleanliness). Matrix: both
    // multiband topologies x 3 rates, mono, width 0, 4kHz crossover.
    let frames = 1441;
    let quiet_frame = 1240;
    let loud_probes = [1250, 1290, 1330];
    for bands_param in [1, 2] {
        let band_count = bands_param + 1;
        for rate in [44_100, 48_000, 96_000] {
            let tag = format!("bands={band_count} rate={rate} ch=1 linked=true width=0 xover=4000");
            let tone = sine(frames, 440.0, rate as f32, 0.25);
            let mut corrupted = tone.clone();
            let mut is_click = vec![false; frames];
            let mut start = 140;
            while start < 1341 {
                if start != quiet_frame {
                    corrupted[start] += CLICK_AMP;
                    is_click[start] = true;
                }
                start += 100;
            }
            for &at in &loud_probes {
                corrupted[at] += CLICK_AMP;
                is_click[at] = true;
            }
            let template = DeclickPluginParams {
                mode: 1,
                bands: bands_param,
                repair_width: 0,
                link_channels: true,
                sensitivity: 5.0,
                crossover_hz: 4000.0,
                ..Default::default()
            };
            let mut plugin = DeclickPlugin::from_params(1, rate, template.clone()).unwrap();
            let latency = plugin.latency_samples();
            assert_eq!(latency, FULL_CONTEXT, "{tag}: latency");
            let repaired = run_drained(&mut plugin, &corrupted, 1, rate);
            let mut residual_plugin = DeclickPlugin::from_params(
                1,
                rate,
                DeclickPluginParams {
                    audition_residual: true,
                    ..template
                },
            )
            .unwrap();
            let residual = run_drained(&mut residual_plugin, &corrupted, 1, rate);
            assert_eq!(repaired.len(), frames + latency, "{tag}: length");
            assert!(repaired.iter().all(|s| s.is_finite()), "{tag}: finite");
            let mut worst_loud = 0.0_f32;
            for &at in &loud_probes {
                let actual = repaired[at + latency];
                let spike = corrupted[at];
                assert!(
                    (actual - spike).abs() > 1.0,
                    "{tag}: loud off-phase at={at} passed through"
                );
                let error = (actual - tone[at]).abs();
                worst_loud = worst_loud.max(error);
                assert!(
                    error < 0.15,
                    "{tag}: loud off-phase at={at} must repair, error={error}"
                );
            }
            report(&format!("{tag} loud-override"), worst_loud);
            let clean_damage = (repaired[quiet_frame + latency] - tone[quiet_frame]).abs();
            assert!(
                clean_damage < 0.05,
                "{tag}: clean on-phase damage={clean_damage}"
            );
            let mut true_hot = 0;
            let mut false_hot = 0;
            let mut missed = 0;
            for input in 32..frames {
                let hot = residual[input + latency].abs() > HOT_RESIDUAL;
                match (is_click[input], hot) {
                    (true, true) => true_hot += 1,
                    (false, true) => false_hot += 1,
                    (true, false) => missed += 1,
                    (false, false) => {}
                }
            }
            let total = true_hot + missed;
            let recall = true_hot as f32 / total as f32;
            let precision = true_hot as f32 / (true_hot + false_hot).max(1) as f32;
            assert!(
                recall >= 0.95,
                "{tag}: recall={recall} ({true_hot}/{total})"
            );
            assert!(
                precision >= 0.95,
                "{tag}: precision={precision} false_hot={false_hot}"
            );
        }
    }
}
