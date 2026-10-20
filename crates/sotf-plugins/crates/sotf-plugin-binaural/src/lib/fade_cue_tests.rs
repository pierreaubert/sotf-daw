//! Independent time-domain references for repeatedly retargeted transfers.

use crate::types::BinauralState;
use crate::{BinauralDecoderPlugin, RoomModel};
use rustfft::num_complex::Complex;
use sotf_host::{Plugin, ProcessContext};
use std::sync::Arc;

fn kernel(id: usize, ear: usize) -> Vec<f64> {
    let mut taps = vec![0.0; 16];
    // Alternating polarity and distinct delays exercise phase as well as gain.
    let gain = if id.is_multiple_of(2) { 0.8 } else { -0.6 };
    let delay = (id + ear * 2) % 7;
    let eq_delay = id % 3;
    let eq_gain = 0.7 + ear as f64 * 0.2;
    taps[delay + eq_delay] = gain * eq_gain;
    taps[delay + eq_delay + 2] = gain * eq_gain * 0.2;
    taps
}

fn state(id: usize, n: usize) -> Arc<BinauralState> {
    let n = n * 2;
    let bins = n / 2 + 1;
    let mut filters = vec![Complex::new(0.0, 0.0); bins * 2];
    let mut eq = [
        vec![Complex::new(0.0, 0.0); bins],
        vec![Complex::new(0.0, 0.0); bins],
    ];
    for ear in 0..2 {
        let gain = if id.is_multiple_of(2) { 0.8 } else { -0.6 };
        for k in 0..bins {
            let w = -2.0 * std::f64::consts::PI * k as f64 / n as f64;
            let h = Complex::from_polar(gain, w * ((id + ear * 2) % 7) as f64)
                * (Complex::new(1.0, 0.0) + Complex::from_polar(0.2, w * 2.0));
            filters[ear * bins + k] = Complex::new(h.re as f32, h.im as f32);
            let e = Complex::from_polar(0.7 + ear as f64 * 0.2, w * (id % 3) as f64);
            eq[ear][k] = Complex::new(e.re as f32, e.im as f32);
        }
    }
    for ear in 0..2 {
        for k in 0..bins {
            filters[ear * bins + k] *= eq[ear][k];
        }
    }
    Arc::new(BinauralState {
        hrtf_filters_freq: vec![filters],
        design_diffuse_field_eq: None,
        reflection_gains: None,
        _hrtf_data: None,
    })
}

#[test]
fn repeated_linear_publications_match_direct_time_domain_transfer() {
    for rate in [44_100, 48_000, 96_000] {
        for n in [64, 256, 1024] {
            for shape in 0..3 {
                let mut p = BinauralDecoderPlugin::new(
                    1,
                    n,
                    None,
                    1.0,
                    0.0,
                    false,
                    120.0,
                    2.0,
                    0.0,
                    RoomModel {
                        max_order: 0,
                        ..Default::default()
                    },
                );
                p.initialize(f64::from(rate)).unwrap();
                // This transfer oracle controls retirement capacity explicitly;
                // scheduler timing must not postpone any of its publications.
                p.retirement.tx.take();
                p.retirement.thread.take().unwrap().join().unwrap();
                let (tx, _retired) = std::sync::mpsc::sync_channel(512);
                p.retirement.tx = Some(tx);
                p.config.crossfade_ms = 10.0;
                p.config.late_reverb_enabled = false;
                let initial = state(0, n);
                p.state.store(Arc::clone(&initial));
                p.crossfade.current_state_snapshot = initial;
                let hop = p.config.hop_size;
                let fade_hops = ((rate as f32 * 0.010) as usize).div_ceil(hop);
                let total_hops = 100 + fade_hops * 2;
                let mut reference = vec![0.0; (total_hops * hop + n + 32) * 2];
                let mut recorded = Vec::new();
                let mut old = [kernel(0, 0), kernel(0, 1)];
                let mut target = old.clone();
                let mut progress = fade_hops;
                let mut maximum = 0.0_f64;
                for frame in 0..total_hops {
                    // Fast, then slower updates exercise repeated effective blends,
                    // completion, and the ordinary non-interrupted adoption path.
                    if frame < 40 || (frame < 90 && frame % 7 == 0) {
                        let alpha = progress.min(fade_hops) as f64 / fade_hops as f64;
                        for ear in 0..2 {
                            for tap in 0..16 {
                                old[ear][tap] =
                                    old[ear][tap] * (1.0 - alpha) + target[ear][tap] * alpha;
                            }
                        }
                        let id = frame + 1;
                        target = [kernel(id, 0), kernel(id, 1)];
                        progress = 0;
                        p.state.store(state(id, n));
                    }
                    let alpha = progress.min(fade_hops) as f64 / fade_hops as f64;
                    for ear in 0..2 {
                        for tap in 0..16 {
                            reference[((frame * hop + n + tap) * 2) + ear] +=
                                0.25 * (old[ear][tap] * (1.0 - alpha) + target[ear][tap] * alpha);
                        }
                    }
                    progress += 1;
                    let mut input = vec![0.0; hop];
                    input[0] = 0.25;
                    let mut output = vec![0.0; hop * 2];
                    let mut offset = 0;
                    let mut part = 0;
                    while offset < hop {
                        let chunk = match shape {
                            0 => hop,
                            1 => 1,
                            _ => [3, 17, 5][part % 3],
                        }
                        .min(hop - offset);
                        p.process(
                            &input[offset..offset + chunk],
                            &mut output[offset * 2..(offset + chunk) * 2],
                            &ProcessContext::new(rate, chunk),
                        )
                        .unwrap();
                        offset += chunk;
                        part += 1;
                    }
                    recorded.extend(output);
                }
                for (actual, expected) in recorded.iter().zip(&reference) {
                    maximum = maximum.max((f64::from(*actual) - expected).abs());
                }
                println!(
                    "INTERRUPTED_LINEAR rate={rate} fft={n} shape={shape} max_error={maximum}"
                );
                assert!(
                    maximum < 2.0e-5,
                    "interrupted transfer {rate}/{n}/{shape}: {maximum}"
                );
            }
        }
    }
}

#[test]
fn spectral_publications_preserve_active_phase_history_and_coalesce_latest() {
    for rate in [44_100, 48_000, 96_000] {
        for n in [64, 256, 1024] {
            let create = || {
                let params = serde_json::from_value(serde_json::json!({
                    "input_channels": 1, "fft_size": n, "crossfade_mode": 1,
                    "crossfade_ms": 10.0, "late_reverb_enabled": false,
                    "room_model": {"max_order": 0}
                }))
                .unwrap();
                let mut p = BinauralDecoderPlugin::try_from_params(params).unwrap();
                p.initialize(f64::from(rate)).unwrap();
                let first = state(0, n);
                p.state.store(Arc::clone(&first));
                p.crossfade.current_state_snapshot = first;
                p
            };
            let mut tested = create();
            let mut reference = create();
            let hop = tested.config.hop_size;
            let hops = ((rate as f32 * 0.010) as usize).div_ceil(hop);
            let destination = state(1, n);
            tested.state.store(Arc::clone(&destination));
            reference.state.store(Arc::clone(&destination));
            let mut latest = Arc::clone(&destination);
            for frame in 0..hops {
                if frame > 0 {
                    latest = state(frame + 2, n);
                    tested.state.store(Arc::clone(&latest));
                }
                let input: Vec<_> = (0..hop)
                    .map(|i| {
                        let index = frame * hop + i;
                        (index as f32 * 0.13).sin() * 0.1 + (index as f32 * 0.31).cos() * 0.07
                    })
                    .collect();
                let mut actual = vec![0.0; hop * 2];
                let mut expected = vec![0.0; hop * 2];
                let context = ProcessContext::new(rate, hop);
                tested.process(&input, &mut actual, &context).unwrap();
                reference.process(&input, &mut expected, &context).unwrap();
                assert_eq!(
                    actual, expected,
                    "spectral phase history {rate}/{n}/{frame}"
                );
                assert!(Arc::ptr_eq(
                    &tested.crossfade.current_state_snapshot,
                    &destination
                ));
            }
            assert_eq!(tested.crossfade.crossfade_remaining, 0);
            tested
                .process(
                    &vec![0.0; hop],
                    &mut vec![0.0; hop * 2],
                    &ProcessContext::new(rate, hop),
                )
                .unwrap();
            assert!(Arc::ptr_eq(
                &tested.crossfade.current_state_snapshot,
                &latest
            ));
            println!("INTERRUPTED_SPECTRAL rate={rate} fft={n} coalesced_after_hops={hops}");
        }
    }
}
