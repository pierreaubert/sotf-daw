//! Head-relative reflection cues through control and worker preparation.

use crate::room::ReflectionHrtf;
use crate::{BinauralDecoderPlugin, RoomModel};
use realfft::RealFftPlanner;
use sotf_host::sofa::SourcePosition;
use sotf_host::{Plugin, ProcessContext};

fn make(
    rate: u32,
    sofa: bool,
    head: f32,
) -> (
    BinauralDecoderPlugin,
    Option<crate::head_cue_tests::Fixture>,
) {
    let file = sofa.then(|| crate::head_cue_tests::sofa(rate, 36, 0.35));
    let room = RoomModel {
        dimensions: [6.0, 7.0, 3.2],
        listener_position: [2.5, 2.7, 1.1],
        absorption: [1.0, 1.0, 0.0, 1.0, 1.0, 1.0],
        max_order: 1,
        speed_of_sound: 343.0,
    };
    let mut p = BinauralDecoderPlugin::new(
        1,
        256,
        file.as_ref().map(|f| f.path.clone()),
        1.0,
        0.0,
        false,
        120.0,
        2.0,
        0.0,
        room,
    );
    p.smoothing.head_yaw_deg.reset(head);
    p.initialize(f64::from(rate)).unwrap();
    p.config.crossfade_ms = 10.0;
    p.hrtf_update_tx.take();
    if let Some(worker) = p.hrtf_update_thread.take() {
        worker.join().unwrap();
    }
    (p, file)
}

fn expected(p: &BinauralDecoderPlugin, head: f32) -> (usize, [f32; 2]) {
    let r = p.room.cached_reflections[0]
        .iter()
        .find(|r| r.gain > 0.0)
        .unwrap();
    let target = SourcePosition::new(r.azimuth_deg - head, r.elevation_deg, 1.0);
    let state = p.state.load_full();
    let ear = if let Some(sofa) = &state._hrtf_data {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(256);
        let (near, weights) = crate::hrtf::find_hrtf_neighbors(sofa, &target);
        let (l, r) = crate::hrtf::interpolate_hrtf_frequency_domain(
            &near,
            &weights,
            sofa,
            256,
            p.config.sample_rate,
            &fft,
            0.0,
            target.azimuth,
            target.elevation,
        );
        let h = ReflectionHrtf::from_freq_domain(l, r);
        [h.left_gain_broadband, h.right_gain_broadband]
    } else {
        let y = f64::from(target.azimuth).to_radians().sin();
        [
            ((1.0 + y) * 0.5).sqrt() as f32,
            ((1.0 - y) * 0.5).sqrt() as f32,
        ]
    };
    (r.delay_samples, [r.gain * ear[0], r.gain * ear[1]])
}

fn pulse(p: &mut BinauralDecoderPlugin, rate: u32, warm: usize) -> Vec<f32> {
    let mut result = vec![0.0; warm * 2];
    if warm > 0 {
        p.process(
            &vec![0.0; warm],
            &mut result,
            &ProcessContext::new(rate, warm),
        )
        .unwrap();
    }
    let mut first = [0.0; 2];
    p.process(&[0.25], &mut first, &ProcessContext::new(rate, 1))
        .unwrap();
    result.extend(first);
    loop {
        let mut out = [0.0; 14];
        let drain = p.drain(&mut out, &ProcessContext::new(rate, 7)).unwrap();
        result.extend_from_slice(&out[..drain.frames * 2]);
        if drain.complete {
            break;
        }
    }
    result
}

#[test]
fn held_head_pose_rotates_native_and_fallback_reflection_audio() {
    for rate in [44100, 48000, 96000] {
        for sofa in [false, true] {
            for head in [-90.0, 0.0, 90.0] {
                let (mut p, _file) = make(rate, sofa, head);
                let (delay, gain) = expected(&p, head);
                let audio = pulse(&mut p, rate, 0);
                let index = (p.latency_samples() + delay) * 2;
                let error = (audio[index] - 0.25 * gain[0])
                    .abs()
                    .max((audio[index + 1] - 0.25 * gain[1]).abs());
                println!("REFLECTION_HELD rate={rate} sofa={sofa} head={head} max_error={error}");
                assert!(
                    error < 2.0e-5,
                    "held reflection head pose {rate}/{sofa}/{head}: {error}"
                );
            }
        }
    }
}

#[test]
fn worker_prepared_pose_updates_reflection_audio_with_direct_filters() {
    for rate in [44100, 48000, 96000] {
        for head in [-90.0, 90.0] {
            let (mut p, _file) = make(rate, true, 0.0);
            let (delay, gain) = expected(&p, head);
            let base = p.state.load_full();
            let sofa = base._hrtf_data.as_ref().unwrap();
            let fft = RealFftPlanner::<f32>::new().plan_fft_forward(256);
            let prepared = crate::hrtf::prepare_hrtf_spectra(sofa, 256, f64::from(rate), &fft);
            let state = BinauralDecoderPlugin::compute_head_rotated_hrtf_state(
                &base,
                &p.config,
                Some(&prepared),
                &p.fft,
                &[],
                (head, 0.0, 0.0),
            )
            .unwrap();
            p.state.store(state);
            let warm = (rate as usize / 50 + 1024).div_ceil(p.config.hop_size) * p.config.hop_size;
            let audio = pulse(&mut p, rate, warm);
            let index = (warm + p.latency_samples() + delay) * 2;
            let error = (audio[index] - 0.25 * gain[0])
                .abs()
                .max((audio[index + 1] - 0.25 * gain[1]).abs());
            println!("REFLECTION_WORKER rate={rate} head={head} max_error={error}");
            assert!(
                error < 2.0e-5,
                "worker reflection pose {rate}/{head}: {error}"
            );
        }
    }
}

fn prepared_pose(
    p: &BinauralDecoderPlugin,
    head: f32,
) -> std::sync::Arc<crate::types::BinauralState> {
    let base = p.state.load_full();
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(p.config.fft_size);
    let prepared = base._hrtf_data.as_ref().map(|sofa| {
        crate::hrtf::prepare_hrtf_spectra(sofa, p.config.fft_size, p.config.sample_rate, &fft)
    });
    BinauralDecoderPlugin::compute_head_rotated_hrtf_state(
        &base,
        &p.config,
        prepared.as_ref(),
        &p.fft,
        &[],
        (head, 0.0, 0.0),
    )
    .unwrap()
}

#[test]
fn broadband_cache_matches_full_complex_reflection_energy_without_heap_activity() {
    use crate::realtime_tests::callback_counts;
    for rate in [44100, 48000, 96000] {
        for n in [64, 256, 1024] {
            let (_file, sofa) = crate::linear_support_tests::fixture(rate, 8);
            let fft = RealFftPlanner::<f32>::new().plan_fft_forward(n);
            let prepared = crate::hrtf::prepare_hrtf_spectra(&sofa, n, f64::from(rate), &fft);
            for az in [-150.0, -90.0, -30.0, 0.0, 15.0, 45.0, 75.0, 90.0, 150.0] {
                let target = SourcePosition::new(az, 12.0, 1.0);
                let (near, weights) = crate::hrtf::find_hrtf_neighbors(&sofa, &target);
                let (left, right) = crate::hrtf::interpolate_hrtf_prepared(
                    &near,
                    &weights,
                    &prepared,
                    n,
                    f64::from(rate),
                    0.0,
                    az,
                    12.0,
                );
                let reference = ReflectionHrtf::from_freq_domain(left, right);
                let mut actual = [0.0; 2];
                let counts = callback_counts(|| {
                    actual =
                        crate::hrtf::interpolate_hrtf_broadband_prepared(&near, &weights, &prepared)
                });
                assert_eq!(counts, (0, 0));
                assert!((actual[0] - reference.left_gain_broadband).abs() < 2.0e-5);
                assert!((actual[1] - reference.right_gain_broadband).abs() < 2.0e-5);
            }
        }
    }
}

#[test]
fn reflection_reload_and_clear_reprepare_the_same_held_pose() {
    use sotf_host::ParameterValue;
    for rate in [44100, 48000, 96000] {
        for head in [-90.0, 90.0] {
            let (mut p, _first) = make(rate, true, head);
            let second = crate::head_cue_tests::sofa(rate, 36, -0.2);
            for path in [second.path.to_string_lossy().into_owned(), String::new()] {
                p.reset();
                p.set_parameter("hrtf_file".into(), ParameterValue::String(path))
                    .unwrap();
                p.shutdown_hrtf_update_thread();
                let (delay, gain) = expected(&p, head);
                let warm =
                    (rate as usize / 50 + 1024).div_ceil(p.config.hop_size) * p.config.hop_size;
                let audio = pulse(&mut p, rate, warm);
                let index = (warm + p.latency_samples() + delay) * 2;
                assert!((audio[index] - 0.25 * gain[0]).abs() < 2.0e-5);
                assert!((audio[index + 1] - 0.25 * gain[1]).abs() < 2.0e-5);
            }
        }
    }
}

#[test]
fn interrupted_reflection_fades_follow_output_source_hops_across_partitions_and_eof() {
    use crate::realtime_tests::callback_counts;
    for rate in [44100, 48000, 96000] {
        for sofa in [false, true] {
            for shape in 0..3 {
                let (mut wet, _file) = make(rate, sofa, 0.0);
                let (mut dry, _dry_file) = make(rate, sofa, 0.0);
                dry.smoothing.externalization.reset(0.0);
                // Keep retirement scheduling outside this deterministic transfer oracle.
                let mut receivers = Vec::new();
                for p in [&mut wet, &mut dry] {
                    p.retirement.tx.take();
                    p.retirement.thread.take().unwrap().join().unwrap();
                    let (tx, rx) = std::sync::mpsc::sync_channel(256);
                    p.retirement.tx = Some(tx);
                    receivers.push(rx);
                }
                let hop = wet.config.hop_size;
                let total_hops = 260;
                let input: Vec<f32> = (0..total_hops * hop + 3)
                    .map(|i| (i as f32 * 0.071).sin() * 0.04 + ((i * 37 % 17) as f32 - 8.0) * 0.001)
                    .collect();
                let updates = [
                    (4, 90.0),
                    (6, -90.0),
                    (7, 45.0),
                    (12, -30.0),
                    (60, 60.0),
                    (total_hops - 2, -75.0),
                ];
                let (delay, initial) = expected(&wet, 0.0);
                let fade_hops = ((rate as f32 * 0.010) as usize).div_ceil(hop);
                let mut reference_gains = Vec::new();
                let mut old = initial.map(f64::from);
                let mut target = old;
                let mut progress = fade_hops;
                for frame in 0..total_hops + 32 {
                    if let Some((_, head)) = updates.iter().find(|(h, _)| *h == frame) {
                        let alpha = progress.min(fade_hops) as f64 / fade_hops as f64;
                        old = std::array::from_fn(|ear| {
                            old[ear] * (1.0 - alpha) + target[ear] * alpha
                        });
                        target = expected(&wet, *head).1.map(f64::from);
                        progress = 0;
                    }
                    let alpha = progress.min(fade_hops) as f64 / fade_hops as f64;
                    reference_gains.push(std::array::from_fn::<_, 2, _>(|ear| {
                        old[ear] * (1.0 - alpha) + target[ear] * alpha
                    }));
                    progress += 1;
                }
                let mut actual_wet = Vec::new();
                let mut actual_dry = Vec::new();
                let mut position = 0;
                let mut part = 0;
                while position < input.len() {
                    if position.is_multiple_of(hop)
                        && let Some((_, head)) = updates.iter().find(|(h, _)| *h == position / hop)
                    {
                        let a = prepared_pose(&wet, *head);
                        let b = prepared_pose(&dry, *head);
                        wet.state.store(a);
                        dry.state.store(b);
                    }
                    let boundary = updates
                        .iter()
                        .map(|(h, _)| h * hop)
                        .find(|b| *b > position)
                        .unwrap_or(input.len());
                    let frames = match shape {
                        0 => input.len(),
                        1 => 1,
                        _ => [3, 17, 5, hop + 1][part % 4],
                    }
                    .min(boundary - position)
                    .min(input.len() - position);
                    for (p, recorded) in [(&mut wet, &mut actual_wet), (&mut dry, &mut actual_dry)]
                    {
                        let mut out = vec![0.0; frames * 2];
                        assert_eq!(
                            callback_counts(|| {
                                p.process(
                                    &input[position..position + frames],
                                    &mut out,
                                    &ProcessContext::new(rate, frames),
                                )
                                .unwrap();
                            }),
                            (0, 0)
                        );
                        recorded.extend(out);
                    }
                    position += frames;
                    part += 1;
                }
                let late_wet = prepared_pose(&wet, 135.0);
                let late_dry = prepared_pose(&dry, 135.0);
                let mut first = true;
                loop {
                    let frames = match shape {
                        0 => hop,
                        1 => 1,
                        _ => 7,
                    };
                    let mut results = Vec::new();
                    for (p, recorded) in [(&mut wet, &mut actual_wet), (&mut dry, &mut actual_dry)]
                    {
                        let mut out = vec![0.0; frames * 2];
                        let mut result = None;
                        assert_eq!(
                            callback_counts(|| result = Some(
                                p.drain(&mut out, &ProcessContext::new(rate, frames))
                                    .unwrap()
                            )),
                            (0, 0)
                        );
                        let r = result.unwrap();
                        recorded.extend_from_slice(&out[..r.frames * 2]);
                        results.push((r.frames, r.complete));
                    }
                    assert_eq!(results[0], results[1]);
                    if first {
                        wet.state.store(std::sync::Arc::clone(&late_wet));
                        dry.state.store(std::sync::Arc::clone(&late_dry));
                        first = false;
                    }
                    if results[0].1 {
                        break;
                    }
                }
                let mut maximum = 0.0_f64;
                for (i, (a, b)) in actual_wet.iter().zip(&actual_dry).enumerate() {
                    let frame = i / 2;
                    let ear = i % 2;
                    let expected = if frame >= 256 + delay {
                        let clock = frame - 256;
                        f64::from(input.get(clock - delay).copied().unwrap_or(0.0))
                            * reference_gains[clock / hop][ear]
                    } else {
                        0.0
                    };
                    maximum = maximum.max((f64::from(*a) - f64::from(*b) - expected).abs());
                }
                println!(
                    "REFLECTION_MOTION rate={rate} sofa={sofa} shape={shape} maximum_error={maximum}"
                );
                assert!(
                    maximum < 2.0e-5,
                    "moving reflection {rate}/{sofa}/{shape}: {maximum}"
                );
                drop(receivers);
            }
        }
    }
}

#[test]
fn reflection_retarget_backpressure_and_reset_preserve_effective_gains_without_heap() {
    use crate::realtime_tests::callback_counts;
    use std::sync::Arc;
    let (mut p, _file) = make(48000, true, 0.0);
    p.retirement.tx.take();
    p.retirement.thread.take().unwrap().join().unwrap();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    p.retirement.tx = Some(tx);
    let initial = p
        .crossfade
        .current_state_snapshot
        .reflection_gains
        .clone()
        .unwrap();
    let b = prepared_pose(&p, 90.0);
    let b_gain = b.reflection_gains.clone().unwrap();
    p.state.store(b);
    let input = vec![0.0; p.config.hop_size];
    let mut output = vec![0.0; p.config.hop_size * 2];
    let context = ProcessContext::new(48000, p.config.hop_size);
    assert_eq!(
        callback_counts(|| {
            p.process(&input, &mut output, &context).unwrap();
        }),
        (0, 0)
    );
    let c = prepared_pose(&p, -90.0);
    let c_gain = c.reflection_gains.clone().unwrap();
    p.state.store(c);
    assert_eq!(
        callback_counts(|| {
            p.process(&input, &mut output, &context).unwrap();
        }),
        (0, 0)
    );
    let total = p.crossfade.crossfade_total as f32;
    let step = p.config.hop_size as f32 / total;
    let effective: Vec<[f32; 2]> = initial
        .iter()
        .zip(&b_gain)
        .map(|(a, b)| std::array::from_fn(|ear| a[ear] * (1.0 - step) + b[ear] * step))
        .collect();
    for (actual, expected) in p.room.reflection_effective.iter().zip(&effective) {
        for ear in 0..2 {
            assert!((actual[ear] - expected[ear]).abs() < 2.0e-6);
        }
    }
    let current = Arc::clone(&p.crossfade.current_state_snapshot);
    let d = prepared_pose(&p, 45.0);
    let d_gain = d.reflection_gains.clone().unwrap();
    p.state.store(d);
    assert_eq!(
        callback_counts(|| {
            p.process(&input, &mut output, &context).unwrap();
        }),
        (0, 0)
    );
    assert!(Arc::ptr_eq(&current, &p.crossfade.current_state_snapshot));
    assert_eq!(p.room.reflection_effective, effective);
    drop(rx.try_recv().unwrap());
    assert_eq!(
        callback_counts(|| {
            p.process(&input, &mut output, &context).unwrap();
        }),
        (0, 0)
    );
    for (i, actual) in p.room.reflection_effective.iter().enumerate() {
        for ear in 0..2 {
            let expected = effective[i][ear] * (1.0 - 2.0 * step) + c_gain[i][ear] * (2.0 * step);
            assert!((actual[ear] - expected).abs() < 2.0e-6);
        }
    }
    assert_eq!(callback_counts(|| p.reset()), (0, 0));
    assert_eq!(
        (
            p.room.reflection_gain_write,
            p.room.reflection_gain_read,
            p.room.reflection_gain_phase
        ),
        (0, 0, 0)
    );
    assert!(p.room.reflection_gain_frames.iter().all(|g| g == &d_gain));
}

#[test]
fn real_worker_publishes_reflection_gains_for_native_and_fallback_scenes() {
    use std::time::{Duration, Instant};
    for sofa in [false, true] {
        let (mut p, _file) = make(48000, sofa, 0.0);
        p.spawn_hrtf_update_thread();
        p.smoothing.head_yaw_deg.reset(90.0);
        let (delay, gain) = expected(&p, 90.0);
        let input = vec![0.0; p.config.hop_size];
        let mut output = vec![0.0; p.config.hop_size * 2];
        let context = ProcessContext::new(48000, p.config.hop_size);
        let start = Instant::now();
        p.process(&input, &mut output, &context).unwrap();
        let deadline = start + Duration::from_secs(2);
        loop {
            if let Ok(guard) = p.state.try_lock() {
                let (_, generation) = p.state.head_publication();
                drop(guard);
                if generation > 0 {
                    break;
                }
            }
            assert!(
                Instant::now() < deadline,
                "real reflection worker did not publish"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let publication_ms = start.elapsed().as_secs_f64() * 1000.0;
        p.process(&input, &mut output, &context).unwrap();
        assert!(
            p.crossfade
                .current_state_snapshot
                .reflection_gains
                .is_some()
        );
        p.shutdown_hrtf_update_thread();
        let warm = 2048;
        let audio = pulse(&mut p, 48000, warm);
        let index = (warm + p.latency_samples() + delay) * 2;
        // pulse() starts after two already emitted source hops; latency stays N.
        assert!((audio[index] - 0.25 * gain[0]).abs() < 2.0e-5);
        assert!((audio[index + 1] - 0.25 * gain[1]).abs() < 2.0e-5);
        println!(
            "REFLECTION_REAL_WORKER sofa={sofa} observed_publication_ms={}",
            publication_ms
        );
    }
}

#[test]
fn retired_reflection_epoch_and_wrong_topology_cannot_replace_the_audio_owner() {
    use std::sync::Arc;
    let (mut p, _file) = make(48000, true, 0.0);
    let current = Arc::clone(&p.crossfade.current_state_snapshot);
    p.head_requests.start();
    p.head_requests.publish((90.0, 0.0, 0.0));
    let pose = p.head_requests.snapshot().unwrap();
    let stale = prepared_pose(&p, 90.0);
    p.reset();
    assert!(!p.state.store_head(stale, &p.head_requests, pose));
    let mut wrong = prepared_pose(&p, -90.0);
    Arc::get_mut(&mut wrong)
        .unwrap()
        .reflection_gains
        .as_mut()
        .unwrap()
        .pop();
    p.state.store(wrong);
    let input = vec![0.0; p.config.hop_size];
    let mut output = vec![0.0; p.config.hop_size * 2];
    p.process(
        &input,
        &mut output,
        &ProcessContext::new(48000, p.config.hop_size),
    )
    .unwrap();
    assert!(Arc::ptr_eq(&current, &p.crossfade.current_state_snapshot));
}

#[test]
fn merged_reflection_paths_follow_independent_three_axis_head_basis() {
    use std::collections::BTreeMap;
    let (mut p, _file) = make(48000, true, 0.0);
    p.config.room_model = RoomModel {
        dimensions: [6.0, 6.0, 3.0],
        listener_position: [3.0, 3.0, 1.5],
        absorption: [0.2; 6],
        max_order: 2,
        speed_of_sound: 343.0,
    };
    p.initialize(48000.0).unwrap();
    p.shutdown_hrtf_update_thread();
    assert!(
        p.config.reflection_scene.tap_count < p.config.reflection_scene.rays.len(),
        "fixture must merge distinct directions at equal delay"
    );
    let base = p.state.load_full();
    let sofa = base._hrtf_data.as_ref().unwrap();
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(256);
    let prepared = crate::hrtf::prepare_hrtf_spectra(sofa, 256, 48000.0, &fft);
    for angles in [
        (90.0, 0.0, 0.0),
        (0.0, 45.0, 0.0),
        (0.0, 0.0, 60.0),
        (45.0, 30.0, -45.0),
    ] {
        let (yaw, pitch, roll) = (
            f64::from(angles.0).to_radians(),
            f64::from(angles.1).to_radians(),
            f64::from(angles.2).to_radians(),
        );
        let forward = [
            pitch.cos() * yaw.cos(),
            pitch.cos() * yaw.sin(),
            pitch.sin(),
        ];
        let horizontal_left = [-yaw.sin(), yaw.cos(), 0.0];
        let upright = [
            -pitch.sin() * yaw.cos(),
            -pitch.sin() * yaw.sin(),
            pitch.cos(),
        ];
        let left = std::array::from_fn::<_, 3, _>(|i| {
            roll.cos() * horizontal_left[i] + roll.sin() * upright[i]
        });
        let up = std::array::from_fn::<_, 3, _>(|i| {
            -roll.sin() * horizontal_left[i] + roll.cos() * upright[i]
        });
        let mut expected = BTreeMap::<(usize, usize), [f64; 2]>::new();
        for (channel, rays) in p.room.cached_reflections.iter().enumerate() {
            for ray in rays {
                let a = f64::from(ray.azimuth_deg).to_radians();
                let e = f64::from(ray.elevation_deg).to_radians();
                let world = [e.cos() * a.cos(), e.cos() * a.sin(), e.sin()];
                let dot = |axis: [f64; 3]| axis.iter().zip(world).map(|(a, b)| a * b).sum::<f64>();
                let (x, y, z) = (dot(forward), dot(left), dot(up));
                let target = SourcePosition::new(
                    y.atan2(x).to_degrees() as f32,
                    z.atan2(x.hypot(y)).to_degrees() as f32,
                    1.0,
                );
                let (near, weights) = crate::hrtf::find_hrtf_neighbors(sofa, &target);
                let (l, r) = crate::hrtf::interpolate_hrtf_frequency_domain(
                    &near,
                    &weights,
                    sofa,
                    256,
                    48000.0,
                    &fft,
                    0.0,
                    target.azimuth,
                    target.elevation,
                );
                let h = ReflectionHrtf::from_freq_domain(l, r);
                let gain = expected
                    .entry((ray.delay_samples, channel))
                    .or_insert([0.0; 2]);
                gain[0] += f64::from(ray.gain) * f64::from(h.left_gain_broadband);
                gain[1] += f64::from(ray.gain) * f64::from(h.right_gain_broadband);
            }
        }
        let result = BinauralDecoderPlugin::compute_head_rotated_hrtf_state(
            &base,
            &p.config,
            Some(&prepared),
            &p.fft,
            &[],
            angles,
        )
        .unwrap();
        let actual = result.reflection_gains.as_ref().unwrap();
        assert_eq!(actual.len(), expected.len());
        for (gain, reference) in actual.iter().zip(expected.values()) {
            for ear in 0..2 {
                assert!(
                    (f64::from(gain[ear]) - reference[ear]).abs() < 2.0e-5,
                    "three-axis reflection {angles:?}: {gain:?} vs {reference:?}"
                );
            }
        }
    }
}

#[test]
#[ignore = "explicit release preparation-cost measurement for a second-order eight-channel scene"]
fn audit_reflection_pose_preparation_cost() {
    use crate::realtime_tests::preparation_heap;
    use std::time::Instant;
    let file = crate::head_cue_tests::sofa(48000, 72, 0.35);
    let mut p = BinauralDecoderPlugin::new(
        8,
        4096,
        Some(file.path.clone()),
        1.0,
        0.0,
        false,
        120.0,
        2.0,
        0.0,
        RoomModel {
            max_order: 2,
            ..Default::default()
        },
    );
    p.initialize(48000.0).unwrap();
    p.shutdown_hrtf_update_thread();
    let base = p.state.load_full();
    let sofa = base._hrtf_data.as_ref().unwrap();
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(4096);
    let prepared = crate::hrtf::prepare_hrtf_spectra(sofa, 4096, 48000.0, &fft);
    let mut poses = Vec::new();
    let mut maximum_ns = 0;
    let mut maximum_bytes = 0;
    for angle in [-90.0, -45.0, 0.0, 45.0, 90.0] {
        let start = Instant::now();
        let (result, bytes, _, _) = preparation_heap(|| {
            BinauralDecoderPlugin::compute_head_rotated_hrtf_state(
                &base,
                &p.config,
                Some(&prepared),
                &p.fft,
                &p.coefficients.lfe_channels,
                (angle, 15.0, -10.0),
            )
            .unwrap()
        });
        maximum_ns = maximum_ns.max(start.elapsed().as_nanos());
        maximum_bytes = maximum_bytes.max(bytes);
        assert_eq!(
            result.reflection_gains.as_ref().unwrap().len(),
            p.config.reflection_scene.tap_count
        );
        poses.push(result);
    }
    println!(
        "REFLECTION_POSE_COST design=4096 channels=8 measurements=72 rays={} merged_taps={} maximum_ns={maximum_ns} maximum_requested_bytes={maximum_bytes}",
        p.config.reflection_scene.rays.len(),
        p.config.reflection_scene.tap_count
    );
    let frames = p.config.hop_size;
    let input: Vec<f32> = (0..frames * 8)
        .map(|i| (i as f32 * 0.031).sin() * 0.02)
        .collect();
    let mut output = vec![0.0; frames * 2];
    let context = ProcessContext::new(48000, frames);
    let mut times = Vec::with_capacity(512);
    for index in 0..512 {
        p.state
            .store(std::sync::Arc::clone(&poses[index % poses.len()]));
        let start = Instant::now();
        let counts = crate::realtime_tests::callback_counts(|| {
            p.process(&input, &mut output, &context).unwrap();
        });
        times.push(start.elapsed().as_nanos());
        assert_eq!(counts, (0, 0));
        assert!(output.iter().all(|x| x.is_finite()));
    }
    times.sort_unstable();
    println!(
        "REFLECTION_WET_CALLBACK_COST callbacks=512 frames={frames} channels=8 design=4096 audio_fft=8192 allocations=0 frees=0 p50_ns={} p95_ns={} max_ns={}",
        times[256], times[486], times[511]
    );
}

#[test]
fn horizontal_queries_use_circular_brackets_without_third_neighbor_discontinuities() {
    use sotf_host::sofa::SofaFile;
    let positions: Vec<_> = (0..36)
        .map(|i| SourcePosition::new(-180.0 + i as f32 * 10.0, 0.0, 1.0))
        .collect();
    let impulse_responses: Vec<_> = positions
        .iter()
        .flat_map(|p| {
            let side = f64::from(p.azimuth).to_radians().sin() as f32;
            [1.0 + 0.25 * side, 1.0 - 0.25 * side]
        })
        .collect();
    let sofa = SofaFile {
        sample_rate: 48000.0,
        num_measurements: 36,
        ir_length: 1,
        positions,
        impulse_responses,
        convention: "SimpleFreeFieldHRIR".into(),
        data_sample_rate: Some(48000.0),
    };
    for az in [
        -179.999, -175.0, -45.0001, -45.0, -44.9999, 0.0, 44.9999, 45.0, 45.0001, 179.999, 180.0,
        360.0, 405.0,
    ] {
        for el in [-90.0, -60.0, 0.0, 60.0, 90.0] {
            let target = SourcePosition::new(az, el, 1.0);
            let (near, weights) = crate::hrtf::find_hrtf_neighbors(&sofa, &target);
            assert!((weights.iter().sum::<f32>() - 1.0).abs() < 1.0e-6);
            assert_eq!(weights[2], 0.0);
            let mut center = 0.0_f64;
            for i in 0..3 {
                let offset = (f64::from(sofa.positions[near[i].0].azimuth) - f64::from(az) + 180.0)
                    .rem_euclid(360.0)
                    - 180.0;
                if weights[i] > 0.0 {
                    assert!(offset.abs() <= 10.0001);
                }
                center += f64::from(weights[i]) * offset;
            }
            assert!(
                center.abs() < 5.0e-5,
                "azimuth bracket at {az}/{el}: {center}"
            );
        }
    }
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(256);
    let prepared = crate::hrtf::prepare_hrtf_spectra(&sofa, 256, 48000.0, &fft);
    let mut previous = None::<[f32; 2]>;
    for az in [44.9999, 45.0, 45.0001] {
        let target = SourcePosition::new(az, 60.0, 1.0);
        let (near, weights) = crate::hrtf::find_hrtf_neighbors(&sofa, &target);
        let actual = crate::hrtf::interpolate_hrtf_broadband_prepared(&near, &weights, &prepared);
        let t = (f64::from(az) - 40.0) / 10.0;
        let expected: [f64; 2] = std::array::from_fn(|ear| {
            let sign = if ear == 0 { 1.0 } else { -1.0 };
            (1.0 + sign * 0.25 * 40.0_f64.to_radians().sin()).powf(1.0 - t)
                * (1.0 + sign * 0.25 * 50.0_f64.to_radians().sin()).powf(t)
        });
        let maximum = expected[0].max(expected[1]);
        for ear in 0..2 {
            assert!((f64::from(actual[ear]) - expected[ear] / maximum).abs() < 2.0e-6);
        }
        if let Some(old) = previous {
            assert!(
                (actual[1] - old[1]).abs() < 1.0e-5,
                "third-neighbor gain jump"
            );
        }
        previous = Some(actual);
    }
}

#[test]
fn source_hop_grid_is_nonzero_and_padded_size_is_representable() {
    for n in [0, 1, 2, 3, 6, 1_usize << (usize::BITS - 1)] {
        let params =
            serde_json::from_value(serde_json::json!({"input_channels":1,"fft_size":n})).unwrap();
        assert!(
            BinauralDecoderPlugin::try_from_params(params).is_err(),
            "invalid source-hop grid {n}"
        );
    }
    let params = serde_json::from_value(
        serde_json::json!({"input_channels":1,"fft_size":4,"room_model":{"max_order":0}}),
    )
    .unwrap();
    let mut p = BinauralDecoderPlugin::try_from_params(params).unwrap();
    p.initialize(48000.0).unwrap();
    let audio = pulse(&mut p, 48000, 0);
    assert!((audio[8] - 0.25 * 0.95).abs() < 2.0e-6);
}
