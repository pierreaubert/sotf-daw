//! Offline rendering keeps recursive delay continuation under an explicit policy.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterValue, TailLength, host::DawHost};
use sotf_plugins::create_plugin;

#[derive(Debug, PartialEq)]
enum Stop {
    QuietWindow,
    DurationLimit,
}

fn host(rate: u32, delay_ms: f32, feedback: f32, clean: bool) -> DawHost {
    let mut host = DawHost::new(1, rate);
    host.add_plugin(
        create_plugin(
            "delay",
            &serde_json::json!({
                "delay_ms":delay_ms,"feedback":feedback,"mix":1.0,
                "lfo_rate_hz":0.0,"lfo_depth_ms":0.0,"pitch_preserving":clean
            }),
            1,
            rate,
        )
        .unwrap(),
    )
    .unwrap();
    host.build().unwrap();
    host
}

// The offline caller feeds silence BEFORE native drain. Energy is checked on
// fixed windows at least as long as the largest delay, never on callback gaps.
// This policy is for these frozen, bounded test configurations, not a generic
// proof that arbitrary recursive/modulated chains have exhausted their state.
fn render_tail(
    host: &mut DawHost,
    source: &[f32],
    parts: &[usize],
    max_tail: usize,
    window: usize,
    quiet_energy: f64,
) -> (Vec<f32>, Stop) {
    let mut output = Vec::new();
    let mut part = 0;
    while output.len() < source.len() {
        let start = output.len();
        let frames = parts[part % parts.len()].min(source.len() - start);
        let mut block = vec![0.0; frames];
        assert_eq!(
            host.process(&source[start..start + frames], &mut block)
                .unwrap(),
            frames
        );
        output.extend(block);
        part += 1;
    }
    let mut tail_frames = 0;
    let mut energy = 0.0;
    while tail_frames < max_tail {
        let frames = parts[part % parts.len()]
            .min(window - tail_frames % window)
            .min(max_tail - tail_frames);
        let mut block = vec![0.0; frames];
        assert_eq!(
            host.process(&vec![0.0; frames], &mut block).unwrap(),
            frames
        );
        energy += block.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>();
        output.extend(block);
        tail_frames += frames;
        part += 1;
        if tail_frames.is_multiple_of(window) {
            if tail_frames >= 2 * window && energy <= quiet_energy {
                return (output, Stop::QuietWindow);
            }
            energy = 0.0;
        }
    }
    (output, Stop::DurationLimit)
}

#[test]
fn near_limit_feedback_offline_tail_matches_geometric_energy_and_absolute_clock() {
    for rate in [44_100, 48_000, 96_000] {
        let period = rate as usize / 100;
        let mut source = vec![0.0; period];
        source[period - 1] = 1.0;
        for feedback in [-0.95_f32, 0.95] {
            let render = |parts: &[usize]| {
                render_tail(
                    &mut host(rate, 10.0, feedback, false),
                    &source,
                    parts,
                    rate as usize * 3,
                    period,
                    1e-8,
                )
            };
            let (actual, reason) = render(&[8192]);
            assert_eq!(reason, Stop::QuietWindow);
            assert_eq!(render(&[1, 7, 31, 257]).0, actual);
            let repeats = (actual.len() - source.len()) / period;
            assert!(repeats > 170 && repeats < 200);
            let mut measured_energy = 0.0;
            for (frame, &sample) in actual.iter().enumerate() {
                let echo = (frame + 1) / period;
                let expected = if (frame + 1).is_multiple_of(period) && echo >= 2 {
                    f64::from(feedback).powi(echo as i32 - 2)
                } else {
                    0.0
                };
                assert!(
                    (f64::from(sample) - expected).abs() < 2e-6,
                    "rate={rate}, feedback={feedback}, frame={frame}"
                );
                measured_energy += f64::from(sample).powi(2);
            }
            let f = f64::from(feedback);
            let omitted = f.powi(2 * repeats as i32) / (1.0 - f * f);
            assert!(omitted <= 1e-7, "unrendered energy={omitted}");
            assert!((measured_energy + omitted - 1.0 / (1.0 - f * f)).abs() < 2e-5);
        }
    }
}

#[test]
fn duration_limit_is_exact_and_native_drain_is_not_recursive_tail_rendering() {
    let rate = 48_000;
    let mut source = vec![0.0; 480];
    source[479] = 1.0;
    let mut native = host(rate, 10.0, 0.95, false);
    native
        .process(&source, &mut vec![0.0; source.len()])
        .unwrap();
    assert_eq!(native.tail_length(), TailLength::Infinite);
    let result = native.drain(&mut [0.0; 512]).unwrap();
    assert!(result.complete);
    assert_eq!(result.frames, 0);
    let (output, reason) = render_tail(
        &mut host(rate, 10.0, 0.95, false),
        &source,
        &[7, 31, 257],
        1_681,
        480,
        1e-8,
    );
    assert_eq!(reason, Stop::DurationLimit);
    assert_eq!(output.len(), source.len() + 1_681);
    assert!(output.iter().filter(|&&x| x != 0.0).count() >= 3);
    // A large, intentionally truncated remaining tail must not be reported quiet.
    assert!(0.95_f64.powi(6) / (1.0 - 0.95_f64.powi(2)) > 7.0);
}

fn tempo_render(rate: u32, clean: bool, parts: &[usize]) -> Vec<f32> {
    let mut host = host(rate, 125.0, 0.95, clean);
    let total = rate as usize * 2;
    let source_end = rate as usize / 2;
    // External tempo-to-time conversion for sixteenth notes: 15000 / BPM ms.
    let changes = [(source_end / 3, 240.0), (source_end * 2 / 3, 80.0)];
    let mut output = Vec::new();
    let mut part = 0;
    while output.len() < total {
        let start = output.len();
        for &(at, bpm) in &changes {
            if start == at {
                host.set_plugin_parameter(0, "delay_ms", ParameterValue::Float(15_000.0 / bpm))
                    .unwrap();
            }
        }
        let boundary = changes
            .iter()
            .map(|&(at, _)| at)
            .chain([source_end, total])
            .filter(|&at| at > start)
            .min()
            .unwrap();
        let frames = parts[part % parts.len()].min(boundary - start);
        let input: Vec<f32> = (start..start + frames)
            .map(|i| {
                if i < source_end {
                    0.05 * (std::f32::consts::TAU * 997.0 * i as f32 / rate as f32).sin()
                } else {
                    0.0
                }
            })
            .collect();
        let mut block = vec![0.0; frames];
        host.process(&input, &mut block).unwrap();
        output.extend(block);
        part += 1;
    }
    output
}

#[test]
fn abrupt_tempo_changes_keep_both_modes_finite_and_callback_invariant() {
    for rate in [44_100, 48_000, 96_000] {
        let mut modes = Vec::new();
        for clean in [false, true] {
            let actual = tempo_render(rate, clean, &[8192]);
            assert_eq!(actual, tempo_render(rate, clean, &[1, 31, 257]));
            assert!(actual.iter().all(|x| x.is_finite() && x.abs() < 1.0));
            assert!(actual[rate as usize..].iter().any(|x| x.abs() > 1e-4));
            modes.push(actual);
        }
        assert!(
            modes[0]
                .iter()
                .zip(&modes[1])
                .any(|(a, b)| (a - b).abs() > 0.01),
            "tape glide and stationary-tap transition are distinct audible modes"
        );
    }
}
