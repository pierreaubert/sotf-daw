//! Continuous-time array fixtures independent of production steering and adaptation.

use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_plugin_beamformer::{BeamformerPlugin, BeamformerPluginParams};

#[derive(Clone, Copy, Debug)]
enum Scene {
    Stationary,
    Motion,
    Mismatch,
    Diffuse,
    Mixed,
    NoiseThenTarget,
}

fn source(time: f64) -> f64 {
    [211.0, 397.0, 733.0, 1201.0, 1853.0]
        .iter()
        .enumerate()
        .map(|(i, f)| (std::f64::consts::TAU * f * time + i as f64 * 0.73).sin() * 0.055)
        .sum()
}

fn diffuse(time: f64, position: f64) -> f64 {
    // Equal-weight direction cosines quadrature of a 3D isotropic field for
    // a linear array. Each plane wave has distinct deterministic excitation.
    (0..7)
        .map(|ray| {
            let cosine = (2.0 * ray as f64 + 1.0) / 7.0 - 1.0;
            let t = time - position * cosine / 343.0;
            let f = 311.0 + ray as f64 * 271.0;
            ((std::f64::consts::TAU * f * t + ray as f64 * 0.39).sin()
                + (std::f64::consts::TAU * (f + 113.0) * t + ray as f64 * 0.93).cos())
                * 0.025
        })
        .sum()
}

fn fixture(rate: u32, mics: usize, scene: Scene) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let frames = rate as usize * 2;
    let mut input = Vec::with_capacity(frames * mics);
    let mut target = vec![0.0; frames];
    let mut uniform = vec![0.0; frames];
    for n in 0..frames {
        let t = n as f64 / rate as f64;
        let angle = if matches!(scene, Scene::Motion | Scene::Mixed | Scene::NoiseThenTarget) {
            3.0 * (std::f64::consts::TAU * 0.5 * t).sin()
        } else {
            0.0
        };
        for mic in 0..mics {
            let x = mic as f64 * 0.05;
            let sign = if mic % 2 == 0 { -1.0 } else { 1.0 };
            let faulty = matches!(
                scene,
                Scene::Mismatch | Scene::Mixed | Scene::NoiseThenTarget
            );
            let position = x + if faulty { sign * 0.001 } else { 0.0 };
            let time = t - if faulty {
                sign * 0.05 / rate as f64
            } else {
                0.0
            };
            let gain = if faulty {
                10.0_f64.powf(sign * 0.5 / 20.0)
            } else {
                1.0
            };
            let target_on = !matches!(scene, Scene::Diffuse)
                && (!matches!(scene, Scene::NoiseThenTarget) || t >= 1.0);
            let noise_on = matches!(scene, Scene::Diffuse | Scene::Mixed)
                || (matches!(scene, Scene::NoiseThenTarget) && t < 1.0);
            let nominal = if target_on {
                source(t - x * angle.to_radians().sin() / 343.0)
            } else {
                0.0
            };
            let value = (if target_on {
                source(time - position * angle.to_radians().sin() / 343.0)
            } else {
                0.0
            } + if noise_on {
                diffuse(time, position)
            } else {
                0.0
            }) * gain;
            input.push(value as f32);
            target[n] += (nominal / mics as f64) as f32;
            uniform[n] += (value / mics as f64) as f32;
        }
    }
    (input, target, uniform)
}

fn render(rate: u32, mics: usize, kind: usize, input: &[f32], chunks: &[usize]) -> Vec<f32> {
    let mut plugin = BeamformerPlugin::from_params(
        rate,
        BeamformerPluginParams {
            num_mics: mics,
            mic_spacing_cm: 5.0,
            steer_angle_deg: 0.0,
            beamformer_type: kind,
        },
    )
    .unwrap();
    let latency = if kind == 2 { 0 } else { 512 };
    assert_eq!(plugin.latency_samples(), latency);
    let frames = input.len() / mics;
    let mut output = vec![0.0; frames];
    let mut offset = 0;
    let mut callback = 0;
    while offset < frames {
        let count = chunks[callback % chunks.len()].min(frames - offset);
        plugin
            .process(
                &input[offset * mics..(offset + count) * mics],
                &mut output[offset..offset + count],
                &ProcessContext::new(rate, count),
            )
            .unwrap();
        offset += count;
        callback += 1;
    }
    let mut tail = [0.0; 257];
    loop {
        let result = plugin
            .drain(&mut tail, &ProcessContext::new(rate, 0))
            .unwrap();
        output.extend_from_slice(&tail[..result.frames]);
        if result.complete {
            break;
        }
    }
    assert!(output.len() >= latency + frames);
    plugin.reset();
    let mut replay = vec![0.0; 1025];
    plugin
        .process(
            &input[..1025 * mics],
            &mut replay,
            &ProcessContext::new(rate, 1025),
        )
        .unwrap();
    assert_eq!(
        &replay[latency..latency + 512],
        &output[latency..latency + 512],
        "reset replay"
    );
    output[latency..latency + frames].to_vec()
}

fn energy(values: impl Iterator<Item = f64>) -> f64 {
    values.map(|x| x * x).sum::<f64>()
}

#[test]
fn public_moving_and_mismatched_arrays_preserve_targets_and_control_diffuse_noise() {
    let mut failures = Vec::new();
    for rate in [16_000, 48_000, 96_000] {
        for mics in [2, 4, 8] {
            for scene in [
                Scene::Stationary,
                Scene::Motion,
                Scene::Mismatch,
                Scene::Diffuse,
                Scene::Mixed,
                Scene::NoiseThenTarget,
            ] {
                let (input, target, uniform) = fixture(rate, mics, scene);
                let start = if matches!(scene, Scene::NoiseThenTarget) {
                    rate as usize * 5 / 4
                } else {
                    rate as usize
                };
                let target_power = energy(target[start..].iter().map(|&x| f64::from(x)));
                let uniform_error = energy(
                    uniform[start..]
                        .iter()
                        .zip(&target[start..])
                        .map(|(&x, &s)| f64::from(x - s)),
                );
                for kind in 0..3 {
                    let output = render(rate, mics, kind, &input, &[193, 1, 511, 67]);
                    let regular = render(rate, mics, kind, &input, &[256]);
                    assert_eq!(output, regular, "partition {rate}/{mics}/{scene:?}/{kind}");
                    assert!(output.iter().all(|x| x.is_finite()));
                    let error = energy(
                        output[start..]
                            .iter()
                            .zip(&target[start..])
                            .map(|(&x, &s)| f64::from(x - s)),
                    );
                    let snr = 10.0 * (target_power / error.max(1e-30)).log10();
                    let improvement = 10.0 * (uniform_error / error.max(1e-30)).log10();
                    eprintln!(
                        "MOVING_ARRAY {}",
                        serde_json::json!({"rate":rate,"mics":mics,"scene":format!("{scene:?}"),"kind":kind,"target_snr_db":if target_power>0.0 {Some(snr)} else {None},"improvement_vs_uniform_db":if uniform_error>0.0 {Some(improvement)} else {None}})
                    );
                    let good = match scene {
                        Scene::Stationary
                        | Scene::Motion
                        | Scene::Mismatch
                        | Scene::NoiseThenTarget => snr >= 12.0,
                        Scene::Diffuse | Scene::Mixed => improvement >= -1.0,
                    };
                    if !good {
                        failures.push(format!("{rate}/{mics}/{scene:?}/{kind}: SNR {snr:.3}, improvement {improvement:.3}"));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
