//! Canonical EOS processing and filter ownership oracles.

// Rust guideline compliant 2026-02-21
use crate::filters::XtcFilters;
use crate::realtime_tests::callback_counts;
use crate::types::PendingFilterUpdate;
use crate::{XtcPlugin, XtcPluginParams};
use rustfft::num_complex::Complex;
use sotf_host::{ParameterId, ParameterValue, Plugin, ProcessContext};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

const RATE: u32 = 48_000;

fn make(n: usize, auto_gain: bool) -> XtcPlugin {
    let mut plugin = XtcPlugin::new(
        XtcPluginParams {
            fft_size: n,
            auto_gain_enabled: auto_gain,
            ..Default::default()
        },
        RATE,
    )
    .unwrap();
    plugin.initialize(RATE).unwrap();
    plugin
}

fn input(plugin: &mut XtcPlugin, source: &[f32]) -> Vec<f32> {
    let frames = source.len() / 2;
    let mut output = vec![0.0; frames * plugin.output_channels()];
    plugin
        .process(source, &mut output, &ProcessContext::new(RATE, frames))
        .unwrap();
    output
}

fn drain(plugin: &mut XtcPlugin, capacities: &[usize]) -> Vec<f32> {
    let channels = plugin.output_channels();
    let mut output = Vec::new();
    for &capacity in capacities.iter().cycle().take(40_000) {
        let mut block = vec![123.0; capacity * channels];
        let result = plugin
            .drain(&mut block, &ProcessContext::new(RATE, 0))
            .unwrap();
        assert!(
            block[result.frames * channels..]
                .iter()
                .all(|v| *v == 123.0)
        );
        output.extend_from_slice(&block[..result.frames * channels]);
        if result.complete {
            return output;
        }
    }
    panic!("bounded XTC drain did not complete");
}

#[test]
fn nonidentity_matrix_matches_independent_f64_windowed_circular_fir() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    const N: usize = 128;
    const H: usize = 32;
    // A half-spectrum product is a circular convolution within each finite
    // analysis window. The reference uses direct f64 FIR sums, no plugin FFT,
    // coefficients, OLA storage, or scheduling helper.
    let routes = [
        [(0.6, 0), (0.1, 3)],
        [(-0.2, 7), (0.4, 1)],
        [(0.3, 2), (0.1, 9)],
        [(-0.1, 4), (0.2, 0)],
    ];
    let mut worst = 0.0_f64;
    for channels in [2, 4] {
        let path = std::env::temp_dir().join(format!(
            "sotf-xtc-eof-matrix-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let speakers: Vec<_> = (0..channels).map(|ch| format!("speaker{ch}")).collect();
        let mut filters = Vec::new();
        for (ch, routes) in routes.iter().take(channels).enumerate() {
            for (ear, &(gain, delay)) in routes.iter().enumerate() {
                let mut taps = vec![0.0; delay + 1];
                taps[delay] = gain;
                filters.push(serde_json::json!({"speaker":speakers[ch],"target_ear":format!("ear{ear}"),"taps":taps}));
            }
        }
        std::fs::write(&path,serde_json::to_vec(&serde_json::json!({"sample_rate":RATE,"speakers":speakers,"ears":["ear0","ear1"],"filters":filters})).unwrap()).unwrap();
        let mut plugin = XtcPlugin::new(
            XtcPluginParams {
                fft_size: N,
                auto_gain_enabled: false,
                source_mode: "roomeq_recommended".into(),
                recommended_matrix_file: Some(path.to_string_lossy().into_owned()),
                ..Default::default()
            },
            RATE,
        )
        .unwrap();
        plugin.initialize(RATE).unwrap();
        std::fs::remove_file(path).unwrap();
        for frames in [1, 17, 32, 33, 129] {
            plugin.reset();
            let source: Vec<_> = (0..frames * 2)
                .map(|i| ((i * 7 % 31) as i32 - 15) as f32 / 128.0)
                .collect();
            let mut actual = input(&mut plugin, &source);
            actual.extend(drain(&mut plugin, &[1, 17, 33]));
            let total_frames = 2 * N + (frames - 1) / H * H;
            assert_eq!(actual.len(), total_frames * channels);
            let mut expected = vec![0.0_f64; actual.len()];
            let window: Vec<_> = (0..N)
                .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / N as f64).cos())
                .collect();
            let last = (frames - 1) / H * H;
            for origin in (-(N as isize - H as isize)..=last as isize).step_by(H) {
                for offset in 0..N {
                    let time = origin + offset as isize;
                    if time < 0 {
                        continue;
                    }
                    let output_frame = N + time as usize;
                    for (ch, route) in routes.iter().take(channels).enumerate() {
                        let mut sum = 0.0;
                        for (ear, &(gain, delay)) in route.iter().enumerate() {
                            let wrapped = (offset + N - delay) % N;
                            let source_frame = origin + wrapped as isize;
                            if source_frame >= 0 && (source_frame as usize) < frames {
                                sum += gain
                                    * f64::from(source[source_frame as usize * 2 + ear])
                                    * window[wrapped];
                            }
                        }
                        expected[output_frame * channels + ch] += sum * window[offset] / 1.5;
                    }
                }
            }
            for (&a, &e) in actual.iter().zip(&expected) {
                let error = (f64::from(a) - e).abs();
                worst = worst.max(error);
                assert!(
                    error < 1.5e-6,
                    "channels={channels} frames={frames} actual={a} expected={e} error={error}"
                );
            }
        }
    }
    eprintln!("XTC f64 finite operator: cases=10 max_absolute_error={worst:e}");
}

fn primed_auto_gain() -> XtcPlugin {
    let mut plugin = make(128, true);
    // Prime actual meter and smoother state, then put the next fixed 100 ms
    // measurement boundary inside the first canonical drain refill.
    let ag = plugin.dynamics.auto_gain.as_mut().unwrap();
    ag.measure_input(&vec![0.1; 48_000]).unwrap();
    ag.measure_output(&vec![0.4; 48_000]).unwrap();
    ag.apply_compensation(&mut vec![0.4; 16_384], 8192);
    assert!(ag.current_gain_db().abs() > 0.01);
    plugin.dynamics.limiter_envelope = 0.37;
    for _ in 0..282 {
        input(&mut plugin, &[0.125; 34]);
    }
    input(&mut plugin, &[0.125; 10]);
    assert_eq!(plugin.diagnostics.auto_gain_frames, 4799);
    plugin
}

#[test]
fn canonical_cache_preserves_auto_gain_limiter_and_measurement_cadence() {
    let mut reference = primed_auto_gain();
    let hop = reference.fft.hop_size;
    let remaining = 2 * reference.fft.fft_size - hop + (hop - 4799 % hop) % hop;
    let mut expected = Vec::new();
    let mut todo = remaining;
    while todo > 0 {
        let frames = todo.min(reference.fft.hop_size);
        expected.extend(input(&mut reference, &vec![0.0; frames * 2]));
        todo -= frames;
    }
    for capacities in [&[1][..], &[17, 33, 3][..], &[32][..]] {
        let mut actual = primed_auto_gain();
        assert_eq!(drain(&mut actual, capacities), expected);
        assert_eq!(
            actual.diagnostics.auto_gain_frames,
            reference.diagnostics.auto_gain_frames
        );
        assert_eq!(
            actual.dynamics.limiter_envelope,
            reference.dynamics.limiter_envelope
        );
        assert_eq!(
            actual
                .dynamics
                .auto_gain
                .as_ref()
                .unwrap()
                .current_gain_db(),
            reference
                .dynamics
                .auto_gain
                .as_ref()
                .unwrap()
                .current_gain_db()
        );
    }
}

fn publication(plugin: &XtcPlugin, generation: u64, scale: f32) -> Arc<PendingFilterUpdate> {
    let bins = plugin.fft.fft_size / 2 + 1;
    Arc::new(PendingFilterUpdate {
        generation,
        filters: Arc::new(XtcFilters {
            filter_ll: vec![Complex::new(scale, 0.0); bins],
            filter_lr: vec![Complex::new(0.0, 0.0); bins],
            filter_rl: None,
            filter_rr: None,
            is_symmetric: true,
            speaker_filters: None,
        }),
        hrtf_transfer_functions: None,
        room_reflection_cache: None,
        room_params_hash: generation,
    })
}

fn enqueue(plugin: &XtcPlugin, update: Arc<PendingFilterUpdate>) {
    plugin
        .filter_state
        .filter_update_generation
        .store(update.generation, Ordering::Release);
    plugin.filter_state.exchange.lock().unwrap().pending = Some(update);
}

#[test]
fn eof_freezes_ready_and_late_publications_and_reset_adopts_in_a_new_epoch() {
    for fade in [0.001, 1.0] {
        for blocked_retirement in [false, true] {
            let mut actual = make(128, false);
            let mut reference = make(128, false);
            for plugin in [&mut actual, &mut reference] {
                let update = publication(plugin, 2, 0.5);
                enqueue(plugin, update);
                plugin.filter_state.progress_per_hop = fade;
                input(plugin, &[0.125; 34]);
            }
            let old = Arc::downgrade(actual.filter_state.prev_filters.as_ref().unwrap());
            let active = Arc::clone(&actual.filter_state.cached_current_filters);
            let ready = publication(&actual, 3, 0.25);
            enqueue(&actual, ready);
            let exchange = Arc::clone(&actual.filter_state.exchange);
            if blocked_retirement {
                let mut storage = exchange.lock().unwrap();
                storage.retired_updates[1] = Some(Arc::clone(
                    actual.filter_state.active_filter_update.as_ref().unwrap(),
                ));
                storage.retired_snapshots = [Some(Arc::clone(&active)), Some(Arc::clone(&active))];
            }
            let guard = blocked_retirement.then(|| exchange.lock().unwrap());
            // Finish the first canonical cache on a later call; publication may
            // arrive while the already produced samples are still unread.
            let mut first = [0.0; 2];
            actual
                .drain(&mut first, &ProcessContext::new(RATE, 0))
                .unwrap();
            drop(guard);
            let late = publication(&actual, 4, 0.75);
            enqueue(&actual, Arc::clone(&late));
            let mut tail = first.to_vec();
            tail.extend(drain(&mut actual, &[1, 7, 33]));
            assert_eq!(tail, drain(&mut reference, &[32]));
            assert!(Arc::ptr_eq(
                &active,
                &actual.filter_state.cached_current_filters
            ));
            assert!(old.upgrade().is_some());
            assert!(Arc::ptr_eq(
                actual
                    .filter_state
                    .exchange
                    .lock()
                    .unwrap()
                    .pending
                    .as_ref()
                    .unwrap(),
                &late
            ));
            // Control-side reclaim creates capacity; reset itself only transfers owners.
            let retired = {
                let mut e = exchange.lock().unwrap();
                (
                    std::mem::take(&mut e.retired_updates),
                    std::mem::take(&mut e.retired_snapshots),
                )
            };
            drop(retired);
            actual.reset();
            actual
                .process(&[], &mut [], &ProcessContext::new(RATE, 0))
                .unwrap();
            assert!(Arc::ptr_eq(
                &active,
                &actual.filter_state.cached_current_filters
            ));
            assert!(
                actual
                    .process(&[0.0; 2], &mut [0.0; 2], &ProcessContext::new(44_100, 1))
                    .is_err()
            );
            assert!(Arc::ptr_eq(
                &active,
                &actual.filter_state.cached_current_filters
            ));
            input(&mut actual, &[0.125; 2]);
            assert!(Arc::ptr_eq(
                &late.filters,
                &actual.filter_state.cached_current_filters
            ));
        }
    }
}

#[test]
fn cold_drain_measurement_retirement_reset_and_adoption_do_not_touch_the_heap() {
    for n in [128, 2048, 16384] {
        let mut plugin = make(n, true);
        // First drain happens on a fresh audio thread, with nine preceding
        // ordinary calls on the control thread and a live fade/pending result.
        for _ in 0..8 {
            input(&mut plugin, &[0.125; 34]);
        }
        enqueue(&plugin, publication(&plugin, 2, 0.5));
        plugin.filter_state.progress_per_hop = 1.0;
        input(&mut plugin, &[0.125; 34]);
        enqueue(&plugin, publication(&plugin, 3, 0.25));
        let pending = Arc::downgrade(
            plugin
                .filter_state
                .exchange
                .lock()
                .unwrap()
                .pending
                .as_ref()
                .unwrap(),
        );
        let mut destination = vec![0.0; n / 4 * 2];
        let scalar_id = ParameterId::from("auto_gain_enabled");
        let frames_id = ParameterId::from("fft_size");
        let (plugin, counts) = std::thread::spawn(move || {
            let counts = callback_counts(|| {
                assert!(plugin.drain_call_bound().is_some());
                // Retain the caller's interned ID owners outside the measured
                // setter calls, matching the host's cached parameter IDs.
                plugin
                    .drain(&mut destination[..2], &ProcessContext::new(RATE, 0))
                    .unwrap();
                plugin
                    .set_parameter(scalar_id.clone(), ParameterValue::Bool(true))
                    .unwrap();
                plugin
                    .set_parameter(frames_id.clone(), ParameterValue::Int(n as i32))
                    .unwrap();
                while !plugin
                    .drain(&mut destination, &ProcessContext::new(RATE, 0))
                    .unwrap()
                    .complete
                {}
                plugin.reset();
                plugin
                    .process(
                        &[0.125; 2],
                        &mut destination[..2],
                        &ProcessContext::new(RATE, 1),
                    )
                    .unwrap();
            });
            (plugin, counts)
        })
        .join()
        .unwrap();
        assert_eq!(counts, (0, 0), "N={n}");
        assert!(pending.upgrade().is_some());
        assert_eq!(
            plugin
                .filter_state
                .active_filter_update
                .as_ref()
                .unwrap()
                .generation,
            3
        );
    }
}
