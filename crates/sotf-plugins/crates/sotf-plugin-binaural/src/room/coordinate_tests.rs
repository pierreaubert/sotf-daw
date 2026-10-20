//! Independent physical frames for shadowing, image paths and native ear cues.

use super::{RoomModel, calculate_reflections};
use crate::BinauralDecoderPlugin;
use rustfft::num_complex::Complex;
use sotf_host::speaker_config::get_speaker_config_by_channels;
use sotf_host::{Plugin, ProcessContext};

fn shape(rate: f64, n: usize, az: f64, el: f64, strength: f64) -> [Vec<Complex<f32>>; 2] {
    let mut left = vec![Complex::new(0.6, -0.2); n / 2 + 1];
    let mut right = left.clone();
    crate::hrtf::apply_near_field_shadowing(
        &mut left,
        &mut right,
        az as f32,
        el as f32,
        n,
        rate,
        strength as f32,
    );
    [left, right]
}

#[test]
fn shadowing_matches_normalized_lateral_geometry_without_ipsilateral_changes() {
    let mut cases = 0;
    let mut maximum = 0.0_f64;
    for rate in [44100.0, 48000.0, 96000.0] {
        for n in [64, 256, 1024] {
            for az in [
                90.0, -90.0, 0.0, 15.0, -15.0, 30.0, -30.0, 150.0, -150.0, 179.0, -179.0, 180.0,
                -180.0,
            ] {
                for el in [0.0, -60.0, 60.0, -90.0, 90.0] {
                    for strength in [0.0, 0.25, 1.0] {
                        let output = shape(rate, n, az, el, strength);
                        // Source unit vector in forward/left/up coordinates. Dotting with the
                        // physical left-ear axis determines ipsilateral and contralateral sides.
                        let leftness = az.to_radians().sin() * el.to_radians().cos();
                        let shadow = if leftness > 0.0 { 1 } else { 0 };
                        for (ear, spectrum) in output.iter().enumerate() {
                            for (k, h) in spectrum.iter().enumerate() {
                                let q = leftness.abs();
                                let alpha = 1.05 - 0.95 * q;
                                let x = 2.0 * 0.0875 / 343.0
                                    * 2.0
                                    * std::f64::consts::PI
                                    * k as f64
                                    * rate
                                    / n as f64;
                                let gain = if ear == shadow {
                                    ((1.0 + (alpha * x).powi(2)) / (1.0 + (1.05 * x).powi(2)))
                                        .powf(strength / 2.0)
                                } else {
                                    1.0
                                };
                                let expected = Complex::new(0.6_f64 * gain, -0.2_f64 * gain);
                                let error = (Complex::new(f64::from(h.re), f64::from(h.im))
                                    - expected)
                                    .norm();
                                maximum = maximum.max(error);
                                assert!(
                                    error < 2.0e-6,
                                    "shadow side/profile az={az} el={el} rate={rate} n={n} ear={ear} bin={k}: {h:?} vs {expected:?}"
                                );
                                if ear != shadow {
                                    assert_eq!(*h, Complex::new(0.6, -0.2));
                                }
                                assert!(
                                    h.norm() <= Complex::new(0.6_f32, -0.2).norm() + 1.0e-6,
                                    "shadowing must not boost"
                                );
                            }
                        }
                        cases += 1;
                    }
                }
            }
        }
    }
    println!("SHADOW_GEOMETRY cases={cases} maximum_complex_error={maximum}");
}

#[test]
fn shadow_profile_is_continuous_at_former_gates_seams_and_poles() {
    for az in [-180.0, -15.0, 0.0, 15.0, 180.0] {
        let before = shape(48000.0, 1024, az - 0.001, 0.0, 1.0);
        let after = shape(48000.0, 1024, az + 0.001, 0.0, 1.0);
        let error = before
            .iter()
            .flatten()
            .zip(after.iter().flatten())
            .map(|(a, b)| (*a - *b).norm())
            .fold(0.0_f32, f32::max);
        assert!(error < 5.0e-5, "angle jump at {az}: {error}");
    }
    let before = shape((50.0 - 0.001) * 1024.0, 1024, 90.0, 0.0, 1.0);
    let after = shape((50.0 + 0.001) * 1024.0, 1024, 90.0, 0.0, 1.0);
    assert!(
        (before[1][1] - after[1][1]).norm() < 5.0e-6,
        "50 Hz hard gate"
    );
    for el in [-90.0, 90.0] {
        for az in [-180.0, -90.0, 0.0, 90.0, 180.0] {
            assert!(
                shape(48000.0, 1024, az, el, 1.0)
                    .iter()
                    .flatten()
                    .all(|h| (*h - Complex::new(0.6, -0.2)).norm() < 1.0e-6)
            );
        }
    }
}

fn mirror(mut p: [f64; 3], wall: usize, d: [f64; 3]) -> [f64; 3] {
    let (axis, boundary) = match wall {
        0 => (1, d[1]),
        1 => (1, 0.0),
        2 => (0, 0.0),
        3 => (0, d[0]),
        4 => (2, 0.0),
        _ => (2, d[2]),
    };
    p[axis] = 2.0 * boundary - p[axis];
    p
}

#[test]
fn image_sources_bridge_room_right_forward_to_sofa_forward_left() {
    let mut paths = 0;
    for rate in [44100.0, 48000.0, 96000.0] {
        for channels in [1, 2, 6, 8, 12] {
            for order in [1, 2] {
                let config = get_speaker_config_by_channels(channels).unwrap();
                let room = RoomModel {
                    dimensions: [6.0, 7.0, 3.2],
                    listener_position: [2.5, 2.7, 1.1],
                    absorption: [0.13, 0.27, 0.21, 0.37, 0.46, 0.58],
                    max_order: order,
                    speed_of_sound: 343.0,
                };
                let actual = calculate_reflections(&room, config, rate);
                let dimensions = room.dimensions.map(f64::from);
                let listener = room.listener_position.map(f64::from);
                for spk in config.speakers {
                    let a = f64::from(spk.azimuth).to_radians();
                    let e = f64::from(spk.elevation).to_radians();
                    let source = [
                        listener[0] - 1.5 * a.sin() * e.cos(),
                        listener[1] + 1.5 * a.cos() * e.cos(),
                        listener[2] + 1.5 * e.sin(),
                    ];
                    let mut expected = Vec::new();
                    let mut seen: Vec<[f64; 3]> = Vec::new();
                    for first in 0..6 {
                        let p = mirror(source, first, dimensions);
                        expected.push((p, 1.0 - f64::from(room.absorption[first])));
                        if order == 2 {
                            for second in 0..6 {
                                if first != second {
                                    let image = mirror(p, second, dimensions);
                                    if !seen.iter().any(|q| {
                                        q.iter().zip(image).all(|(a, b)| (*a - b).abs() < 1e-8)
                                    }) {
                                        seen.push(image);
                                        expected.push((
                                            image,
                                            (1.0 - f64::from(room.absorption[first]))
                                                * (1.0 - f64::from(room.absorption[second])),
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    assert_eq!(actual[spk.channel].len(), expected.len());
                    let mut used = vec![false; expected.len()];
                    for (image, absorption) in expected {
                        let delta =
                            std::array::from_fn::<_, 3, _>(|axis| image[axis] - listener[axis]);
                        let distance = delta.iter().map(|x| x * x).sum::<f64>().sqrt();
                        let az = (-delta[0]).atan2(delta[1]).to_degrees();
                        let el = delta[2].atan2(delta[0].hypot(delta[1])).to_degrees();
                        let gain = 1.5 / distance * absorption;
                        let delay = ((distance - 1.5) / 343.0 * rate).round() as usize;
                        let found = actual[spk.channel].iter().enumerate().find(|(i, r)| {
                            !used[*i]
                                && ((f64::from(r.azimuth_deg) - az)
                                    .to_radians()
                                    .sin()
                                    .atan2((f64::from(r.azimuth_deg) - az).to_radians().cos())
                                    .to_degrees())
                                .abs()
                                    < 1.0e-4
                                && (f64::from(r.elevation_deg) - el).abs() < 1.0e-4
                                && r.delay_samples.abs_diff(delay) <= 1
                                && (f64::from(r.gain) - gain).abs() < 2.0e-6
                        });
                        let (index,r)=found.unwrap_or_else(|| panic!("physical image missing: channels={channels} rate={rate} order={order} speaker={} az={az} el={el} delay={delay} gain={gain}",spk.label));
                        used[index] = true;
                        let lateral = az.to_radians().sin();
                        assert!(
                            (f64::from(r.left_gain).powi(2) - (1.0 + lateral) * 0.5).abs() < 2.0e-6
                        );
                        assert!(
                            (f64::from(r.right_gain).powi(2) - (1.0 - lateral) * 0.5).abs()
                                < 2.0e-6
                        );
                        paths += 1;
                    }
                }
            }
        }
    }
    println!("ROOM_COORDINATES independent_paths={paths}");
}

#[test]
fn ssir_doa_and_fallback_pan_agree_on_positive_left() {
    use math_rir::{RirSegment, SsirResult};
    for az in [-150.0_f64, -90.0, -30.0, 0.0, 30.0, 90.0, 150.0] {
        let a = az.to_radians();
        let segment = |toa, direct, doa| RirSegment {
            onset_sample: toa,
            end_sample: toa + 1,
            toa_sample: toa,
            doa,
            peak_energy: 1.0,
            is_direct_sound: direct,
        };
        let result = SsirResult {
            segments: vec![
                segment(0, true, None),
                segment(3, false, Some([a.cos() as f32, a.sin() as f32, 0.0])),
            ],
            mixing_time_samples: 4,
            sample_rate: 48000.0,
        };
        let reflections = super::reflection::ssir_result_to_reflections(
            &result,
            &[1.0, 0.0, 0.0, 0.5],
            48000,
            48000.0,
        );
        let r = &reflections[0];
        assert!(
            ((f64::from(r.azimuth_deg) - az)
                .to_radians()
                .sin()
                .atan2((f64::from(r.azimuth_deg) - az).to_radians().cos())
                .to_degrees())
            .abs()
                < 1.0e-4
        );
        let expected_left = ((1.0 + a.sin()) * 0.5).sqrt();
        let expected_right = ((1.0 - a.sin()) * 0.5).sqrt();
        assert!(
            (f64::from(r.left_gain) - expected_left).abs() < 1.0e-6,
            "SSIR positive-left panning {az}"
        );
        assert!((f64::from(r.right_gain) - expected_right).abs() < 1.0e-6);
        let fallback = super::misc::azimuth_to_pan_gains(a as f32);
        assert!((f64::from(fallback.0) - expected_left).abs() < 1.0e-6);
        assert!((f64::from(fallback.1) - expected_right).abs() < 1.0e-6);
    }
}

#[test]
fn native_sofa_left_wall_reflection_is_left_in_both_metadata_and_audio() {
    for rate in [44100, 48000, 96000] {
        let file = crate::head_cue_tests::sofa(rate, 36, 0.35);
        let model = RoomModel {
            dimensions: [6.0, 7.0, 3.2],
            listener_position: [2.5, 2.7, 1.1],
            absorption: [1.0, 1.0, 0.0, 1.0, 1.0, 1.0],
            max_order: 1,
            speed_of_sound: 343.0,
        };
        let mut p = BinauralDecoderPlugin::new(
            1,
            256,
            Some(file.path.clone()),
            1.0,
            0.0,
            false,
            120.0,
            2.0,
            0.0,
            model,
        );
        p.initialize(f64::from(rate)).unwrap();
        let nonzero: Vec<_> = p.room.cached_reflections[0]
            .iter()
            .filter(|r| r.gain > 0.0)
            .collect();
        assert_eq!(nonzero.len(), 1);
        let r = nonzero[0];
        let h = r.hrtf_filter.as_ref().unwrap();
        let delay = r.delay_samples;
        let gain = r.gain;
        println!(
            "NATIVE_WALL rate={rate} az={} left_ild={} right_ild={}",
            r.azimuth_deg, h.left_gain_broadband, h.right_gain_broadband
        );
        assert!(
            r.azimuth_deg > 0.0
                && r.left_gain > r.right_gain
                && h.left_gain_broadband > h.right_gain_broadband,
            "left wall must remain left through SOFA lookup"
        );
        let mut actual = vec![0.0; 2];
        p.process(&[0.25], &mut actual, &ProcessContext::new(rate, 1))
            .unwrap();
        loop {
            let mut output = [0.0; 14];
            let result = p.drain(&mut output, &ProcessContext::new(rate, 7)).unwrap();
            actual.extend_from_slice(&output[..result.frames * 2]);
            if result.complete {
                break;
            }
        }
        let peak = (p.latency_samples() + delay) * 2;
        assert!((actual[peak] - gain * 0.25).abs() < 2.0e-6);
        assert!(actual[peak] > actual[peak + 1]);
    }
}

#[test]
fn native_near_field_cues_follow_head_relative_source_side() {
    for rate in [44100, 48000, 96000] {
        let file = crate::head_cue_tests::sofa(rate, 36, 0.0);
        for az in [-90.0_f32, 90.0] {
            let mut p = BinauralDecoderPlugin::new(
                1,
                256,
                Some(file.path.clone()),
                1.0,
                1.0,
                false,
                120.0,
                2.0,
                0.0,
                RoomModel {
                    max_order: 0,
                    ..Default::default()
                },
            );
            p.smoothing.head_yaw_deg.reset(-az);
            p.initialize(f64::from(rate)).unwrap();
            let state = p.state.load_full();
            let shadow = if az > 0.0 { 1 } else { 0 };
            let mut maximum = 0.0_f64;
            for bin in [0, 1, 7, 17, 63, 128] {
                let x = 2.0 * 0.0875 / 343.0
                    * 2.0
                    * std::f64::consts::PI
                    * bin as f64
                    * f64::from(rate)
                    / 256.0;
                let gain = ((1.0 + (0.1 * x).powi(2)) / (1.0 + (1.05 * x).powi(2))).sqrt();
                for ear in 0..2 {
                    let expected = 0.95 * if ear == shadow { gain } else { 1.0 };
                    let actual = f64::from(
                        state.hrtf_filters_freq[0][ear * p.config.freq_size + bin * 2].norm(),
                    );
                    maximum = maximum.max((actual - expected).abs());
                    assert!(
                        (actual - expected).abs() < 2.0e-5,
                        "native shadow ear {rate}/{az}/{ear}/{bin}: {actual} vs {expected}"
                    );
                }
            }
            println!(
                "NATIVE_SHADOW rate={rate} relative_azimuth={az} shadow_ear={shadow} maximum_gain_error={maximum}"
            );
        }
    }
}
