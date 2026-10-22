//! Shared-model preparation stages; this is not measured acoustic validation.

// Rust guideline compliant 2026-02-21
use super::*;
use crate::validation::evaluate_cancellation;

type Matrix = [[Complex<f32>; 2]; 2];

fn pack(matrix: Matrix) -> [[[f64; 2]; 2]; 2] {
    matrix.map(|row| row.map(|value| [f64::from(value.re), f64::from(value.im)]))
}

fn number(value: f64) -> serde_json::Value {
    if value.is_finite() {
        serde_json::json!(value)
    } else if value.is_nan() {
        serde_json::json!("nan")
    } else if value.is_sign_positive() {
        serde_json::json!("+inf")
    } else {
        serde_json::json!("-inf")
    }
}

#[test]
fn preparation_stage_replay_matches_production_and_retains_metrics() {
    let mut conditions = 0;
    let mut frequency_cases = 0;
    let mut records = 0;
    let mut maximum_reconstruction_error = 0.0_f32;
    for rate in [48_000_u32, 96_000] {
        for fft_size in [2048, 4096] {
            for head_model in [0, 1] {
                for mode in ["phase_only", "explicit_delay"] {
                    let params = XtcPluginParams {
                        fft_size,
                        head_model,
                        itd_modeling: mode.into(),
                        ..Default::default()
                    };
                    let bins = fft_size / 2 + 1;
                    let cache = compute_geometry_cache(&params, rate, bins);
                    let actual = compute_xtc_filters_full(&params, rate, bins);
                    let ceiling = 10.0_f32.powf(params.max_gain_db / 20.0);
                    for bin in [
                        0,
                        100 * fft_size / rate as usize,
                        200 * fft_size / rate as usize,
                        1000 * fft_size / rate as usize,
                        fft_size / 4,
                        fft_size / 2,
                    ] {
                        let frequency = bin as f32 * cache.freq_per_bin;
                        let plant = analytical_plant(&params, &cache, frequency);
                        let beta = compute_beta_condition_number_full(
                            plant[0][0],
                            plant[0][1],
                            plant[1][0],
                            plant[1][1],
                            frequency,
                            &params,
                        );
                        let inverse = |limit, bypass| {
                            let (ll, lr, rl, rr) = compute_full_2x2_inverse(
                                plant[0][0],
                                plant[0][1],
                                plant[1][0],
                                plant[1][1],
                                beta,
                                limit,
                                bypass,
                            );
                            [[ll, lr], [rl, rr]]
                        };
                        // Unbounded rows are counterfactuals: the refinement
                        // candidate choice itself can depend on the ceiling.
                        let regularized = inverse(1e6, true);
                        let refined = inverse(1e6, false);
                        assert!(
                            regularized
                                .iter()
                                .flatten()
                                .chain(refined.iter().flatten())
                                .all(|c| c.norm() < 500_000.0)
                        );
                        let limited = inverse(ceiling, params.bypass_neumann_refinement);
                        let normalized = normalize_filter_columns(plant, limited, ceiling);
                        let alpha = (1.0 - sigmoid_smooth(100.0 - frequency, 30.0))
                            * (1.0 - sigmoid_smooth(frequency - 12000.0, 1500.0));
                        let blended: Matrix = std::array::from_fn(|row| {
                            std::array::from_fn(|column| {
                                normalized[row][column] * alpha
                                    + Complex::new(
                                        if row == column { 1.0 - alpha } else { 0.0 },
                                        0.0,
                                    )
                            })
                        });
                        let mut ll = [blended[0][0]];
                        let mut lr = [blended[0][1]];
                        let mut rl = [blended[1][0]];
                        let mut rr = [blended[1][1]];
                        apply_effort_constraint(
                            &mut ll,
                            &mut lr,
                            Some(&mut rl),
                            Some(&mut rr),
                            ceiling,
                        );
                        let final_matrix = [[ll[0], lr[0]], [rl[0], rr[0]]];
                        let stored = [
                            [actual.filter_ll[bin], actual.filter_lr[bin]],
                            [
                                actual.filter_rl.as_ref().unwrap_or(&actual.filter_lr)[bin],
                                actual.filter_rr.as_ref().unwrap_or(&actual.filter_ll)[bin],
                            ],
                        ];
                        for (a, b) in final_matrix
                            .into_iter()
                            .flatten()
                            .zip(stored.into_iter().flatten())
                        {
                            let error = (a - b).norm();
                            maximum_reconstruction_error = maximum_reconstruction_error.max(error);
                            assert!(error < 3e-7);
                        }
                        for (stage, matrix) in [
                            ("regularized_unbounded", regularized),
                            ("refined_unbounded", refined),
                            ("inverse_limited", limited),
                            ("normalized_capped", normalized),
                            ("identity_blended", blended),
                            ("effort_limited", final_matrix),
                        ] {
                            let metrics = evaluate_cancellation(pack(plant), pack(matrix));
                            let powers = matrix.map(|row| {
                                row.map(|c| f64::from(c.re).powi(2) + f64::from(c.im).powi(2))
                                    .into_iter()
                                    .sum::<f64>()
                            });
                            if stage == "effort_limited" {
                                assert!(
                                    powers.iter().all(
                                        |power| *power <= f64::from(ceiling).powi(2) * 1.000002
                                    )
                                );
                            }
                            println!(
                                "{}",
                                serde_json::json!({"measurement":"xtc_preparation_stage","rate":rate,"fft_size":fft_size,"head_model":head_model,"itd_modeling":mode,"bin":bin,"frequency_hz":frequency,"beta":beta,"alpha":alpha,"ceiling":ceiling,"stage":stage,"plant":pack(plant),"filters":pack(matrix),"row_powers":powers,"stored_filters":if stage=="effort_limited" { Some(pack(stored)) } else { None },"raw_depth_db":metrics.raw_depth_db.map(number),"desired_gain_db":metrics.desired_gain_db.map(number),"separation_improvement_db":metrics.separation_improvement_db.map(number),"scope":"shared-model control-thread replay; unbounded stages are counterfactuals, not measured/native audio"})
                            );
                            records += 1;
                        }
                        frequency_cases += 1;
                    }
                    conditions += 1;
                }
            }
        }
    }
    println!(
        "preparation_stages conditions={conditions} frequencies={frequency_cases} records={records} maximum_reconstruction_error={maximum_reconstruction_error}"
    );
}
