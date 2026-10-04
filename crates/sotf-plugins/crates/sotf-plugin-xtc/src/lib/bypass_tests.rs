//! Prepared bypass lifecycle, dynamics and real-time ownership regressions.
// Rust guideline compliant 2026-02-21
use crate::initialize_tests::MatrixFile;
use crate::realtime_tests::callback_counts;
use crate::{XtcPlugin, XtcPluginParams};
use sotf_host::{ParameterId, ParameterValue, Plugin, ProcessContext, TailLength};

fn make(n: usize, rate: u32, enabled: bool, auto_gain: bool) -> XtcPlugin {
    let mut p = XtcPlugin::new(
        XtcPluginParams {
            fft_size: n,
            enabled,
            auto_gain_enabled: auto_gain,
            ..Default::default()
        },
        rate,
    )
    .unwrap();
    p.initialize(f64::from(rate)).unwrap();
    p
}
fn enabled(p: &mut XtcPlugin, value: bool) {
    p.set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(value))
        .unwrap();
}
fn render(p: &mut XtcPlugin, source: &[f32]) -> Vec<f32> {
    let mut output = vec![0.0; source.len() / 2 * p.output_channels()];
    p.process(
        source,
        &mut output,
        &ProcessContext::new(p.fft.sample_rate, source.len() / 2),
    )
    .unwrap();
    output
}

#[test]
fn convex_transition_is_finite_for_opposite_f32_extremes_and_has_exact_endpoints() {
    let mut p = make(128, 48000, false, false);
    for mix in [0.0, 0.25, 0.5, 0.75, 1.0] {
        p.bypass.mix = mix;
        p.bypass.remaining = 0;
        p.bypass.position = 0;
        p.bypass.dry[..2].copy_from_slice(&[f32::MAX, -f32::MAX]);
        let mut wet = [-f32::MAX, f32::MAX];
        p.bypass.apply(&[0.0; 2], &mut wet, 2, false);
        assert!(wet.iter().all(|v| v.is_finite()));
        assert_eq!(
            wet[0],
            ((1.0 - mix) * f64::from(f32::MAX) - mix * f64::from(f32::MAX)) as f32
        );
        assert_eq!(wet[1], -wet[0]);
    }
}

#[test]
fn ramp_frames_snapshots_reversal_and_reset_follow_the_sample_clock() {
    for rate in [44100, 48000, 96000, 192000] {
        let mut p = make(128, rate, true, false);
        let frames = (rate as usize + 50) / 100;
        enabled(&mut p, false);
        assert_eq!(p.bypass.remaining, frames);
        for i in 1..frames {
            render(&mut p, &[0.0; 2]);
            enabled(&mut p, false);
            assert_eq!(p.bypass.remaining, frames - i);
            assert!((p.bypass.mix - (1.0 - i as f64 / frames as f64)).abs() < 1e-12);
        }
        render(&mut p, &[0.0; 2]);
        assert_eq!(p.bypass.mix, 0.0);
        assert_eq!(p.bypass.remaining, 0);
        enabled(&mut p, true);
        render(&mut p, &vec![0.0; (frames / 3) * 2]);
        let start = p.bypass.mix;
        enabled(&mut p, false);
        render(&mut p, &vec![0.0; (frames - 1) * 2]);
        assert!((p.bypass.mix - start / frames as f64).abs() < 1e-12);
        render(&mut p, &[0.0; 2]);
        assert_eq!(p.bypass.mix, 0.0);
        enabled(&mut p, true);
        p.reset();
        assert_eq!(p.bypass.mix, 1.0);
        assert_eq!(p.bypass.remaining, 0);
        assert!(p.bypass.dry.iter().all(|&v| v == 0.0));
        assert_eq!(
            p.get_parameter(&ParameterId::from("enabled")),
            Some(ParameterValue::Bool(true))
        );
        assert_eq!(
            p.parameters()
                .iter()
                .find(|v| v.id.as_str() == "enabled")
                .unwrap()
                .default_value,
            ParameterValue::Bool(true)
        );
    }
}

#[test]
fn disabled_wet_autogain_limiter_and_filter_history_stay_warm() {
    let mut dry = make(128, 48000, false, true);
    let mut wet = make(128, 48000, true, true);
    for p in [&mut dry, &mut wet] {
        let ag = p.dynamics.auto_gain.as_mut().unwrap();
        ag.measure_input(&vec![0.1; 48000]).unwrap();
        ag.measure_output(&vec![0.4; 48000]).unwrap();
        ag.apply_compensation(&mut vec![0.4; 16384], 8192);
        assert!(ag.current_gain_db().abs() > 0.01);
        p.dynamics.limiter_envelope = 0.37;
    }
    let mut full_source = Vec::new();
    let mut full_dry = Vec::new();
    for block in 0..30 {
        let source: Vec<_> = (0..274)
            .map(|i| ((i + block * 274) % 127) as f32 / 32.0 - 2.0)
            .collect();
        full_source.extend_from_slice(&source);
        full_dry.extend(render(&mut dry, &source));
        render(&mut wet, &source);
        assert_eq!(dry.dynamics.limiter_envelope, wet.dynamics.limiter_envelope);
        let d = dry.dynamics.auto_gain.as_ref().unwrap().get_data();
        let w = wet.dynamics.auto_gain.as_ref().unwrap().get_data();
        assert_eq!(
            (
                d.enabled,
                d.gain_db,
                d.input_lufs,
                d.output_lufs,
                d.input_peak,
                d.output_peak
            ),
            (
                w.enabled,
                w.gain_db,
                w.input_lufs,
                w.output_lufs,
                w.input_peak,
                w.output_peak
            )
        );
        assert_eq!(
            dry.dynamics.auto_gain.as_ref().unwrap().current_gain_db(),
            wet.dynamics.auto_gain.as_ref().unwrap().current_gain_db()
        );
        assert_eq!(dry.input.input_buffer_l, wet.input.input_buffer_l);
        assert_eq!(dry.output.output_accumulator, wet.output.output_accumulator);
    }
    assert_eq!(&full_dry[256..], &full_source[..full_source.len() - 256]);
    enabled(&mut dry, true);
    let source = vec![0.125; 960 * 2];
    let actual = render(&mut dry, &source);
    let reference = render(&mut wet, &source);
    assert_eq!(&actual[480 * 2..], &reference[480 * 2..]);
}

#[test]
fn disabled_and_transition_eof_keep_canonical_waveform_bound_and_snapshot_contract() {
    for n in [128, 2048] {
        for rate in [44100, 48000, 96000] {
            let ramp = (rate as usize + 50) / 100;
            // Initial dry, settled dry, both fades, completed wet, and a fade
            // ending inside a refill (uniform metadata must remain latched).
            for (initial, target, after) in [
                (false, false, 17),
                (true, false, ramp),
                (true, false, 1),
                (false, true, 1),
                (false, true, ramp),
                (true, false, ramp - 1),
            ] {
                for capacity in [1, 17, n / 4, n / 4 + 1] {
                    let mut actual = make(n, rate, initial, true);
                    let mut reference = make(n, rate, initial, true);
                    for p in [&mut actual, &mut reference] {
                        render(p, &vec![0.125; (n + 17) * 2]);
                        enabled(p, target);
                        render(p, &vec![-0.0625; after * 2]);
                    }
                    let settled = !target && (!initial || after >= ramp);
                    let count = if settled {
                        n
                    } else {
                        2 * n - n / 4 + (n / 4 - ((n + 17 + after) % (n / 4))) % (n / 4)
                    };
                    let bound = TailLength::Finite(if settled {
                        n as u64
                    } else {
                        (2 * n - 1) as u64
                    });
                    assert_eq!(actual.tail_length(), bound);
                    let mut expected = Vec::new();
                    let mut left = count;
                    while left > 0 {
                        let frames = left.min(n / 4);
                        expected.extend(render(&mut reference, &vec![0.0; frames * 2]));
                        left -= frames;
                    }
                    let mut got = Vec::new();
                    let mut output = vec![987.0; capacity * 2];
                    loop {
                        let step = actual
                            .drain(&mut output, &ProcessContext::new(rate, 0))
                            .unwrap();
                        got.extend_from_slice(&output[..step.frames * 2]);
                        assert!(output[step.frames * 2..].iter().all(|&v| v == 987.0));
                        output.fill(987.0);
                        assert_eq!(actual.tail_length(), bound);
                        if step.complete {
                            break;
                        }
                    }
                    assert_eq!(got.len(), count * 2);
                    assert_eq!(got, expected);
                    enabled(&mut actual, target);
                    assert!(
                        actual
                            .set_parameter(
                                ParameterId::from("enabled"),
                                ParameterValue::Bool(!target)
                            )
                            .is_err()
                    );
                    assert!(
                        actual
                            .process(&[0.0; 2], &mut output[..2], &ProcessContext::new(rate, 1))
                            .is_err()
                    );
                    assert_eq!(actual.drain_call_bound().unwrap().get(), 1);
                    actual.reset();
                    assert_eq!(
                        actual.tail_length(),
                        TailLength::Finite(if target { (2 * n - 1) as u64 } else { n as u64 })
                    );
                }
            }
        }
    }
}

#[test]
fn dry_and_fading_preflight_errors_leave_audio_and_eof_state_unchanged() {
    for initial in [false, true] {
        let mut actual = make(128, 48000, initial, false);
        let mut reference = make(128, 48000, initial, false);
        assert!(
            actual
                .drain(&mut [], &ProcessContext::new(48000, 0))
                .unwrap()
                .complete
        );
        assert!(actual.drain_state.remaining.is_none());
        for p in [&mut actual, &mut reference] {
            render(p, &[0.125; 34]);
            enabled(p, !initial);
            render(p, &[0.25; 2]);
        }
        for (len, rate) in [(0, 48000), (3, 48000), (4, 44100)] {
            let mut output = [987.0; 4];
            assert!(
                actual
                    .drain(&mut output[..len], &ProcessContext::new(rate, 0))
                    .is_err()
            );
            assert_eq!(output, [987.0; 4]);
            assert!(actual.drain_state.remaining.is_none());
            assert!(actual.drain_state.tail_bound.is_none());
        }
        let mut out = [987.0; 4];
        assert!(
            actual
                .process(&[0.0; 3], &mut out, &ProcessContext::new(48000, 2))
                .is_err()
        );
        assert_eq!(out, [987.0; 4]);
        assert_eq!(
            render(&mut actual, &[0.0625; 2048]),
            render(&mut reference, &[0.0625; 2048])
        );
        // A partial cache requires one separate call before full-hop refills.
        let mut first = [0.0; 2];
        actual
            .drain(&mut first, &ProcessContext::new(48000, 0))
            .unwrap();
        let bound = actual.drain_call_bound().unwrap().get();
        let mut count = 0;
        let mut output = [0.0; 64];
        loop {
            count += 1;
            if actual
                .drain(&mut output, &ProcessContext::new(48000, 0))
                .unwrap()
                .complete
            {
                break;
            }
        }
        assert!(count <= bound);
    }
}

#[test]
fn cold_bool_automation_dry_wet_drain_and_reset_have_no_allocations_or_frees() {
    for channels in [2, 4] {
        let file = MatrixFile::new(48000, channels);
        for initial in [false, true] {
            for auto_gain in [false, true] {
                let mut params = file.params();
                params.enabled = initial;
                params.auto_gain_enabled = auto_gain;
                let mut p = XtcPlugin::new(params, 48000).unwrap();
                p.initialize(48000).unwrap();
                let id = ParameterId::from("enabled");
                let input = vec![0.125; 2048];
                let mut output = vec![0.0; 1024 * channels];
                let (p, counts) = std::thread::spawn(move || {
                    let counts = callback_counts(|| {
                        for _ in 0..2 {
                            for target in [initial, !initial, initial] {
                                p.set_parameter(id.clone(), ParameterValue::Bool(target))
                                    .unwrap();
                                p.set_parameter(id.clone(), ParameterValue::Bool(target))
                                    .unwrap();
                                p.process(&input, &mut output, &ProcessContext::new(48000, 1024))
                                    .unwrap();
                            }
                            p.drain(&mut output[..channels], &ProcessContext::new(48000, 0))
                                .unwrap();
                            let bound = p.drain_call_bound().unwrap().get();
                            let mut calls = 0;
                            loop {
                                calls += 1;
                                if p.drain(
                                    &mut output[..32 * channels],
                                    &ProcessContext::new(48000, 0),
                                )
                                .unwrap()
                                .complete
                                {
                                    break;
                                }
                            }
                            assert!(calls <= bound);
                            p.set_parameter(id.clone(), ParameterValue::Bool(initial))
                                .unwrap();
                            p.reset();
                        }
                    });
                    (p, counts)
                })
                .join()
                .unwrap();
                assert_eq!(
                    counts,
                    (0, 0),
                    "channels={channels}, initial={initial}, AG={auto_gain}"
                );
                drop(p);
            }
        }
    }
}

#[test]
fn disabled_filter_fade_completes_without_destroying_owners_on_the_callback() {
    use std::sync::{Arc, atomic::Ordering};
    let mut p = make(128, 48000, false, true);
    let old = Arc::downgrade(&p.filter_state.cached_current_filters);
    let filters = Arc::new(crate::filters::compute_xtc_filters_full(
        &p.params, 48000, 65,
    ));
    let update = Arc::new(crate::types::PendingFilterUpdate {
        generation: 2,
        filters,
        hrtf_transfer_functions: None,
        room_reflection_cache: None,
        room_params_hash: 0,
    });
    p.filter_state
        .filter_update_generation
        .store(2, Ordering::Release);
    p.filter_state.exchange.lock().unwrap().pending = Some(update);
    p.filter_state.progress_per_hop = 1.0;
    let source = vec![0.125; 2048];
    let mut output = vec![0.0; 2048];
    let (p, counts) = std::thread::spawn(move || {
        let counts = callback_counts(|| {
            p.process(&source, &mut output, &ProcessContext::new(48000, 1024))
                .unwrap();
        });
        (p, counts)
    })
    .join()
    .unwrap();
    assert_eq!(counts, (0, 0));
    assert_eq!(p.filter_state.crossfade_progress, 1.0);
    assert!(p.filter_state.prev_filters.is_none());
    assert!(old.upgrade().is_some());
    assert_eq!(
        p.filter_state
            .active_filter_update
            .as_ref()
            .unwrap()
            .generation,
        2
    );
    // Destruction belongs to the control side after the measured callback.
    drop(p);
    assert!(old.upgrade().is_none());
}
