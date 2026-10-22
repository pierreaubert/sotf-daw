//! End-to-end speaker-drive measurements with an independent synthetic acoustic plant.

// Rust guideline compliant 2026-02-21
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_xtc::validation::{ComplexTransferMatrix, evaluate_cancellation};
use sotf_plugin_xtc::{XtcPlugin, XtcPluginParams};
use std::f64::consts::TAU;

fn measured_pose_filter(
    bin: usize,
    chunks: &[usize],
    params: &XtcPluginParams,
    normalization: bool,
) -> ComplexTransferMatrix {
    const FFT: usize = 2048;
    const TOTAL: usize = FFT * 12;
    const START: usize = FFT * 8;
    const AMPLITUDE: f64 = 0.001;
    // Render each input cue as a column, then transpose to speaker x input.
    let columns: ComplexTransferMatrix = [0, 1].map(|input_channel| {
        let mut plugin = XtcPlugin::new(
            XtcPluginParams {
                fft_size: FFT,
                auto_gain_enabled: false,
                spectral_normalization: normalization,
                ..params.clone()
            },
            48_000,
        )
        .unwrap();
        plugin.initialize(48_000.0).unwrap();
        let input: Vec<f32> = (0..TOTAL)
            .flat_map(|frame| {
                let sample =
                    (AMPLITUDE * (TAU * bin as f64 * frame as f64 / FFT as f64).sin()) as f32;
                if input_channel == 0 {
                    [sample, 0.0]
                } else {
                    [0.0, sample]
                }
            })
            .collect();
        let mut output = vec![f32::NAN; input.len()];
        let mut frame = 0;
        for requested in chunks.iter().cycle() {
            let n = (*requested).min(TOTAL - frame);
            assert_eq!(
                plugin
                    .process(
                        &input[frame * 2..(frame + n) * 2],
                        &mut output[frame * 2..(frame + n) * 2],
                        &ProcessContext::new(48_000, n)
                    )
                    .unwrap(),
                n
            );
            frame += n;
            if frame == TOTAL {
                break;
            }
        }
        assert!(output.iter().all(|v| v.is_finite() && v.abs() < 0.1));
        let projection = |data: &[f32], channel: usize| {
            let mut real = 0.0;
            let mut imag = 0.0;
            for frame in START..TOTAL {
                let phase = TAU * bin as f64 * frame as f64 / FFT as f64;
                real += f64::from(data[frame * 2 + channel]) * phase.cos();
                imag -= f64::from(data[frame * 2 + channel]) * phase.sin();
            }
            [real, imag]
        };
        let source = projection(&input, input_channel);
        let denominator = source[0] * source[0] + source[1] * source[1];
        assert!(denominator > 1.0);
        std::array::from_fn(|speaker| {
            let target = projection(&output, speaker);
            [
                (target[0] * source[0] + target[1] * source[1]) / denominator,
                (target[1] * source[0] - target[0] * source[1]) / denominator,
            ]
        })
    });
    [
        [columns[0][0], columns[1][0]],
        [columns[0][1], columns[1][1]],
    ]
}

fn acoustic_plant(frequency: f64, head_x: f64, yaw: f64, echo: f64) -> ComplexTransferMatrix {
    // Independently specified free-field point sources plus one image source:
    // x right/y forward; ears rotate about the head center; pressure scales 1/r.
    // This is controlled mismatch, not a measured human HRTF or a real room.
    std::array::from_fn(|ear| {
        std::array::from_fn(|speaker| {
            let side = if ear == 0 { -0.0875 } else { 0.0875 };
            let x = head_x + side * yaw.cos();
            let y = side * yaw.sin();
            let sx = if speaker == 0 { -1.0 } else { 1.0 };
            let sy = 3.0_f64.sqrt();
            let direct = ((sx - x).powi(2) + (sy - y).powi(2)).sqrt();
            let reflected = ((sx - x).powi(2) + (sy + 2.0 + y).powi(2)).sqrt();
            let a = -TAU * frequency * direct / 343.0;
            let b = -TAU * frequency * reflected / 343.0;
            [
                2.0 / direct * a.cos() + echo * 2.0 / reflected * b.cos(),
                2.0 / direct * a.sin() + echo * 2.0 / reflected * b.sin(),
            ]
        })
    })
}

#[test]
fn rendered_cancellation_retains_signed_mismatch_and_intended_gain() {
    audit_rendered_mismatch(false);
}

#[test]
fn rendered_normalized_cancellation_retains_signed_mismatch_and_intended_gain() {
    audit_rendered_mismatch(true);
}

fn audit_rendered_mismatch(normalization: bool) {
    let mut cases = 0;
    let mut degraded = 0;
    let mut worst = f64::INFINITY;
    let mut maximum_partition_error = 0.0_f64;
    let params = XtcPluginParams::default();
    for bin in [1, 2, 4, 8, 10, 22, 43, 85, 171, 341] {
        let filter = measured_pose_filter(bin, &[512], &params, normalization);
        let irregular = measured_pose_filter(bin, &[1, 17, 257, 63, 137], &params, normalization);
        for (a, b) in filter
            .iter()
            .flatten()
            .flatten()
            .zip(irregular.iter().flatten().flatten())
        {
            maximum_partition_error = maximum_partition_error.max((a - b).abs());
            assert!((a - b).abs() < 2e-6);
        }
        let frequency = 48_000.0 * bin as f64 / 2048.0;
        println!(
            "{}",
            serde_json::json!({"measurement":"rendered_xtc_filter", "spectral_normalization":normalization,"sample_rate":48000,"fft_size":2048,"bin":bin,"frequency_hz":frequency,"input_amplitude":0.001,"total_frames":24576,"measure_start_frame":16384,"auto_gain_enabled":false,"filters":filter,"irregular_filters":irregular,"scope":"quiet rendered speaker transfer; independent controlled point-source/echo field follows"})
        );
        for x in [-0.1, -0.02, 0.0, 0.02, 0.1] {
            for yaw in [-0.3, 0.0, 0.3] {
                for echo in [0.0, 0.4] {
                    let metrics =
                        evaluate_cancellation(acoustic_plant(frequency, x, yaw, echo), filter);
                    for cue in 0..2 {
                        let depth = metrics.raw_depth_db[cue];
                        let normalized = metrics.separation_improvement_db[cue];
                        assert!(
                            depth.is_finite()
                                && normalized.is_finite()
                                && metrics.desired_gain_db[cue].is_finite()
                        );
                        if normalized < 0.0 {
                            degraded += 1;
                        }
                        worst = worst.min(normalized);
                        cases += 1;
                        println!(
                            "{}",
                            serde_json::json!({"measurement":"rendered_xtc_mismatch","spectral_normalization":normalization,
                        "frequency_hz":frequency,"head_x_m":x,"yaw_rad":yaw,"echo_gain":echo,
                        "cue":cue,"raw_depth_db":depth,"desired_gain_db":metrics.desired_gain_db[cue],
                        "separation_improvement_db":normalized})
                        );
                    }
                }
            }
        }
    }
    assert_eq!(cases, 600);
    assert!(
        degraded > 0,
        "mismatch must not be hidden as uniform improvement"
    );
    println!(
        "rendered_xtc_summary normalization={normalization} cases={cases} degraded={degraded} worst_normalized_db={worst} maximum_partition_error={maximum_partition_error}"
    );
}

#[test]
fn rendered_pose_mirrors_both_columns_and_keeps_callback_partition_invariance() {
    let mut total = 0;
    for normalization in [false, true] {
        let mut maximum_mirror_error = 0.0_f64;
        let mut maximum_partition_error = 0.0_f64;
        let mut cases = 0;
        for model in [0, 1] {
            for mode in ["phase_only", "explicit_delay"] {
                for (x, yaw, z) in [
                    (0.1, 0.0, 0.0),
                    (0.04, 20.0, 0.0),
                    (0.0, 0.0, -0.5),
                    (0.0, 0.0, 0.5),
                    (0.1, 0.0, -0.5),
                    (0.04, 20.0, 0.5),
                    (0.0, 90.0, 0.0),
                    (0.0, -90.0, 0.0),
                    (0.1, 75.0, 0.5),
                    (-0.1, -75.0, 0.5),
                ] {
                    let params = XtcPluginParams {
                        head_offset_x: x,
                        head_offset_z: z,
                        head_yaw_deg: yaw,
                        head_model: model,
                        itd_modeling: mode.into(),
                        ..Default::default()
                    };
                    let mirrored = XtcPluginParams {
                        head_offset_x: -x,
                        head_yaw_deg: -yaw,
                        ..params.clone()
                    };
                    for bin in [8, 85, 341] {
                        let a = measured_pose_filter(bin, &[512], &params, normalization);
                        let b = measured_pose_filter(
                            bin,
                            &[1, 17, 257, 63, 137],
                            &params,
                            normalization,
                        );
                        let mirror = measured_pose_filter(
                            bin,
                            &[1, 17, 257, 63, 137],
                            &mirrored,
                            normalization,
                        );
                        for row in 0..2 {
                            for col in 0..2 {
                                for part in 0..2 {
                                    let partition_error =
                                        (a[row][col][part] - b[row][col][part]).abs();
                                    let mirror_error =
                                        (a[row][col][part] - mirror[1 - row][1 - col][part]).abs();
                                    maximum_partition_error =
                                        maximum_partition_error.max(partition_error);
                                    maximum_mirror_error = maximum_mirror_error.max(mirror_error);
                                    assert!(partition_error < 2e-6);
                                    assert!(mirror_error < 2e-6);
                                }
                            }
                        }
                        cases += 1;
                    }
                }
            }
        }
        println!(
            "rendered_pose normalization={normalization} cases={cases} maximum_mirror_error={maximum_mirror_error} maximum_partition_error={maximum_partition_error}"
        );
        assert_eq!(cases, 120);
        total += cases;
    }
    assert_eq!(total, 240);
    println!("rendered_pose_total cases={total}");
}
