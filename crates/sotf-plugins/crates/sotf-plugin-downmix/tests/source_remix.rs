//! Separate remix contracts and exact preservation of ordinary downmix behavior.

#![cfg(feature = "qa")]

// Rust guideline compliant 2026-02-21
use rustfft::num_complex::Complex;
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, ParameterId, ParameterValue, Plugin, ProcessContext};
use sotf_plugin_downmix::source_remix::{
    RemixMethod, RemixParams, SourceMask, SourceRemix, spatial_features,
};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;
const MODEL: &[u8] = include_bytes!("../../../models/source-aware-remix/model.synthetic.json");
const SHA: &str = "5fe02bae05ffe7994e755c93b89226d156803d1cd976e6406ba7315a8e6adb0c";
fn model() -> SourceMask {
    SourceMask::from_json(MODEL, SHA).unwrap()
}
fn params(method: RemixMethod) -> RemixParams {
    RemixParams {
        foreground_gain_db: 3.0,
        bed_gain_db: -3.0,
        mix: 1.0,
        method,
    }
}
fn source() -> Vec<f32> {
    (0..8197 * 2)
        .map(|i| ((i * 13 % 97) as f32 - 48.0) / 1024.0)
        .collect()
}
fn render(p: &mut SourceRemix, x: &[f32], rate: u32, callback: usize, drain: usize) -> Vec<f32> {
    let mut result = Vec::new();
    for start in (0..x.len() / 2).step_by(callback) {
        let count = (x.len() / 2 - start).min(callback);
        let mut out = vec![f32::NAN; count * 2];
        assert_no_allocs_or_deallocs("source-remix callback", || {
            assert_eq!(
                p.process(
                    &x[start * 2..(start + count) * 2],
                    &mut out,
                    &ProcessContext::new(rate, count)
                )
                .unwrap(),
                count
            )
        });
        result.extend(out);
    }
    let counters = p.counters();
    let mut out = vec![0.0; drain * 2];
    let bound = p.drain_call_bound().unwrap().get();
    let mut completed = false;
    for _ in 0..bound {
        let mut r = None;
        assert_no_allocs_or_deallocs("source-remix drain", || {
            r = Some(
                p.drain(&mut out, &ProcessContext::new(rate, drain))
                    .unwrap(),
            )
        });
        let r = r.unwrap();
        result.extend_from_slice(&out[..r.frames * 2]);
        if r.complete {
            completed = true;
            break;
        }
    }
    assert!(completed);
    assert_eq!(p.counters(), counters);
    result
}

#[test]
fn features_network_and_stereo_symmetry_match_independent_numpy_reference() {
    let m = model();
    let rows: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../models/source-aware-remix/parity.synthetic.json"
    ))
    .unwrap();
    let mut feature_error = 0.0_f64;
    let mut probability_error = 0.0_f64;
    for row in rows.as_array().unwrap() {
        let z = |v: &serde_json::Value| {
            Complex::new(v[0].as_f64().unwrap() as f32, v[1].as_f64().unwrap() as f32)
        };
        let l = z(&row["left"]);
        let r = z(&row["right"]);
        let k = row["bin"].as_u64().unwrap() as usize;
        let f = spatial_features(l, r, k).unwrap();
        assert_eq!(f, spatial_features(r, l, k).unwrap());
        for (i, &v) in f.iter().enumerate() {
            feature_error = feature_error.max((v - row["features"][i].as_f64().unwrap()).abs());
        }
        probability_error = probability_error
            .max((f64::from(m.predict(&f).unwrap()) - row["prediction"].as_f64().unwrap()).abs());
    }
    assert!(feature_error < 1e-12);
    assert!(probability_error < 3e-8);
    println!("source_remix_parity feature={feature_error} probability={probability_error}");
    assert!(SourceMask::from_json(MODEL, &"0".repeat(64)).is_err());
    assert!(m.predict(&[f64::NAN; 6]).is_none());
}

#[test]
fn missing_unsupported_and_neutral_remix_reconstruct_delayed_stereo() {
    let x = source();
    for rate in [44100, 48000, 96000, 192000] {
        for missing in [false, true] {
            let mut p = SourceRemix::new(
                params(RemixMethod::Learned),
                (!missing).then(model),
                f64::from(rate),
            )
            .unwrap();
            if !missing && rate != 192000 {
                p = SourceRemix::new(
                    RemixParams {
                        foreground_gain_db: 0.0,
                        bed_gain_db: 0.0,
                        ..params(RemixMethod::Learned)
                    },
                    Some(model()),
                    f64::from(rate),
                )
                .unwrap();
            }
            let y = render(&mut p, &x, rate, 193, 17);
            assert_eq!(p.latency_samples(), 512);
            assert!(y[..1024].iter().all(|&v| v == 0.0));
            for (a, b) in y[1024..1024 + x.len()].iter().zip(&x) {
                assert!((a - b).abs() < 3e-8);
            }
        }
    }
}

#[test]
fn learned_callbacks_drain_reset_and_swapped_channels_are_deterministic() {
    let x = source();
    let mut a = SourceRemix::new(params(RemixMethod::Learned), Some(model()), 48000.0).unwrap();
    let mut b = SourceRemix::new(params(RemixMethod::Learned), Some(model()), 48000.0).unwrap();
    let y = render(&mut a, &x, 48000, 17, 1);
    let other = render(&mut b, &x, 48000, 193, 256);
    assert_eq!(y, other);
    assert!(y.iter().all(|v| v.is_finite()));
    assert!(a.counters().foreground_bins + a.counters().bed_bins > 0);
    assert_no_allocs_or_deallocs("source-remix populated reset", || a.reset());
    assert_eq!(render(&mut a, &x, 48000, 256, 193), y);
    let mut swapped = x.clone();
    for frame in swapped.as_chunks_mut::<2>().0 {
        frame.swap(0, 1);
    }
    let mut p = SourceRemix::new(params(RemixMethod::Learned), Some(model()), 48000.0).unwrap();
    let swapped = render(&mut p, &swapped, 48000, 193, 17);
    for (a, b) in y.as_chunks::<2>().0.iter().zip(swapped.as_chunks::<2>().0) {
        assert!((a[0] - b[1]).abs() < 1e-7 && (a[1] - b[0]).abs() < 1e-7);
    }
}

#[test]
fn conditional_operator_and_complete_output_energy_remain_inside_explicit_gain_bound() {
    let x = source();
    let input_energy: f64 = x.iter().map(|v| f64::from(*v).powi(2)).sum();
    for a in [-6.0, 6.0] {
        for b in [-6.0, 6.0] {
            for method in [RemixMethod::Learned, RemixMethod::Analytic] {
                let mut p = SourceRemix::new(
                    RemixParams {
                        foreground_gain_db: a,
                        bed_gain_db: b,
                        mix: 1.0,
                        method,
                    },
                    Some(model()),
                    48000.0,
                )
                .unwrap();
                let y = render(&mut p, &x, 48000, 193, 17);
                let bound = 10_f32.powf(a.max(b).max(0.0) / 20.0);
                let (mid, side) = p.spectral_gains();
                assert!(
                    mid.iter()
                        .chain(side)
                        .all(|&v| v > 0.0 && v <= bound + 1e-6)
                );
                let energy: f64 = y.iter().map(|v| f64::from(*v).powi(2)).sum();
                assert!(energy <= input_energy * f64::from(bound).powi(2) * (1.0 + 2e-5));
            }
        }
    }
}

#[test]
fn rejected_calls_preserve_stream_and_primitive_controls_allocate_nothing() {
    let mut a = SourceRemix::new(params(RemixMethod::Learned), Some(model()), 48000.0).unwrap();
    let mut b = SourceRemix::new(params(RemixMethod::Learned), Some(model()), 48000.0).unwrap();
    let mut out = [1.0; 34];
    assert!(
        a.process(&[0.01; 34], &mut out, &ProcessContext::new(96000, 17))
            .is_err()
    );
    assert_eq!(out, [1.0; 34]);
    let mut invalid = [0.01; 34];
    invalid[7] = f32::NAN;
    assert!(
        a.process(&invalid, &mut out, &ProcessContext::new(48000, 17))
            .is_err()
    );
    assert_eq!(out, [1.0; 34]);
    let id = ParameterId::from("foreground_gain_db");
    assert!(
        a.set_parameter(id.clone(), ParameterValue::Float(7.0))
            .is_err()
    );
    assert_no_allocs_or_deallocs("source-remix primitive validation/set", || {
        a.validate_parameter(&id, &ParameterValue::Float(3.0))
            .unwrap();
        a.set_parameter(id.clone(), ParameterValue::Float(3.0))
            .unwrap();
        assert_eq!(a.get_parameter(&id), Some(ParameterValue::Float(3.0)));
    });
    let x = source();
    assert_eq!(
        render(&mut a, &x, 48000, 193, 17),
        render(&mut b, &x, 48000, 193, 17)
    );
}

#[test]
#[ignore = "Local callback budget evidence; run alone with one test thread"]
fn measure_explicit_remix_callback_budget() {
    use sotf_plugin_downmix::{DownmixPlugin, DownmixPluginParams};
    use std::time::Instant;
    let mut rows = Vec::new();
    let callback = 193;
    for rate in [44100, 48000, 96000] {
        for label in [
            "ordinary_downmix",
            "separate_bypass",
            "separate_analytic",
            "separate_learned",
        ] {
            let mut downmix = DownmixPlugin::try_from_params(DownmixPluginParams {
                input_channels: 6,
                input_layout: Some("5.1".into()),
                phase_coherence: false,
                itu_mode: true,
                matrix_ltrt: false,
                center_gain_db: -3.0,
                surround_gain_db: -3.0,
                height_gain_db: -6.0,
                lfe_gain_db: -20.0,
                phase_blend_low_hz: 300.0,
                phase_blend_high_hz: 2000.0,
            })
            .unwrap();
            downmix.initialize(f64::from(rate)).unwrap();
            let method = match label {
                "separate_analytic" => RemixMethod::Analytic,
                "separate_learned" => RemixMethod::Learned,
                _ => RemixMethod::Bypass,
            };
            let mut remix =
                SourceRemix::new(params(method), Some(model()), f64::from(rate)).unwrap();
            let input: Vec<_> = (0..callback * 6)
                .map(|i| ((i * 13 % 97) as f32 - 48.0) / 1024.0)
                .collect();
            let mut routed = vec![0.0; callback * 2];
            let mut output = routed.clone();
            let context = ProcessContext::new(rate, callback);
            for _ in 0..16 {
                downmix.process(&input, &mut routed, &context).unwrap();
                if label != "ordinary_downmix" {
                    remix.process(&routed, &mut output, &context).unwrap();
                }
            }
            let mut times = Vec::with_capacity(200);
            for _ in 0..200 {
                let start = Instant::now();
                downmix.process(&input, &mut routed, &context).unwrap();
                if label != "ordinary_downmix" {
                    remix.process(&routed, &mut output, &context).unwrap();
                }
                times.push(start.elapsed().as_secs_f64());
            }
            times.sort_by(f64::total_cmp);
            let deadline = callback as f64 / f64::from(rate);
            rows.push(serde_json::json!({"sample_rate":rate,"mode":label,"callback_frames":callback,"callbacks":200,"p50_seconds":times[100],"p95_seconds":times[190],"p99_seconds":times[198],"max_seconds":times[199],"callback_deadline_seconds":deadline,"declared_local_budget_seconds":0.8*deadline,"local_budget_pass":times[199]<=0.8*deadline,"latency_samples":if label=="ordinary_downmix"{0}else{512},"added_model_lookahead_samples":0}));
        }
    }
    println!(
        "{}",
        serde_json::json!({"cases":rows,"deployment_admitted":false,"scope":"Actual ITU5.1 downmix, optionally followed by explicit separate stereo remix. Local wall-clock budget fixed at80 percent of callback duration; not portable realtime or complete memory admission."})
    );
}
