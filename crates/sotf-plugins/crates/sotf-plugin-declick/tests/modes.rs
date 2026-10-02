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
            report(&format!("mode={mode} bands={bands} repair-error"), worst_repair);
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
        report(&format!("multiband-periodic bands={bands} repair-error"), worst);
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
                batch.insert(
                    ParameterId::from("sensitivity"),
                    ParameterValue::Float(9.0),
                );
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
