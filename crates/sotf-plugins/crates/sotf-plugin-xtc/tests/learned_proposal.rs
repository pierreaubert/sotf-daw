//! Prepared inverse resource bounds, transactional ownership and native audio.
#![cfg(feature = "qa")]
// Rust guideline compliant 2026-02-21
use sha2::{Digest, Sha256};
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, Plugin, ProcessContext};
use sotf_plugin_xtc::learned_proposal::PreparedProposal;
use sotf_plugin_xtc::{XtcPlugin, XtcPluginParams};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

const MODEL: &str = "157b1f395717fcebb669d57e61450214823695f520dcef83545d35ce7c161248";
const BYTES: &[u8] =
    include_bytes!("../../../../../audit/artifacts/ml-xtc-transfers-20261026/proposal.json");

fn sha(raw: &[u8]) -> String {
    Sha256::digest(raw)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn proposal() -> PreparedProposal {
    PreparedProposal::from_json(BYTES, &sha(BYTES), MODEL).unwrap()
}
fn make(rate: u32, selected: bool) -> XtcPlugin {
    let params = XtcPluginParams {
        distance_m: 1.4,
        speaker_angle_deg: 30.0,
        auto_gain_enabled: false,
        ..Default::default()
    };
    let mut p = XtcPlugin::new(params, rate).unwrap();
    p.initialize(f64::from(rate)).unwrap();
    if selected {
        p.install_learned_proposal(proposal()).unwrap();
    }
    p
}
fn source() -> Vec<f32> {
    (0..5101)
        .flat_map(|i| {
            [
                0.002 * (i as f32 * 0.17).sin(),
                0.003 * (i as f32 * 0.31).cos(),
            ]
        })
        .collect()
}
fn render(p: &mut XtcPlugin, x: &[f32], callback: usize, rate: u32) -> Vec<f32> {
    let mut block = vec![0.0; callback * 2];
    let mut result = Vec::new();
    for input in x.chunks(callback * 2) {
        let frames = input.len() / 2;
        assert_no_allocs_or_deallocs("prepared XTC process", || {
            p.process(
                input,
                &mut block[..frames * 2],
                &ProcessContext::new(rate, frames),
            )
            .unwrap();
        });
        result.extend_from_slice(&block[..frames * 2]);
    }
    let mut complete = false;
    for _ in 0..20000 {
        let mut drained = None;
        assert_no_allocs_or_deallocs("prepared XTC drain", || {
            drained = Some(
                p.drain(&mut block, &ProcessContext::new(rate, callback))
                    .unwrap(),
            );
        });
        let drained = drained.unwrap();
        result.extend_from_slice(&block[..drained.frames * 2]);
        if drained.complete {
            complete = true;
            break;
        }
    }
    assert!(complete);
    result
}

#[test]
fn frozen_proposal_has_bounded_f32_gain_and_correct_coefficient_ownership() {
    let p = proposal();
    assert!(p.maximum_operator_gain() <= 4.0);
    let raw: serde_json::Value = serde_json::from_slice(BYTES).unwrap();
    for bin in [0, 5, 93, 372, 1024] {
        for speaker in 0..2 {
            for input in 0..2 {
                let actual = p.coefficient(bin, speaker, input).unwrap();
                for axis in 0..2 {
                    let expected = raw["filters_real_imag"][bin][speaker][input][axis]
                        .as_f64()
                        .unwrap();
                    assert!((f64::from(actual[axis]) - expected).abs() < 5e-6);
                }
            }
        }
    }
    assert!(p.coefficient(1025, 0, 0).is_none());
}

#[test]
fn strict_import_rejects_gain_geometry_identity_and_complex_endpoints() {
    for key in [
        "sample_rate",
        "fft_size",
        "model_sha256",
        "deployment_admitted",
        "native_imported",
    ] {
        let mut raw: serde_json::Value = serde_json::from_slice(BYTES).unwrap();
        raw[key] = serde_json::json!(true);
        let bytes = serde_json::to_vec(&raw).unwrap();
        assert!(PreparedProposal::from_json(&bytes, &sha(&bytes), MODEL).is_err());
    }
    let mut raw: serde_json::Value = serde_json::from_slice(BYTES).unwrap();
    raw["filters_real_imag"][5] =
        serde_json::json!([[[4.0, 0.0], [4.0, 0.0]], [[4.0, 0.0], [4.0, 0.0]]]);
    let bytes = serde_json::to_vec(&raw).unwrap();
    assert!(PreparedProposal::from_json(&bytes, &sha(&bytes), MODEL).is_err());
    raw = serde_json::from_slice(BYTES).unwrap();
    raw["filters_real_imag"][0][0][0][1] = serde_json::json!(0.01);
    let bytes = serde_json::to_vec(&raw).unwrap();
    assert!(PreparedProposal::from_json(&bytes, &sha(&bytes), MODEL).is_err());
    assert!(PreparedProposal::from_json(BYTES, &"0".repeat(64), MODEL).is_err());
}

#[test]
fn native_render_and_reset_are_callback_independent_without_heap() {
    let x = source();
    let mut a = make(44100, true);
    let mut b = make(44100, true);
    let y = render(&mut a, &x, 1, 44100);
    assert_eq!(y, render(&mut b, &x, 257, 44100));
    assert!(y.iter().all(|v| v.is_finite()));
    assert!(y[..a.latency_samples() * 2].iter().all(|v| *v == 0.0));
    assert_eq!(a.learned_proposal_sha256(), Some(sha(BYTES).as_str()));
    assert_no_allocs_or_deallocs("prepared XTC reset", || a.reset());
    assert_eq!(y, render(&mut a, &x, 113, 44100));
    let mut native = make(44100, false);
    assert_ne!(y, render(&mut native, &x, 113, 44100));
}

#[test]
fn rejected_live_or_wrong_profile_installation_preserves_native_state() {
    let x = source();
    let mut a = make(44100, false);
    let mut b = make(44100, false);
    let mut out_a = [0.0; 2];
    let mut out_b = [0.0; 2];
    a.process(&x[..2], &mut out_a, &ProcessContext::new(44100, 1))
        .unwrap();
    b.process(&x[..2], &mut out_b, &ProcessContext::new(44100, 1))
        .unwrap();
    assert!(a.install_learned_proposal(proposal()).is_err());
    assert_eq!(
        render(&mut a, &x[2..], 113, 44100),
        render(&mut b, &x[2..], 113, 44100)
    );
    let mut wrong = make(48000, false);
    assert!(wrong.install_learned_proposal(proposal()).is_err());
    assert!(wrong.learned_proposal_sha256().is_none());
}

#[test]
fn initialize_restores_native_and_unsupported_automation_cannot_replace_proposal() {
    let mut a = make(44100, true);
    assert!(
        a.set_parameter("head_yaw_deg".into(), sotf_host::ParameterValue::Float(5.0))
            .is_err()
    );
    assert!(a.learned_proposal_sha256().is_some());
    a.initialize(44100.0).unwrap();
    assert!(a.learned_proposal_sha256().is_none());
    let mut native = make(44100, false);
    let x = source();
    assert_eq!(
        render(&mut a, &x, 113, 44100),
        render(&mut native, &x, 113, 44100)
    );
}

#[test]
fn learned_impulses_preserve_declared_startup_and_finite_tail_geometry() {
    for input in 0..2 {
        let mut x = vec![0.0; 4097 * 2];
        x[input] = 0.001;
        let mut p = make(44100, true);
        let mut native = make(44100, false);
        assert_eq!(p.latency_samples(), 2048);
        assert_eq!(p.tail_length(), native.tail_length());
        let y = render(&mut p, &x, 113, 44100);
        let baseline = render(&mut native, &x, 113, 44100);
        assert_eq!(y.len(), baseline.len());
        let onset = y
            .as_chunks::<2>()
            .0
            .iter()
            .position(|f| f.iter().any(|v| v.abs() > 1e-12))
            .unwrap();
        let peak = y
            .as_chunks::<2>()
            .0
            .iter()
            .enumerate()
            .max_by(|a, b| {
                a.1.iter()
                    .map(|v| v.abs())
                    .fold(0.0_f32, f32::max)
                    .total_cmp(&b.1.iter().map(|v| v.abs()).fold(0.0_f32, f32::max))
            })
            .unwrap()
            .0;
        println!("LEARNED_XTC_IMPULSE input={input} onset={onset} peak={peak} declared=2048");
        assert!(onset >= 2048);
    }
}

#[test]
fn constant_matrix_matches_closed_form_delayed_stereo_without_cross_column_swaps() {
    let matrix = [[[0.5, 0.0], [0.25, 0.0]], [[-0.1, 0.0], [0.75, 0.0]]];
    let mut raw: serde_json::Value = serde_json::from_slice(BYTES).unwrap();
    raw["filters_real_imag"] = serde_json::json!(vec![matrix; 1025]);
    let bytes = serde_json::to_vec(&raw).unwrap();
    let prepared = PreparedProposal::from_json(&bytes, &sha(&bytes), MODEL).unwrap();
    let mut p = make(44100, false);
    p.install_learned_proposal(prepared).unwrap();
    let x = source();
    let y = render(&mut p, &x, 113, 44100);
    let delay = 2048;
    let mut maximum = 0.0_f32;
    for (frame, out) in y.as_chunks::<2>().0.iter().enumerate() {
        let sample = frame
            .checked_sub(delay)
            .filter(|i| *i < x.len() / 2)
            .map(|i| [x[2 * i], x[2 * i + 1]])
            .unwrap_or([0.0; 2]);
        let expected = [
            0.5 * sample[0] + 0.25 * sample[1],
            -0.1 * sample[0] + 0.75 * sample[1],
        ];
        for channel in 0..2 {
            maximum = maximum.max((out[channel] - expected[channel]).abs());
        }
    }
    println!("LEARNED_XTC_CONSTANT_MATRIX maximum_error={maximum:e}");
    assert!(maximum < 2e-6);
}

#[test]
fn failed_initialize_retains_prepared_selection_and_populated_audio_history() {
    let x = source();
    let mut a = make(44100, true);
    let mut b = make(44100, true);
    let mut out_a = vec![0.0; 1024];
    let mut out_b = vec![0.0; 1024];
    a.process(&x[..1024], &mut out_a, &ProcessContext::new(44100, 512))
        .unwrap();
    b.process(&x[..1024], &mut out_b, &ProcessContext::new(44100, 512))
        .unwrap();
    assert!(a.initialize(0.0).is_err());
    assert!(a.learned_proposal_sha256().is_some());
    assert_eq!(
        render(&mut a, &x[1024..], 113, 44100),
        render(&mut b, &x[1024..], 113, 44100)
    );
}

#[test]
fn selected_resource_preserves_exact_aligned_disabled_output() {
    let x = source();
    let mut p = make(44100, true);
    p.set_parameter("enabled".into(), sotf_host::ParameterValue::Bool(false))
        .unwrap();
    let y = render(&mut p, &x, 113, 44100);
    let delay = p.latency_samples() * 2;
    assert!(y[..delay].iter().all(|v| *v == 0.0));
    assert_eq!(&y[delay..delay + x.len()], x.as_slice());
    assert!(y[delay + x.len()..].iter().all(|v| *v == 0.0));
    assert!(p.learned_proposal_sha256().is_some());
}
