//! Rendered XTC validation against measured KEMAR transfers and held-out yaw directions.
//!
//! Gardner and Martin, MIT Media Laboratory (1994). See the shared fixture's
//! attribution and SHA manifest in sotf-plugin-binaural/tests/fixtures/kemar.

// Rust guideline compliant 2026-02-21
use sotf_host::sofa::{SofaFile, SourcePosition, load_sofa};
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_xtc::validation::{
    ComplexTransferMatrix, bound_cancellation, evaluate_cancellation, predict_analytic_cancellation,
};
use sotf_plugin_xtc::{XtcPlugin, XtcPluginParams};

const RATE: u32 = 44_100;
const FFT: usize = 2048;

fn measured_plant(sofa: &SofaFile, bin: usize, yaw: f32) -> ComplexTransferMatrix {
    let columns = [30.0_f32 + yaw, -30.0 + yaw].map(|azimuth| {
        let h = sofa
            .get_hrtf_at_position(&SourcePosition::new(azimuth, 0.0, 1.4))
            .unwrap();
        [h.ir_left, h.ir_right].map(|ir| {
            let mut value = [0.0_f64; 2];
            for (i, &sample) in ir.iter().enumerate() {
                let phase = -std::f64::consts::TAU * bin as f64 * i as f64 / FFT as f64;
                value[0] += f64::from(sample) * phase.cos();
                value[1] += f64::from(sample) * phase.sin();
            }
            value
        })
    });
    [
        [columns[0][0], columns[1][0]],
        [columns[0][1], columns[1][1]],
    ]
}

fn render_filter(params: &XtcPluginParams, bin: usize) -> ComplexTransferMatrix {
    const FRAMES: usize = FFT * 12;
    const START: usize = FFT * 8;
    let columns = [0, 1].map(|cue| {
        let mut plugin = XtcPlugin::new(params.clone(), RATE).unwrap();
        plugin.initialize(f64::from(RATE)).unwrap();
        let input: Vec<_> = (0..FRAMES)
            .flat_map(|frame| {
                let sample = (0.001
                    * (std::f64::consts::TAU * bin as f64 * frame as f64 / FFT as f64).sin())
                    as f32;
                if cue == 0 {
                    [sample, 0.0]
                } else {
                    [0.0, sample]
                }
            })
            .collect();
        let mut output = vec![f32::NAN; FRAMES * 2];
        let mut offset = 0;
        for count in [127, 512, 31, 257].into_iter().cycle() {
            let n = count.min(FRAMES - offset);
            if n == 0 {
                break;
            }
            assert_eq!(
                plugin
                    .process(
                        &input[offset * 2..(offset + n) * 2],
                        &mut output[offset * 2..(offset + n) * 2],
                        &ProcessContext::new(RATE, n)
                    )
                    .unwrap(),
                n
            );
            offset += n;
        }
        assert!(output.iter().all(|v| v.is_finite() && v.abs() < 0.1));
        let projection = |data: &[f32], channel| {
            let mut value = [0.0_f64; 2];
            for frame in START..FRAMES {
                let phase = std::f64::consts::TAU * bin as f64 * frame as f64 / FFT as f64;
                value[0] += f64::from(data[frame * 2 + channel]) * phase.cos();
                value[1] -= f64::from(data[frame * 2 + channel]) * phase.sin();
            }
            value
        };
        let source = projection(&input, cue);
        let power = source[0].powi(2) + source[1].powi(2);
        assert!(power > 1.0);
        [0, 1].map(|speaker| {
            let target = projection(&output, speaker);
            [
                (target[0] * source[0] + target[1] * source[1]) / power,
                (target[1] * source[0] - target[0] * source[1]) / power,
            ]
        })
    });
    [
        [columns[0][0], columns[1][0]],
        [columns[0][1], columns[1][1]],
    ]
}

#[test]
fn measured_design_and_held_out_yaw_metrics_preserve_signed_degradation() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../sotf-plugin-binaural/tests/fixtures/kemar/mit_kemar_normal_pinna.sofa");
    let sofa = load_sofa(&path).unwrap().data;
    let mut cases = 0;
    let mut degraded = 0;
    let mut worst = f64::INFINITY;
    for normalization in [false, true] {
        // Only the +/-30 degree measured directions enter inverse preparation.
        // The other four yaw cases use distinct measured directions after design.
        let params = XtcPluginParams {
            source_mode: "hrtf_file".into(),
            hrtf_file: Some(path.to_string_lossy().into_owned()),
            distance_m: 1.4,
            fft_size: FFT,
            auto_gain_enabled: false,
            spectral_normalization: normalization,
            ..Default::default()
        };
        let calibration_indices = [30.0_f32, -30.0]
            .map(|az| sofa.find_three_nearest(&SourcePosition::new(az, 0.0, 1.4))[0].0);
        for bin in [4, 8, 22, 43, 85, 171, 341] {
            let frequency = f64::from(RATE) * bin as f64 / FFT as f64;
            assert!(predict_analytic_cancellation(&params, RATE, frequency as f32).is_none());
            let filter = render_filter(&params, bin);
            let nominal = measured_plant(&sofa, bin, 0.0);
            println!(
                "{}",
                serde_json::json!({"measurement":"measured_kemar_rendered_filter","normalization":normalization,"rate":RATE,"fft":FFT,"bin":bin,"frequency_hz":frequency,"calibration_indices":calibration_indices,"filters":filter,"nominal_measured_plant":nominal})
            );
            for yaw in [-10.0_f32, -5.0, 0.0, 5.0, 10.0] {
                let indices = [30.0 + yaw, -30.0 + yaw]
                    .map(|az| sofa.find_three_nearest(&SourcePosition::new(az, 0.0, 1.4))[0].0);
                if yaw != 0.0 {
                    assert!(indices.iter().all(|i| !calibration_indices.contains(i)));
                }
                let plant = measured_plant(&sofa, bin, yaw);
                let metrics = evaluate_cancellation(plant, filter);
                let row_error = std::array::from_fn(|ear| {
                    (0..2)
                        .map(|speaker| {
                            let [a, b] = plant[ear][speaker];
                            let [c, d] = nominal[ear][speaker];
                            (a - c).powi(2) + (b - d).powi(2)
                        })
                        .sum::<f64>()
                        .sqrt()
                });
                let envelope = bound_cancellation(nominal, filter, row_error).unwrap();
                for (cue, depth) in metrics.raw_depth_db.into_iter().enumerate() {
                    let normalized = metrics.separation_improvement_db[cue];
                    assert!(
                        normalized.is_finite()
                            && depth.is_finite()
                            && metrics.desired_gain_db[cue].is_finite()
                    );
                    let response_amplitude = |ear: usize| {
                        let mut out = [0.0_f64; 2];
                        for speaker in 0..2 {
                            let [a, b] = plant[ear][speaker];
                            let [c, d] = filter[speaker][cue];
                            out[0] += a * c - b * d;
                            out[1] += a * d + b * c;
                        }
                        out[0].hypot(out[1])
                    };
                    for (actual, interval) in [
                        (response_amplitude(cue), envelope.desired_amplitude[cue]),
                        (
                            response_amplitude(1 - cue),
                            envelope.undesired_amplitude[cue],
                        ),
                    ] {
                        let slack = 1e-10 * (1.0 + interval[1]);
                        assert!(actual + slack >= interval[0] && actual <= interval[1] + slack);
                    }
                    degraded += usize::from(normalized < 0.0);
                    worst = worst.min(normalized);
                    cases += 1;
                    println!(
                        "{}",
                        serde_json::json!({"measurement":"measured_kemar_yaw","normalization":normalization,"frequency_hz":frequency,"yaw_deg":yaw,"held_out":yaw!=0.0,"measurement_indices":indices,"cue":cue,"raw_depth_db":metrics.raw_depth_db[cue],"desired_gain_db":metrics.desired_gain_db[cue],"separation_improvement_db":normalized,"plant":plant,"row_error_from_calibration":row_error,"desired_amplitude_interval":envelope.desired_amplitude[cue],"undesired_amplitude_interval":envelope.undesired_amplitude[cue],"scope":"measured_mannequin_directions_no_human_or_native_ABI_claim"})
                    );
                }
            }
        }
    }
    println!(
        "MEASURED_KEMAR_XTC_SUMMARY cues={cases} degraded={degraded} worst_separation_improvement_db={worst}"
    );
    assert_eq!(cases, 140);
}
