//! Safety automation and linked output limiting follow source/output time.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterValue, Plugin, ProcessContext};
use sotf_plugin_upmixer::{UpmixerPlugin, UpmixerPluginParams};

#[derive(Clone, Copy)]
struct Case {
    rate: u32,
    fft: usize,
    hr: bool,
    auto_gain: bool,
    binaural: bool,
}

fn render(
    case: Case,
    pattern: &[usize],
    automate: bool,
    snapshots: bool,
    tail_pattern: &[usize],
) -> Vec<f32> {
    let params: UpmixerPluginParams = serde_json::from_value(serde_json::json!({
        "fft_size":case.fft,"speaker_config":"9.1.6","enable_hr_direct":case.hr,
        "auto_gain_enabled":case.auto_gain,"binaural_preview":case.binaural,
        "bypass_decorrelation":true,"safety_cap_db":3.0,
        "gain_front_direct":1.6,"gain_front_ambient":1.2,"height_gain":1.2
    }))
    .unwrap();
    let mut p = UpmixerPlugin::from_params(params);
    p.initialize(f64::from(case.rate)).unwrap();
    let (warm, frames) = (32768, 8197);
    let input: Vec<_> = (0..warm + frames)
        .flat_map(|i| {
            let t = i as f64 / f64::from(case.rate);
            let amplitude = if i % 4096 < 2048 { 3.0 } else { 0.02 };
            let low = (std::f64::consts::TAU * 55.0 * t).sin();
            let left = (low
                + (std::f64::consts::TAU * 997.0 * t).sin()
                + 0.3 * (std::f64::consts::TAU * 3200.0 * t).cos())
                * amplitude;
            let right = (low + (std::f64::consts::TAU * 601.0 * t).cos()) * amplitude;
            [left as f32, right as f32]
        })
        .collect();
    let ch = p.output_channels();
    assert_eq!(ch, if case.binaural { 2 } else { 16 });
    let mut warm_output = vec![0.0; 256 * ch];
    for chunk in input[..warm * 2].as_chunks::<512>().0 {
        p.process(
            chunk,
            &mut warm_output,
            &ProcessContext::new(case.rate, 256),
        )
        .unwrap();
    }
    let events = if automate {
        [(0, 0.0), (2051, 3.0), (4133, 0.0)]
    } else {
        [(0, 3.0), (2051, 3.0), (4133, 3.0)]
    };
    let mut output = vec![0.0; frames * ch];
    let (mut offset, mut event, mut call) = (0, 0, 0);
    while offset < frames {
        if event < events.len() && offset == events[event].0 {
            p.set_parameter(
                "safety_cap_db".into(),
                ParameterValue::Float(events[event].1),
            )
            .unwrap();
            event += 1;
        }
        if snapshots {
            let id = "safety_cap_db".into();
            p.set_parameter(id, p.get_parameter(&"safety_cap_db".into()).unwrap())
                .unwrap();
            p.process(&[], &mut [], &ProcessContext::new(case.rate, 0))
                .unwrap();
        }
        let boundary = events.get(event).map_or(frames, |x| x.0);
        let n = pattern[call % pattern.len()].min(boundary - offset);
        assert_eq!(
            p.process(
                &input[(warm + offset) * 2..(warm + offset + n) * 2],
                &mut output[offset * ch..(offset + n) * ch],
                &ProcessContext::new(case.rate, n)
            )
            .unwrap(),
            n
        );
        offset += n;
        call += 1;
    }
    let max_hop = p.drain_output_frames_max();
    let mut tail = vec![0.0; max_hop * ch];
    let limit = p.drain_call_bound().unwrap().get() as usize * max_hop + 1;
    for call in 0..limit {
        let n = tail_pattern[call % tail_pattern.len()].min(max_hop);
        let result = p
            .drain(&mut tail[..n * ch], &ProcessContext::new(case.rate, 0))
            .unwrap();
        output.extend_from_slice(&tail[..result.frames * ch]);
        if result.complete {
            assert!(output.iter().all(|v| v.is_finite() && v.abs() <= 1.413));
            assert!(output.iter().any(|v| v.abs() > 0.5));
            return output;
        }
    }
    panic!("drain did not complete")
}

fn difference(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max)
}

#[test]
fn safety_cap_clock_matches_callback_partitions() {
    let case = Case {
        rate: 48000,
        fft: 2048,
        hr: true,
        auto_gain: false,
        binaural: false,
    };
    let mut errors = Vec::new();
    for automate in [false, true] {
        let reference = render(case, &[1], automate, false, &[1024]);
        for pattern in [&[8192][..], &[7, 31, 257, 1024][..]] {
            let error = difference(&reference, &render(case, pattern, automate, false, &[1024]));
            println!("safety_clock automate={automate} pattern={pattern:?} error={error:e}");
            errors.push(error);
        }
    }
    assert!(errors.iter().all(|e| *e < 2e-6), "{errors:?}");
}

#[test]
fn safety_clocks_match_rates_fft_hr_autogain_and_binaural_routes() {
    for rate in [44100, 48000, 192000] {
        for fft in [512, 4096] {
            for hr in [false, true] {
                for (auto_gain, binaural) in
                    [(false, false), (false, true), (true, false), (true, true)]
                {
                    let case = Case {
                        rate,
                        fft,
                        hr,
                        auto_gain,
                        binaural,
                    };
                    let reference = render(case, &[1], true, false, &[1024]);
                    let error =
                        difference(&reference, &render(case, &[8192], true, false, &[1024]));
                    println!(
                        "safety_route rate={rate} fft={fft} hr={hr} auto_gain={auto_gain} binaural={binaural} error={error:e}"
                    );
                    assert!(error < 2e-6);
                }
            }
        }
    }
}

#[test]
fn safety_snapshots_empty_calls_and_copied_drain_cache_do_not_move_the_clocks() {
    let case = Case {
        rate: 48000,
        fft: 2048,
        hr: true,
        auto_gain: true,
        binaural: false,
    };
    let reference = render(case, &[7, 31, 257], true, false, &[1024]);
    assert_eq!(reference, render(case, &[7, 31, 257], true, true, &[1]));
    assert_eq!(reference, render(case, &[8192], true, false, &[7, 31, 257]));
}

#[test]
fn configured_zero_db_cap_protects_the_first_hot_output_and_tail() {
    for rate in [44_100, 192_000] {
        for fft in [2, 512, 4096] {
            for binaural in [false, true] {
                let params = serde_json::from_value(serde_json::json!({
                    "fft_size":fft,"speaker_config":"9.1.6","enable_hr_direct":true,
                    "auto_gain_enabled":true,"binaural_preview":binaural,"safety_cap_db":0.0
                }))
                .unwrap();
                let mut p = UpmixerPlugin::from_params(params);
                p.initialize(f64::from(rate)).unwrap();
                assert_eq!(
                    p.get_parameter(&"safety_cap_db".into()),
                    Some(ParameterValue::Float(0.0))
                );
                let input = vec![4.0; 12289 * 2];
                let ch = p.output_channels();
                let mut output = vec![0.0; 12289 * ch];
                assert_eq!(
                    p.process(&input, &mut output, &ProcessContext::new(rate, 12289))
                        .unwrap(),
                    12289
                );
                assert!(output.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
                assert!(output.iter().any(|s| s.abs() > 0.05));
                let mut tail = vec![0.0; p.drain_output_frames_max() * ch];
                for _ in 0..p.drain_call_bound().unwrap().get() {
                    let result = p.drain(&mut tail, &ProcessContext::new(rate, 0)).unwrap();
                    assert!(
                        tail[..result.frames * ch]
                            .iter()
                            .all(|s| s.is_finite() && s.abs() <= 1.0)
                    );
                    if result.complete {
                        break;
                    }
                }
            }
        }
    }
}
