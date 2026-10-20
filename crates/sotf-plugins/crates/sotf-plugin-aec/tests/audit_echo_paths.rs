//! Independent, latency-aligned acoustic-path and near-end preservation checks.

use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_plugin_aec::{AecPlugin, AecPluginParams};

#[derive(Clone, Copy, Debug)]
enum Scene {
    Linear,
    DelayJump,
    Drift(i32),
    Nonlinear,
    NearOnly,
    NearAfterEcho,
    DoubleTalk(f32),
}

fn noise(length: usize, mut seed: u32) -> Vec<f32> {
    let mut low = 0.0;
    (0..length)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            low = 0.65 * low + 0.35 * (seed as f64 / u32::MAX as f64 * 2.0 - 1.0) as f32;
            low * 0.6
        })
        .collect()
}

fn delayed(signal: &[f32], position: f64) -> f32 {
    if position < 0.0 {
        return 0.0;
    }
    let index = position.floor() as usize;
    let fraction = (position - index as f64) as f32;
    signal[index] * (1.0 - fraction) + signal[index + 1] * fraction
}

fn fixture(rate: u32, scene: Scene) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let length = rate as usize * 6;
    let mut reference = noise(length + 2, 0x51c9_2e37);
    if matches!(scene, Scene::NearAfterEcho) {
        reference[rate as usize * 3..].fill(0.0);
    }
    let near = noise(length, 0x9d24_7ab1);
    let mut input = Vec::with_capacity(length * 2);
    let mut echo = Vec::with_capacity(length);
    let mut target = Vec::with_capacity(length);
    for n in 0..length {
        let time = n as f64 / rate as f64;
        let delay = if matches!(scene, Scene::DelayJump) && time >= 3.0 {
            rate as f64 * 0.019
        } else {
            rate as f64 * 0.007
        };
        let drift = if let Scene::Drift(ppm) = scene {
            n as f64 * f64::from(ppm) * 1e-6
        } else {
            0.0
        };
        let mut e = 0.55 * delayed(&reference, n as f64 - delay - drift)
            - 0.17 * delayed(&reference, n as f64 - delay - drift - rate as f64 * 0.003);
        if matches!(scene, Scene::Nonlinear) {
            e = (3.0 * e).tanh() / 3.0;
        }
        let near_active = matches!(scene, Scene::NearOnly)
            || (matches!(scene, Scene::NearAfterEcho) && time >= 3.0)
            || (matches!(scene, Scene::DoubleTalk(_)) && (3.0..4.5).contains(&time));
        let scale = if let Scene::DoubleTalk(scale) = scene {
            scale
        } else {
            1.0
        };
        let s = if near_active { near[n] * scale } else { 0.0 };
        if matches!(scene, Scene::NearOnly) {
            e = 0.0;
        }
        input.extend_from_slice(&[
            e + s,
            if matches!(scene, Scene::NearOnly) {
                0.0
            } else {
                reference[n]
            },
        ]);
        echo.push(e);
        target.push(s);
    }
    (input, echo, target)
}

fn render(rate: u32, input: &[f32], post_filter: bool, chunks: &[usize]) -> Vec<f32> {
    let mut plugin = AecPlugin::from_params(
        rate,
        AecPluginParams {
            echo_tail_ms: 50.0,
            step_size: 0.5,
            post_filter_enabled: post_filter,
        },
    )
    .unwrap();
    assert_eq!(plugin.latency_samples(), 256);
    let frames = input.len() / 2;
    let mut output = vec![0.0; frames + plugin.latency_samples()];
    let mut offset = 0;
    let mut callback = 0;
    while offset < output.len() {
        let count = chunks[callback % chunks.len()].min(output.len() - offset);
        let block: Vec<f32> = (offset..offset + count)
            .flat_map(|n| {
                if n < frames {
                    [input[2 * n], input[2 * n + 1]]
                } else {
                    [0.0, 0.0]
                }
            })
            .collect();
        plugin
            .process(
                &block,
                &mut output[offset..offset + count],
                &ProcessContext::new(rate, count),
            )
            .unwrap();
        offset += count;
        callback += 1;
    }
    output[plugin.latency_samples()..].to_vec()
}

fn ratio_db(signal: &[f32], error: &[f32]) -> f64 {
    let power = |samples: &[f32]| samples.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>();
    10.0 * (power(signal) / power(error).max(1e-30)).log10()
}

#[test]
fn independent_echo_paths_preserve_near_end_and_track_changes() {
    let mut failures = Vec::new();
    for rate in [16_000, 44_100, 48_000] {
        for scene in [
            Scene::Linear,
            Scene::DelayJump,
            Scene::Drift(-100),
            Scene::Drift(100),
            Scene::Nonlinear,
            Scene::NearOnly,
            Scene::NearAfterEcho,
            Scene::DoubleTalk(0.25),
            Scene::DoubleTalk(1.0),
            Scene::DoubleTalk(2.0),
        ] {
            let (input, echo, near) = fixture(rate, scene);
            for post in [false, true] {
                let output = render(rate, &input, post, &[193, 1, 511, 67]);
                let regular = render(rate, &input, post, &[256]);
                assert_eq!(
                    output, regular,
                    "callback partition changed {rate}/{scene:?}/{post}"
                );
                assert!(output.iter().all(|sample| sample.is_finite()));
                // An untrained filter has no estimate. Check the first actual
                // output block against the known microphone, after latency.
                for (n, sample) in output[..256].iter().enumerate() {
                    assert!((sample - input[2 * n]).abs() < 2e-7);
                }
                let (start, end, minimum) = match scene {
                    Scene::NearOnly => (0, near.len(), 100.0),
                    Scene::NearAfterEcho => (rate as usize * 13 / 4, near.len(), 100.0),
                    Scene::DoubleTalk(_) => (rate as usize * 13 / 4, rate as usize * 17 / 4, 10.0),
                    // Require at least half the echo power removed under continuous drift.
                    // This is bounded tracking, not clock synchronization.
                    Scene::Drift(_) => (rate as usize * 5, echo.len(), 3.0),
                    Scene::Nonlinear => (rate as usize * 5, echo.len(), 6.0),
                    _ => (rate as usize * 5, echo.len(), 12.0),
                };
                let metric = if matches!(
                    scene,
                    Scene::NearOnly | Scene::NearAfterEcho | Scene::DoubleTalk(_)
                ) {
                    let error: Vec<f32> = output[start..end]
                        .iter()
                        .zip(&near[start..end])
                        .map(|(y, s)| y - s)
                        .collect();
                    ratio_db(&near[start..end], &error)
                } else {
                    ratio_db(&echo[start..end], &output[start..end])
                };
                eprintln!("AEC {rate} {scene:?} post={post} metric_db={metric:.6}");
                if metric < minimum {
                    failures.push(format!(
                        "{rate}/{scene:?}/post={post}: {metric:.3} < {minimum}"
                    ));
                }
                if matches!(scene, Scene::Linear) {
                    // Report the first adaptation interval in 250 ms windows,
                    // independently of the steady-state score at 5–6 seconds.
                    let width = rate as usize / 4;
                    let curve: Vec<f64> = (0..8)
                        .map(|window| {
                            let range = window * width..(window + 1) * width;
                            ratio_db(&echo[range.clone()], &output[range])
                        })
                        .collect();
                    assert!(curve[7] >= 12.0, "initial convergence: {curve:?}");
                    eprintln!("AEC_CONVERGENCE {rate} post={post} erle_250ms_windows={curve:?}");
                }
                if matches!(scene, Scene::DoubleTalk(_)) {
                    let start = rate as usize * 3;
                    let end = start + rate as usize / 8;
                    let error: Vec<f32> = output[start..end]
                        .iter()
                        .zip(&near[start..end])
                        .map(|(y, s)| y - s)
                        .collect();
                    let onset_snr = ratio_db(&near[start..end], &error);
                    let start = rate as usize * 5;
                    let recovery_erle = ratio_db(&echo[start..], &output[start..]);
                    eprintln!(
                        "AEC_PHASE {rate} {scene:?} post={post} onset_snr_db={onset_snr:.6} recovery_erle_db={recovery_erle:.6}"
                    );
                    if onset_snr < 10.0 || recovery_erle < 12.0 {
                        failures.push(format!("{rate}/{scene:?}/post={post}: onset={onset_snr:.3}, recovery={recovery_erle:.3}"));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
