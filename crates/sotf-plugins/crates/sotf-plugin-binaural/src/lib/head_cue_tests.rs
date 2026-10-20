//! Independent head geometry and real SOFA impulse-cue checks.

use crate::{BinauralDecoderPlugin, RoomModel};
use sofa_reader::SofaWriter;
use sotf_host::{ParameterValue, Plugin, ProcessContext};
use std::path::PathBuf;

fn decoder(rate: u32) -> BinauralDecoderPlugin {
    let mut p = BinauralDecoderPlugin::new(
        1,
        256,
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
    p.set_parameter("crossfade_ms".into(), ParameterValue::Float(10.0))
        .unwrap();
    p
}

fn distance(a: f32, b: f32) -> f64 {
    let a = f64::from(a).to_radians();
    let b = f64::from(b).to_radians();
    ((a.sin() * b.cos() - a.cos() * b.sin()).atan2(a.cos() * b.cos() + a.sin() * b.sin()))
        .to_degrees()
        .abs()
}

#[test]
fn component_seam_crossings_take_the_short_arc_and_keep_public_targets() {
    for rate in [44_100, 48_000, 96_000] {
        for axis in 0..3 {
            for (start, target) in [
                (179.0, -179.0),
                (-179.0, 179.0),
                (180.0, -180.0),
                (-180.0, 180.0),
            ] {
                let mut p = decoder(rate);
                let smoother = match axis {
                    0 => &mut p.smoothing.head_yaw_deg,
                    1 => &mut p.smoothing.head_pitch_deg,
                    _ => &mut p.smoothing.head_roll_deg,
                };
                smoother.reset(start);
                smoother.set_target(target);
                let hop = p.config.hop_size;
                for step in 0..40 {
                    p.process(
                        &vec![0.0; hop],
                        &mut vec![0.0; hop * 2],
                        &ProcessContext::new(rate, hop),
                    )
                    .unwrap();
                    let current = match axis {
                        0 => p.smoothing.head_yaw_deg.current(),
                        1 => p.smoothing.head_pitch_deg.current(),
                        _ => p.smoothing.head_roll_deg.current(),
                    };
                    assert!(
                        distance(start, current) <= distance(start, target) + 0.001,
                        "long-way turn {rate}/{axis}: {start} to {target} via {current}"
                    );
                    let difference = f64::from(target - start).to_radians();
                    let delta = difference.sin().atan2(difference.cos()).to_degrees();
                    let expected = f64::from(start)
                        + delta
                            * (1.0
                                - (-(((step + 1) * hop) as f64) / (f64::from(rate) * 0.010)).exp());
                    assert!(
                        distance(current, expected as f32) < 0.02,
                        "short-arc timing {rate}/{axis}/{step}: {current} vs {expected}"
                    );
                }
                let id = ["head_yaw_deg", "head_pitch_deg", "head_roll_deg"][axis];
                assert_eq!(
                    p.get_parameter(&id.into()),
                    Some(ParameterValue::Float(target))
                );
            }
        }
    }
}

#[test]
fn orientation_request_deadband_is_circular_at_the_seam() {
    for rate in [44_100, 48_000, 96_000] {
        let mut p = decoder(rate);
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        p.hrtf_update_tx = Some(tx);
        p.smoothing.head_yaw_deg.reset(179.99);
        p.smoothing.head_yaw_deg.set_target(-179.99);
        p.smoothing.last_hrtf_yaw = 179.99;
        p.smoothing.last_hrtf_pitch = 0.0;
        p.smoothing.last_hrtf_roll = 0.0;
        for _ in 0..80 {
            p.process(&[0.0; 64], &mut [0.0; 128], &ProcessContext::new(rate, 64))
                .unwrap();
        }
        assert!(
            rx.try_recv().is_err(),
            "subthreshold seam turn created an HRTF request"
        );
    }
}

#[test]
fn inverse_rotation_matches_independent_forward_left_up_head_basis() {
    let mut maximum = 0.0_f64;
    for yaw in [-179.0_f64, -60.0, 0.0, 60.0, 179.0] {
        for pitch in [-60.0_f64, 0.0, 60.0] {
            for roll in [-45.0_f64, 0.0, 45.0] {
                let (sy, cy) = yaw.to_radians().sin_cos();
                let (sp, cp) = pitch.to_radians().sin_cos();
                let (sr, cr) = roll.to_radians().sin_cos();
                // Positive yaw looks left, positive pitch looks up, positive roll
                // raises the left ear. Project world directions onto this physical basis.
                let forward = [cy * cp, sy * cp, sp];
                let left = [-sy * cr - cy * sp * sr, cy * cr - sy * sp * sr, cp * sr];
                let up = [sy * sr - cy * sp * cr, -cy * sr - sy * sp * cr, cp * cr];
                for az in [-179.0_f64, -60.0, 0.0, 60.0, 179.0] {
                    for el in [-60.0_f64, 0.0, 60.0] {
                        let world = [
                            el.to_radians().cos() * az.to_radians().cos(),
                            el.to_radians().cos() * az.to_radians().sin(),
                            el.to_radians().sin(),
                        ];
                        let dot = |basis: [f64; 3]| {
                            basis.iter().zip(world).map(|(a, b)| a * b).sum::<f64>()
                        };
                        let expected = [dot(forward), dot(left), dot(up)];
                        let (a, e) = BinauralDecoderPlugin::rotate_speaker_position(
                            az as f32,
                            el as f32,
                            yaw as f32,
                            pitch as f32,
                            roll as f32,
                        );
                        let a = f64::from(a).to_radians();
                        let e = f64::from(e).to_radians();
                        let actual = [e.cos() * a.cos(), e.cos() * a.sin(), e.sin()];
                        let error = actual
                            .iter()
                            .zip(expected)
                            .map(|(a, b)| (a - b) * (a - b))
                            .sum::<f64>()
                            .sqrt();
                        maximum = maximum.max(error);
                        assert!(error < 2e-6);
                    }
                }
            }
        }
    }
    eprintln!("HEAD_BASIS cases=675 maximum_cartesian_error={maximum:e}");
}

pub(super) struct Fixture {
    directory: PathBuf,
    pub(super) path: PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

pub(super) fn sofa(rate: u32, measurements: usize, strength: f64) -> Fixture {
    let base = std::env::var_os("SOTF_AUDIT_TMP")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&base).unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = base.join(format!("binaural-cues-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("fixture.sofa");
    let mut writer = SofaWriter::new();
    writer.add_attribute_str("Conventions", "SOFA");
    writer.add_attribute_str("SOFAConventions", "SimpleFreeFieldHRIR");
    for (name, size) in [("I", 1), ("M", measurements), ("R", 2), ("C", 3), ("N", 16)] {
        writer.add_dimension(name, size);
    }
    writer.add_variable_f64("Data.SamplingRate", &["I"]);
    writer
        .write_f64("Data.SamplingRate", &[f64::from(rate)])
        .unwrap();
    let mut positions = Vec::new();
    let mut ir = vec![0.0; measurements * 32];
    for measurement in 0..measurements {
        let az = -180.0 + 360.0 * measurement as f64 / measurements as f64;
        positions.extend_from_slice(&[az, 0.0, 1.0]);
        let shift = (2.0 * az.to_radians().sin()).round() as i32;
        ir[measurement * 32 + (4 - shift) as usize] = 1.0 + strength * az.to_radians().sin();
        ir[measurement * 32 + 16 + (4 + shift) as usize] = 1.0 - strength * az.to_radians().sin();
    }
    writer.add_variable_f64("SourcePosition", &["M", "C"]);
    writer.write_f64("SourcePosition", &positions).unwrap();
    writer.add_variable_f64("Data.IR", &["M", "R", "N"]);
    writer.write_f64("Data.IR", &ir).unwrap();
    // Known source-sample delays in both supported standard layouts.
    if measurements == 72 {
        writer.add_variable_f64("Data.Delay", &["I", "R"]);
        writer.write_f64("Data.Delay", &[3.0, 5.0]).unwrap();
    } else {
        writer.add_variable_f64("Data.Delay", &["M", "R"]);
        writer
            .write_f64(
                "Data.Delay",
                &(0..measurements)
                    .flat_map(|m| {
                        let sine = (-180.0 + 360.0 * m as f64 / measurements as f64)
                            .to_radians()
                            .sin();
                        [
                            3.0 + if sine > 0.5 { 1.0 } else { 0.0 },
                            5.0 + if sine < -0.5 { 1.0 } else { 0.0 },
                        ]
                    })
                    .collect::<Vec<_>>(),
            )
            .unwrap();
    }
    writer.finish(&path).unwrap();
    Fixture { directory, path }
}

fn gains(yaw: f32, strength: f64) -> [f64; 2] {
    let sine = (-f64::from(yaw)).to_radians().sin();
    let left = 1.0 + strength * sine;
    let right = 1.0 - strength * sine;
    let scale = 0.95 / left.max(right);
    [left * scale, right * scale]
}

fn delays(yaw: f32, measurement_specific: bool) -> [usize; 2] {
    let shift = (2.0 * (-f64::from(yaw)).to_radians().sin()).round() as i32;
    let sine = (-f64::from(yaw)).to_radians().sin();
    [
        (7 - shift) as usize + usize::from(measurement_specific && sine > 0.5),
        (9 + shift) as usize + usize::from(measurement_specific && sine < -0.5),
    ]
}

fn prepared_cues(p: &BinauralDecoderPlugin, yaw: f32, strength: f64, measurement_specific: bool) {
    let state = p.state.load();
    let expected = gains(yaw, strength);
    for (ear, &expected_gain) in expected.iter().enumerate() {
        let actual = f64::from(state.hrtf_filters_freq[0][ear * p.config.freq_size].re);
        assert!(
            (actual - expected_gain).abs() < 2e-5,
            "prepared pose {yaw}, ear {ear}: {actual} != {}",
            expected_gain
        );
        for bin in [1, 7, 37, p.config.fft_size / 2] {
            let phase =
                -std::f64::consts::TAU * bin as f64 * delays(yaw, measurement_specific)[ear] as f64
                    / p.config.fft_size as f64;
            let actual = state.hrtf_filters_freq[0][ear * p.config.freq_size + bin];
            let error = ((f64::from(actual.re) - expected_gain * phase.cos()).powi(2)
                + (f64::from(actual.im) - expected_gain * phase.sin()).powi(2))
            .sqrt();
            assert!(
                error < 2e-5,
                "prepared ITD {yaw}/ear {ear}/bin {bin}: {error}"
            );
        }
    }
}

fn pulse(
    p: &mut BinauralDecoderPlugin,
    rate: u32,
    yaw: f32,
    strength: f64,
    measurement_specific: bool,
) {
    // Exercise the supported public 10 ms resource crossfade, then probe its
    // settled transfer. The following 11-sample clip still ends inside a hop.
    let fade_frames = (f64::from(rate) * 0.015).ceil() as usize;
    let warm = fade_frames.div_ceil(p.config.hop_size) * p.config.hop_size + 3 * p.config.fft_size;
    p.process(
        &vec![0.0; warm],
        &mut vec![0.0; warm * 2],
        &ProcessContext::new(rate, warm),
    )
    .unwrap();
    let mut input = [0.0; 11];
    input[3] = 0.25;
    let mut output = vec![0.0; 22];
    p.process(&input, &mut output, &ProcessContext::new(rate, 11))
        .unwrap();
    let mut tail = [0.0; 34];
    loop {
        let r = p.drain(&mut tail, &ProcessContext::new(rate, 0)).unwrap();
        output.extend_from_slice(&tail[..r.frames * 2]);
        if r.complete {
            break;
        }
    }
    let expected = gains(yaw, strength);
    let latency = p.latency_samples();
    for frame in 0..output.len() / 2 {
        for (ear, &expected_gain) in expected.iter().enumerate() {
            let value = if frame == latency + 3 + delays(yaw, measurement_specific)[ear] {
                0.25 * expected_gain
            } else {
                0.0
            };
            assert!(
                (f64::from(output[frame * 2 + ear]) - value).abs() < 2e-6,
                "audio cue {rate}/{yaw}/ear {ear}/frame {frame}: {} != {value}",
                output[frame * 2 + ear]
            );
        }
    }
}

#[test]
fn real_sofa_initialization_and_replacement_preserve_pose_and_impulse_cues() {
    for rate in [44_100, 48_000, 96_000] {
        let first = sofa(rate, 72, 0.25);
        let second = sofa(rate, 36, -0.4);
        for yaw in [-180.0, -90.0, 0.0, 90.0, 180.0] {
            let mut p = decoder(rate);
            p.smoothing.head_yaw_deg.reset(yaw);
            p.set_parameter(
                "hrtf_file".into(),
                ParameterValue::String(first.path.to_string_lossy().into_owned()),
            )
            .unwrap();
            prepared_cues(&p, yaw, 0.25, false);
            p.initialize(f64::from(rate)).unwrap();
            prepared_cues(&p, yaw, 0.25, false);
            pulse(&mut p, rate, yaw, 0.25, false);
            p.reset();
            p.set_parameter(
                "hrtf_file".into(),
                ParameterValue::String(second.path.to_string_lossy().into_owned()),
            )
            .unwrap();
            prepared_cues(&p, yaw, -0.4, true);
            pulse(&mut p, rate, yaw, -0.4, true);
            eprintln!(
                "SOFA_POSE rate={rate} yaw={yaw} initialize_and_replace=true directional_delays_and_metadata=true"
            );
        }
    }
}
